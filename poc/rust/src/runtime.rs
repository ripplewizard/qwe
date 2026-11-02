use rusqlite::{params, Connection, OptionalExtension};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
struct Task {
    id: String,
    key: String,
    command: String,
    log_path: String,
    pid: i64,
    pgid: i64,
    start_token: String,
    state: String,
}

const CURRENT_SCHEMA_VERSION: i64 = 2;

enum DaemonError {
    Invalid(String),
    NotFound(String),
    Internal(String),
}

impl DaemonError {
    fn json(&self) -> (&'static str, &str) {
        match self {
            Self::Invalid(message) => ("invalid_request", message),
            Self::NotFound(message) => ("not_found", message),
            Self::Internal(message) => ("internal", message),
        }
    }
}

fn init_db(path: &str) -> Result<Connection, String> {
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|error| error.to_string())?;
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if version > CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "database schema version {version} is newer than supported version {CURRENT_SCHEMA_VERSION}"
        ));
    }
    connection
        .execute_batch(
            "
            PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                idempotency_key TEXT UNIQUE NOT NULL,
                command TEXT NOT NULL,
                log_path TEXT NOT NULL,
                pid INTEGER NOT NULL,
                pgid INTEGER NOT NULL,
                start_token TEXT NOT NULL,
                state TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS restart_tasks (
                task_key TEXT PRIMARY KEY,
                policy TEXT NOT NULL,
                max_attempts INTEGER NOT NULL,
                log_prefix TEXT NOT NULL,
                command TEXT NOT NULL,
                state TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS task_instances (
                id TEXT PRIMARY KEY,
                task_key TEXT NOT NULL,
                attempt INTEGER NOT NULL,
                pid INTEGER NOT NULL,
                pgid INTEGER NOT NULL,
                start_token TEXT NOT NULL,
                state TEXT NOT NULL,
                exit_code INTEGER,
                log_path TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                ended_at INTEGER,
                UNIQUE(task_key, attempt)
            );
            ",
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute_batch(&format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION};"))
        .map_err(|error| error.to_string())?;
    let migrated_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if migrated_version != CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "database schema migration left version {migrated_version}, expected {CURRENT_SCHEMA_VERSION}"
        ));
    }
    Ok(connection)
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        key: row.get(1)?,
        command: row.get(2)?,
        log_path: row.get(3)?,
        pid: row.get(4)?,
        pgid: row.get(5)?,
        start_token: row.get(6)?,
        state: row.get(7)?,
    })
}

const TASK_COLUMNS: &str = "id, idempotency_key, command, log_path, pid, pgid, start_token, state";

fn load_by_key(connection: &Connection, key: &str) -> Result<Option<Task>, String> {
    connection
        .query_row(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE idempotency_key = ?1"),
            params![key],
            task_from_row,
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn load_by_id(connection: &Connection, id: &str) -> Result<Option<Task>, String> {
    connection
        .query_row(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
            params![id],
            task_from_row,
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn update_state(connection: &Connection, id: &str, state: &str) -> Result<(), String> {
    connection
        .execute(
            "UPDATE tasks SET state = ?1, updated_at = ?2 WHERE id = ?3",
            params![state, now_string(), id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn transition_state(
    connection: &Connection,
    id: &str,
    from: &str,
    to: &str,
) -> Result<bool, String> {
    connection
        .execute(
            "UPDATE tasks SET state = ?1, updated_at = ?2 WHERE id = ?3 AND state = ?4",
            params![to, now_string(), id, from],
        )
        .map(|rows| rows == 1)
        .map_err(|error| error.to_string())
}

fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
pub(crate) fn process_start_token(pid: i64) -> Result<String, String> {
    let stat =
        fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|error| error.to_string())?;
    let end = stat
        .rfind(')')
        .ok_or_else(|| "invalid process stat".to_string())?;
    stat[end + 1..]
        .split_whitespace()
        .nth(19)
        .map(str::to_string)
        .ok_or_else(|| "missing process start time".to_string())
}

#[cfg(target_os = "macos")]
pub(crate) fn process_start_token(pid: i64) -> Result<String, String> {
    let output = Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .map_err(|error| error.to_string())?;
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !token.is_empty() {
        Ok(token)
    } else {
        Err("process not found".to_string())
    }
}

pub(crate) fn process_alive(pid: i64) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn group_alive(pgid: i64) -> bool {
    let result = unsafe { libc::kill(-(pgid as libc::pid_t), 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn task_alive(task: &Task) -> bool {
    process_alive(task.pid)
        && process_start_token(task.pid)
            .map(|token| token == task.start_token)
            .unwrap_or(false)
}

pub(crate) fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn task_response(request_id: &str, task: &Task, created: Option<bool>) -> String {
    let created_field = created
        .map(|value| format!(r#","created":{value}"#))
        .unwrap_or_default();
    format!(
        r#"{{"ok":true,"request_id":"{}","task_id":"{}","pid":{},"pgid":{},"start_token":"{}","state":"{}"{}}}"#,
        json_escape(request_id),
        json_escape(&task.id),
        task.pid,
        task.pgid,
        json_escape(&task.start_token),
        json_escape(&task.state),
        created_field
    )
}

fn error_response(request_id: &str, error: &DaemonError) -> String {
    let (code, message) = error.json();
    format!(
        r#"{{"ok":false,"request_id":"{}","error":"{}","message":"{}"}}"#,
        json_escape(request_id),
        code,
        json_escape(message)
    )
}

fn recover(connection: &Connection) -> Result<(), String> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE state = 'running'"
        ))
        .map_err(|error| error.to_string())?;
    let tasks = statement
        .query_map([], task_from_row)
        .map_err(|error| error.to_string())?;
    let mut collected = Vec::new();
    for task in tasks {
        collected.push(task.map_err(|error| error.to_string())?);
    }
    drop(statement);
    for task in collected {
        if !task_alive(&task) {
            update_state(connection, &task.id, "exited")?;
        }
    }
    Ok(())
}

fn register_task_instance(connection: &Connection, task: &Task) -> Result<(), String> {
    if task.state != "running" {
        return Ok(());
    }
    connection
        .execute(
            "INSERT INTO task_instances
             (id, task_key, attempt, pid, pgid, start_token, state, exit_code,
              log_path, started_at)
             VALUES (?1, ?2, 1, ?3, ?4, ?5, 'running', NULL, ?6, ?7)",
            params![
                format!("{}-instance-1", task.id),
                task.id,
                task.pid,
                task.pgid,
                task.start_token,
                task.log_path,
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as i64)
                    .unwrap_or_default()
            ],
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn finish_task_instance(database: Arc<Mutex<Connection>>, task_id: &str, exit_code: i32) {
    let connection = database
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let ended_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default();
    let state = if exit_code == 0 { "exited" } else { "failed" };
    let _ = connection.execute(
        "UPDATE task_instances
         SET state = ?1, exit_code = ?2, ended_at = ?3
         WHERE task_key = ?4 AND attempt = 1",
        params![state, exit_code, ended_at, task_id],
    );
    let _ = transition_state(&connection, task_id, "running", state);
}

fn task_history_response(connection: &Connection, request_id: &str, task_id: &str) -> String {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?1)",
            params![task_id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !exists {
        return format!(
            r#"{{"ok":false,"request_id":"{}","error":"not_found","message":"task not found: {}"}}"#,
            json_escape(request_id),
            json_escape(task_id)
        );
    }
    let mut statement = match connection.prepare(
        "SELECT attempt, pid, pgid, start_token, state, exit_code,
                started_at, ended_at, log_path
         FROM task_instances WHERE task_key = ?1 ORDER BY attempt",
    ) {
        Ok(statement) => statement,
        Err(error) => return error_response(request_id, &DaemonError::Internal(error.to_string())),
    };
    let rows = match statement.query_map(params![task_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, String>(8)?,
        ))
    }) {
        Ok(rows) => rows,
        Err(error) => return error_response(request_id, &DaemonError::Internal(error.to_string())),
    };
    let mut instances = Vec::new();
    for row in rows {
        let Ok((attempt, pid, pgid, token, state, exit_code, started_at, ended_at, log_path)) = row
        else {
            return error_response(
                request_id,
                &DaemonError::Internal("invalid TaskInstance row".to_string()),
            );
        };
        let exit_json = exit_code
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string());
        let ended_json = ended_at
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string());
        let duration_json = ended_at
            .map(|ended| (ended - started_at).to_string())
            .unwrap_or_else(|| "null".to_string());
        instances.push(format!(
            r#"{{"attempt":{},"pid":{},"pgid":{},"start_token":"{}","state":"{}","exit_code":{},"started_at":{},"ended_at":{},"duration_ms":{},"log_path":"{}"}}"#,
            attempt,
            pid,
            pgid,
            json_escape(&token),
            state,
            exit_json,
            started_at,
            ended_json,
            duration_json,
            json_escape(&log_path)
        ));
    }
    format!(
        r#"{{"ok":true,"request_id":"{}","task_id":"{}","attempts":{},"instances":[{}]}}"#,
        json_escape(request_id),
        json_escape(task_id),
        instances.len(),
        instances.join(",")
    )
}

fn signal_task_response(
    connection: &Connection,
    request_id: &str,
    task_id: &str,
    signal_name: &str,
) -> String {
    let task = match load_by_id(connection, task_id) {
        Ok(Some(task)) => task,
        Ok(None) => {
            return error_response(
                request_id,
                &DaemonError::NotFound(format!("task not found: {task_id}")),
            )
        }
        Err(error) => return error_response(request_id, &DaemonError::Internal(error)),
    };
    if task.state != "running" || !task_alive(&task) {
        return error_response(
            request_id,
            &DaemonError::Invalid(format!("task is not running: {task_id}")),
        );
    }
    let signal = match signal_name {
        "SIGINT" => libc::SIGINT,
        "SIGTERM" => libc::SIGTERM,
        "SIGKILL" => libc::SIGKILL,
        _ => {
            return error_response(
                request_id,
                &DaemonError::Invalid("signal must be SIGINT, SIGTERM, or SIGKILL".to_string()),
            )
        }
    };
    let result = unsafe { libc::kill(-(task.pgid as libc::pid_t), signal) };
    if result != 0 {
        return error_response(
            request_id,
            &DaemonError::Internal(std::io::Error::last_os_error().to_string()),
        );
    }
    task_response(request_id, &task, None)
}

fn start_task(
    connection: &Connection,
    database: Arc<Mutex<Connection>>,
    key: &str,
    log_path: &str,
    program: &str,
    arguments: &[String],
) -> Result<(Task, bool), DaemonError> {
    if let Some(task) = load_by_key(connection, key).map_err(DaemonError::Internal)? {
        if task.state == "running" && task_alive(&task) {
            return Ok((task, false));
        }
        return Err(DaemonError::Internal(
            "idempotency key already used by a non-running task".to_string(),
        ));
    }

    let output = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .map_err(|error| DaemonError::Internal(error.to_string()))?;
    let error_output = output
        .try_clone()
        .map_err(|error| DaemonError::Internal(error.to_string()))?;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(error_output);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| DaemonError::Internal(error.to_string()))?;
    let pid = i64::from(child.id());
    let mut start_token = None;
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(10));
        match process_start_token(pid) {
            Ok(token) => {
                start_token = Some(token);
                break;
            }
            Err(_) if !process_alive(pid) => break,
            Err(_) => {}
        }
    }
    let (start_token, state) = match start_token {
        Some(token) => (token, "running".to_string()),
        None if !process_alive(pid) => (String::new(), "exited".to_string()),
        None => {
            unsafe {
                libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
            }
            return Err(DaemonError::Internal(
                "task identity unavailable for a running process".to_string(),
            ));
        }
    };
    let task = Task {
        id: format!("task-{}-{}", pid, unique_suffix()),
        key: key.to_string(),
        command: std::iter::once(program.to_string())
            .chain(arguments.iter().cloned())
            .collect::<Vec<_>>()
            .join(" "),
        log_path: log_path.to_string(),
        pid,
        pgid: pid,
        start_token,
        state,
    };
    let insert_result = connection.execute(
        "INSERT INTO tasks
             (id, idempotency_key, command, log_path, pid, pgid, start_token,
              state, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        params![
            task.id,
            task.key,
            task.command,
            task.log_path,
            task.pid,
            task.pgid,
            task.start_token,
            task.state,
            now_string()
        ],
    );
    if let Err(error) = insert_result {
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
        return Err(DaemonError::Internal(error.to_string()));
    }
    if let Err(error) = register_task_instance(connection, &task) {
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
        let _ = child.wait();
        let _ = update_state(connection, &task.id, "failed");
        return Err(DaemonError::Internal(error));
    }
    let task_id = task.id.clone();
    thread::spawn(move || {
        let status = child.wait().ok();
        let exit_code = status
            .as_ref()
            .map(|status| {
                status
                    .code()
                    .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
            })
            .unwrap_or(1);
        finish_task_instance(database, &task_id, exit_code);
    });
    Ok((task, true))
}

fn stop_task(connection: &Connection, id: &str) -> Result<Task, DaemonError> {
    let mut task = load_by_id(connection, id)
        .map_err(DaemonError::Internal)?
        .ok_or_else(|| DaemonError::NotFound(format!("task not found: {id}")))?;
    if task.state != "running" {
        return Ok(task);
    }

    if group_alive(task.pgid) {
        unsafe {
            libc::kill(-(task.pgid as libc::pid_t), libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while group_alive(task.pgid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        if group_alive(task.pgid) {
            unsafe {
                libc::kill(-(task.pgid as libc::pid_t), libc::SIGKILL);
            }
        }
    }

    update_state(connection, id, "stopped").map_err(DaemonError::Internal)?;
    task.state = "stopped".to_string();
    Ok(task)
}

fn start_task_with_timeout(
    connection: &Connection,
    database: Arc<Mutex<Connection>>,
    key: &str,
    log_path: &str,
    timeout_ms: &str,
    program: &str,
    arguments: &[String],
) -> Result<(Task, bool), DaemonError> {
    let timeout_ms = timeout_ms
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| DaemonError::Invalid("timeout must be a positive integer".to_string()))?;
    let started = Instant::now();
    let (task, created) = start_task(
        connection,
        Arc::clone(&database),
        key,
        log_path,
        program,
        arguments,
    )?;
    if created && task.state == "running" {
        let timeout = Duration::from_millis(timeout_ms).saturating_sub(started.elapsed());
        let task = Task {
            id: task.id.clone(),
            key: task.key.clone(),
            command: task.command.clone(),
            log_path: task.log_path.clone(),
            pid: task.pid,
            pgid: task.pgid,
            start_token: task.start_token.clone(),
            state: task.state.clone(),
        };
        thread::spawn(move || enforce_task_timeout(database, task, timeout));
    }
    Ok((task, created))
}

fn enforce_task_timeout(database: Arc<Mutex<Connection>>, task: Task, timeout: Duration) {
    thread::sleep(timeout);
    {
        let connection = database
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = load_by_id(&connection, &task.id).ok().flatten();
        if current
            .as_ref()
            .is_none_or(|current| current.state != "running" || !task_alive(current))
        {
            return;
        }
        match transition_state(&connection, &task.id, "running", "timing_out") {
            Ok(true) => {}
            _ => return,
        }
    }
    if group_alive(task.pgid) {
        unsafe {
            libc::kill(-(task.pgid as libc::pid_t), libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_millis(200);
        while group_alive(task.pgid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if group_alive(task.pgid) {
            unsafe {
                libc::kill(-(task.pgid as libc::pid_t), libc::SIGKILL);
            }
        }
    }
    for _ in 0..50 {
        if !group_alive(task.pgid) {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let connection = database
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let final_state = if group_alive(task.pgid) {
        "timeout_failed"
    } else {
        "timed_out"
    };
    let _ = transition_state(&connection, &task.id, "timing_out", final_state);
}

fn handle_request(
    connection: &Connection,
    database: &Arc<Mutex<Connection>>,
    leases: &Arc<Mutex<crate::lease::LeaseManager>>,
    sessions: &mut crate::session::SessionManager,
    line: &str,
) -> String {
    let fields: Vec<&str> = line.trim_end().split('\t').collect();
    let request_id = fields.get(1).copied().unwrap_or("unknown");
    let result = match fields.as_slice() {
        ["START", _, key, log_path, program, arguments @ ..] => {
            let arguments: Vec<String> = arguments.iter().map(|value| value.to_string()).collect();
            start_task(
                connection,
                Arc::clone(database),
                key,
                log_path,
                program,
                &arguments,
            )
            .map(|(task, created)| task_response(request_id, &task, Some(created)))
        }
        ["TIMEOUT_START", _, key, log_path, timeout_ms, program, arguments @ ..] => {
            let arguments: Vec<String> = arguments.iter().map(|value| value.to_string()).collect();
            start_task_with_timeout(
                connection,
                Arc::clone(database),
                key,
                log_path,
                timeout_ms,
                program,
                &arguments,
            )
            .map(|(task, created)| task_response(request_id, &task, Some(created)))
        }
        ["RESTART_START", _, key, log_prefix, policy, max_attempts, program, arguments @ ..] => {
            let arguments: Vec<String> = arguments.iter().map(|value| value.to_string()).collect();
            crate::restart::start(crate::restart::StartRequest {
                connection,
                database: Arc::clone(database),
                request_id,
                key,
                log_prefix,
                policy,
                max_attempts,
                program,
                arguments: &arguments,
            })
            .map_err(DaemonError::Internal)
        }
        ["RESTART_STATUS", _, key] => {
            crate::restart::status(connection, request_id, key, None).map_err(DaemonError::Internal)
        }
        ["STATUS", _, id] => load_by_id(connection, id)
            .map_err(DaemonError::Internal)
            .and_then(|task| {
                task.ok_or_else(|| DaemonError::NotFound(format!("task not found: {id}")))
            })
            .map(|mut task| {
                if task.state == "running" && !task_alive(&task) {
                    let _ = update_state(connection, &task.id, "exited");
                    task.state = "exited".to_string();
                }
                task_response(request_id, &task, None)
            }),
        ["STOP", _, id] => {
            stop_task(connection, id).map(|task| task_response(request_id, &task, None))
        }
        ["TASK_HISTORY", _, id] => Ok(task_history_response(connection, request_id, id)),
        ["TASK_SIGNAL", _, id, signal_name] => Ok(signal_task_response(
            connection,
            request_id,
            id,
            signal_name,
        )),
        ["TASK_RESOURCE", _, id] => load_by_id(connection, id)
            .map_err(DaemonError::Internal)
            .and_then(|task| {
                task.ok_or_else(|| DaemonError::NotFound(format!("task not found: {id}")))
            })
            .and_then(|task| {
                if task.state != "running" || !task_alive(&task) {
                    return Err(DaemonError::Invalid(format!("task is not running: {id}")));
                }
                crate::resource::response(id, task.pgid, request_id).map_err(DaemonError::Internal)
            }),
        ["TASK_RESOURCE_SAMPLE", _, id, interval, count] => load_by_id(connection, id)
            .map_err(DaemonError::Internal)
            .and_then(|task| {
                task.ok_or_else(|| DaemonError::NotFound(format!("task not found: {id}")))
            })
            .and_then(|task| {
                if task.state != "running" || !task_alive(&task) {
                    return Err(DaemonError::Invalid(format!("task is not running: {id}")));
                }
                crate::resource::sample(id, task.pgid, request_id, interval, count)
                    .map_err(DaemonError::Internal)
            }),
        ["LEASE_CLAIM", _, session_id, client_id] => {
            let mut leases = leases
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Ok(leases.claim(request_id, session_id, client_id))
        }
        ["LEASE_TOUCH", _, session_id, client_id] => {
            let mut leases = leases
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Ok(leases.touch(request_id, session_id, client_id))
        }
        ["LEASE_RELEASE", _, session_id, client_id] => {
            let mut leases = leases
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Ok(leases.release(request_id, session_id, client_id))
        }
        ["LEASE_STATUS", _, session_id] => {
            let mut leases = leases
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Ok(leases.status(request_id, session_id))
        }
        ["LEASE_TAKEOVER", _, session_id, client_id] => {
            let mut leases = leases
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Ok(leases.takeover(request_id, session_id, client_id))
        }
        ["SESSION_START", _, session_id, log_path, program, arguments @ ..] => {
            let arguments: Vec<String> = arguments.iter().map(|value| value.to_string()).collect();
            Ok(sessions.start(request_id, session_id, log_path, program, &arguments))
        }
        ["SESSION_ATTACH", _, session_id, client_id] => {
            Ok(sessions.attach(request_id, session_id, client_id))
        }
        ["SESSION_DETACH", _, session_id, client_id] => {
            Ok(sessions.detach(request_id, session_id, client_id))
        }
        ["SESSION_INPUT", _, session_id, client_id, encoded] => {
            Ok(sessions.input(request_id, session_id, client_id, encoded))
        }
        ["SESSION_STATUS", _, session_id] => Ok(sessions.status(request_id, session_id)),
        _ => Err(DaemonError::Invalid("unsupported request".to_string())),
    };
    result.unwrap_or_else(|error| error_response(request_id, &error))
}

pub fn run_daemon(socket_path: &str, database_path: &str) -> Result<(), String> {
    let connection = init_db(database_path)?;
    recover(&connection)?;
    let _ = fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path).map_err(|error| error.to_string())?;
    let connection = Arc::new(Mutex::new(connection));
    let leases = Arc::new(Mutex::new(crate::lease::LeaseManager::new()));
    let sessions = Arc::new(Mutex::new(crate::session::SessionManager::new(Arc::clone(
        &leases,
    ))));
    for stream in listener.incoming() {
        let stream = stream.map_err(|error| error.to_string())?;
        let connection = Arc::clone(&connection);
        let leases = Arc::clone(&leases);
        let sessions = Arc::clone(&sessions);
        thread::spawn(move || {
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            if let Err(error) = reader.read_line(&mut line) {
                eprintln!("IPC read failed: {error}");
                return;
            }
            let response = {
                let database = Arc::clone(&connection);
                let connection = connection
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let mut sessions = sessions
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                handle_request(&connection, &database, &leases, &mut sessions, &line)
            };
            let mut writer = &stream;
            if let Err(error) = writeln!(writer, "{response}") {
                eprintln!("IPC write failed: {error}");
            }
        });
    }
    Ok(())
}

fn client_request(socket_path: &str, request: &str) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket_path).map_err(|error| error.to_string())?;
    writeln!(stream, "{request}").map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .map_err(|error| error.to_string())?;
    print!("{response}");
    Ok(())
}

pub fn run_dashboard(database_path: &str) -> ExitCode {
    let render = || -> Result<String, String> {
        let connection = Connection::open(database_path).map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare("SELECT state, COUNT(*) FROM tasks GROUP BY state ORDER BY state")
            .map_err(|error| error.to_string())?;
        let states = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let latest = connection
            .query_row(
                "SELECT id, idempotency_key, state FROM tasks ORDER BY updated_at DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let task_count: i64 = states.iter().map(|(_, count)| count).sum();
        let states_text = if states.is_empty() {
            "none".to_string()
        } else {
            states
                .iter()
                .map(|(state, count)| format!("{state}={count}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let latest_text = latest
            .map(|(id, key, state)| format!("latest: id={id} key={key} state={state}"))
            .unwrap_or_else(|| "latest: none".to_string());
        Ok(format!(
            "\x1b[?1049h\x1b[H\x1b[2JQWE POC DASHBOARD\r\ndatabase: {database_path}\r\ntasks={task_count} {states_text}\r\n{latest_text}\r\n\r\n[q] quit (one-shot POC screen)\x1b[?1049l"
        ))
    };
    match render() {
        Ok(screen) => {
            print!("{screen}");
            let _ = std::io::stdout().flush();
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("dashboard failed: {error}");
            ExitCode::FAILURE
        }
    }
}

pub fn client_command(operation: &str) -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(2).collect();
    let exact_arguments = match operation {
        "START" | "TIMEOUT_START" | "RESTART_START" | "SESSION_START" => None,
        "SESSION_INPUT" => Some(4),
        "SESSION_ATTACH" | "SESSION_DETACH" => Some(3),
        "TASK_SIGNAL" => Some(3),
        "TASK_RESOURCE_SAMPLE" => Some(4),
        "LEASE_CLAIM" | "LEASE_TOUCH" | "LEASE_RELEASE" | "LEASE_TAKEOVER" => Some(3),
        _ => Some(2),
    };
    let valid_arguments = match exact_arguments {
        Some(required) => arguments.len() == required,
        None => {
            let minimum = match operation {
                "TIMEOUT_START" => 5,
                "RESTART_START" => 6,
                _ => 4,
            };
            arguments.len() >= minimum
        }
    };
    if !valid_arguments {
        if operation == "START" {
            eprintln!("usage: qwe-poc task-start <socket> <key> <log> <program> [args...]");
        } else if operation == "TIMEOUT_START" {
            eprintln!(
                "usage: qwe-poc task-start-timeout <socket> <key> <log> <timeout-ms> <program> [args...]"
            );
        } else if operation == "RESTART_START" {
            eprintln!(
                "usage: qwe-poc task-start-restart <socket> <key> <log-prefix> <policy> <max-attempts> <program> [args...]"
            );
        } else if operation == "SESSION_START" {
            eprintln!(
                "usage: qwe-poc session-start <socket> <session-id> <log> <program> [args...]"
            );
        } else {
            eprintln!(
                "usage: qwe-poc task-{} <socket> <task-id>",
                operation.to_lowercase()
            );
        }
        return ExitCode::from(2);
    }

    let request_id = format!("request-{}-{}", std::process::id(), unique_suffix());
    let mut fields = vec![operation.to_string(), request_id];
    fields.extend(arguments.iter().skip(1).cloned());
    let request = fields.join("\t");
    match client_request(&arguments[0], &request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("IPC failed: {error}");
            ExitCode::FAILURE
        }
    }
}
