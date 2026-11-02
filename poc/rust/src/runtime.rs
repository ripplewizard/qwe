use rusqlite::{params, Connection, OptionalExtension};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
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
            ",
        )
        .map_err(|error| error.to_string())?;
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
fn process_start_token(pid: i64) -> Result<String, String> {
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
fn process_start_token(pid: i64) -> Result<String, String> {
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

fn process_alive(pid: i64) -> bool {
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

fn json_escape(value: &str) -> String {
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

fn start_task(
    connection: &Connection,
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
    thread::spawn(move || {
        let _ = child.wait();
    });
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

fn handle_request(connection: &Connection, line: &str) -> String {
    let fields: Vec<&str> = line.trim_end().split('\t').collect();
    let request_id = fields.get(1).copied().unwrap_or("unknown");
    let result = match fields.as_slice() {
        ["START", _, key, log_path, program, arguments @ ..] => {
            let arguments: Vec<String> = arguments.iter().map(|value| value.to_string()).collect();
            start_task(connection, key, log_path, program, &arguments)
                .map(|(task, created)| task_response(request_id, &task, Some(created)))
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
    for stream in listener.incoming() {
        let stream = stream.map_err(|error| error.to_string())?;
        let connection = Arc::clone(&connection);
        thread::spawn(move || {
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            if let Err(error) = reader.read_line(&mut line) {
                eprintln!("IPC read failed: {error}");
                return;
            }
            let response = {
                let connection = connection
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                handle_request(&connection, &line)
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

pub fn client_command(operation: &str) -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(2).collect();
    let minimum = if operation == "START" { 4 } else { 2 };
    if arguments.len() < minimum {
        if operation == "START" {
            eprintln!("usage: qwe-poc task-start <socket> <key> <log> <program> [args...]");
        } else {
            eprintln!(
                "usage: qwe-poc task-{} <socket> <task-id>",
                operation.to_lowercase()
            );
        }
        return ExitCode::from(2);
    }
    if operation != "START" && arguments.len() != 2 {
        eprintln!(
            "usage: qwe-poc task-{} <socket> <task-id>",
            operation.to_lowercase()
        );
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
