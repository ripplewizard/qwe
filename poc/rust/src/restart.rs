use crate::runtime::{json_escape, process_alive, process_start_token};
use rusqlite::{params, Connection, OptionalExtension};
use std::fs::OpenOptions;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct RestartTask {
    key: String,
    policy: String,
    max_attempts: i64,
    log_prefix: String,
    state: String,
}

struct RestartInstance {
    attempt: i64,
    pid: i64,
    pgid: i64,
    start_token: String,
    state: String,
    exit_code: Option<i64>,
    log_path: String,
}

pub struct StartRequest<'a> {
    pub connection: &'a Connection,
    pub database: Arc<Mutex<Connection>>,
    pub request_id: &'a str,
    pub key: &'a str,
    pub log_prefix: &'a str,
    pub policy: &'a str,
    pub max_attempts: &'a str,
    pub program: &'a str,
    pub arguments: &'a [String],
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn load_task(connection: &Connection, key: &str) -> Result<Option<RestartTask>, String> {
    connection
        .query_row(
            "SELECT task_key, policy, max_attempts, log_prefix, state
             FROM restart_tasks WHERE task_key = ?1",
            params![key],
            |row| {
                Ok(RestartTask {
                    key: row.get(0)?,
                    policy: row.get(1)?,
                    max_attempts: row.get(2)?,
                    log_prefix: row.get(3)?,
                    state: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn load_instances(connection: &Connection, key: &str) -> Result<Vec<RestartInstance>, String> {
    let mut statement = connection
        .prepare(
            "SELECT attempt, pid, pgid, start_token, state, exit_code, log_path
             FROM task_instances WHERE task_key = ?1 ORDER BY attempt",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![key], |row| {
            Ok(RestartInstance {
                attempt: row.get(0)?,
                pid: row.get(1)?,
                pgid: row.get(2)?,
                start_token: row.get(3)?,
                state: row.get(4)?,
                exit_code: row.get(5)?,
                log_path: row.get(6)?,
            })
        })
        .map_err(|error| error.to_string())?;
    let mut instances = Vec::new();
    for instance in rows {
        instances.push(instance.map_err(|error| error.to_string())?);
    }
    Ok(instances)
}

pub fn status(
    connection: &Connection,
    request_id: &str,
    key: &str,
    created: Option<bool>,
) -> Result<String, String> {
    let Some(task) = load_task(connection, key)? else {
        return Ok(error(
            request_id,
            "not_found",
            &format!("restart task not found: {key}"),
        ));
    };
    let instances = load_instances(connection, key)?;
    let instance_json = instances
        .iter()
        .map(|instance| {
            format!(
                r#"{{"attempt":{},"pid":{},"pgid":{},"start_token":"{}","state":"{}","exit_code":{},"log_path":"{}"}}"#,
                instance.attempt,
                instance.pid,
                instance.pgid,
                json_escape(&instance.start_token),
                json_escape(&instance.state),
                instance
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                json_escape(&instance.log_path)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let created_field = created
        .map(|value| format!(r#","created":{value}"#))
        .unwrap_or_default();
    Ok(format!(
        r#"{{"ok":true,"request_id":"{}","task_key":"{}","policy":"{}","state":"{}"{},"attempts":{},"instances":[{}]}}"#,
        json_escape(request_id),
        json_escape(&task.key),
        json_escape(&task.policy),
        json_escape(&task.state),
        created_field,
        instances.len(),
        instance_json
    ))
}

pub fn start(request: StartRequest<'_>) -> Result<String, String> {
    if !matches!(request.policy, "never" | "on-failure" | "always") {
        return Ok(error(
            request.request_id,
            "invalid_request",
            "policy must be never, on-failure, or always",
        ));
    }
    let max_attempts = request
        .max_attempts
        .parse::<i64>()
        .ok()
        .filter(|value| (1..=20).contains(value))
        .ok_or_else(|| "max attempts must be between 1 and 20".to_string());
    let max_attempts = match max_attempts {
        Ok(value) => value,
        Err(message) => return Ok(error(request.request_id, "invalid_request", &message)),
    };
    let command = std::iter::once(request.program.to_string())
        .chain(request.arguments.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let inserted = request
        .connection
        .execute(
            "INSERT OR IGNORE INTO restart_tasks
             (task_key, policy, max_attempts, log_prefix, command, state,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?6)",
            params![
                request.key,
                request.policy,
                max_attempts,
                request.log_prefix,
                command,
                now()
            ],
        )
        .map_err(|error| error.to_string())?
        == 1;
    let response = status(
        request.connection,
        request.request_id,
        request.key,
        Some(inserted),
    )?;
    if inserted {
        let task = load_task(request.connection, request.key)?
            .ok_or_else(|| "restart task disappeared after insert".to_string())?;
        let program = request.program.to_string();
        let arguments = request.arguments.to_vec();
        thread::spawn(move || run_sequence(request.database, task, program, arguments));
    }
    Ok(response)
}

fn run_sequence(
    database: Arc<Mutex<Connection>>,
    task: RestartTask,
    program: String,
    arguments: Vec<String>,
) {
    let mut failed = false;
    for attempt in 1..=task.max_attempts {
        let exit_code = match run_attempt(&database, &task, attempt, &program, &arguments) {
            Ok(code) => code,
            Err(_) => {
                failed = true;
                break;
            }
        };
        let restart = match task.policy.as_str() {
            "on-failure" => exit_code != 0,
            "always" => true,
            _ => false,
        };
        if !restart {
            break;
        }
    }
    let connection = database
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let final_state = if failed { "failed" } else { "completed" };
    let _ = connection.execute(
        "UPDATE restart_tasks SET state = ?1, updated_at = ?2
         WHERE task_key = ?3",
        params![final_state, now(), task.key],
    );
}

fn run_attempt(
    database: &Arc<Mutex<Connection>>,
    task: &RestartTask,
    attempt: i64,
    program: &str,
    arguments: &[String],
) -> Result<i64, String> {
    let log_path = format!("{}.{}.log", task.log_prefix, attempt);
    let output = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| error.to_string())?;
    let error_output = output.try_clone().map_err(|error| error.to_string())?;
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
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let pid = i64::from(child.id());
    let mut start_token = String::new();
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(10));
        if let Ok(token) = process_start_token(pid) {
            start_token = token;
            break;
        }
        if !process_alive(pid) {
            break;
        }
    }
    if start_token.is_empty() && process_alive(pid) {
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
        let _ = child.wait();
        return Err("restart instance identity unavailable".to_string());
    }
    let instance_id = format!("{}-{}-{}", task.key, attempt, now());
    {
        let connection = database
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let insert_result = connection.execute(
            "INSERT INTO task_instances
                 (id, task_key, attempt, pid, pgid, start_token, state,
                  exit_code, log_path, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', NULL, ?7, ?8)",
            params![
                instance_id,
                task.key,
                attempt,
                pid,
                pid,
                start_token,
                log_path,
                now()
            ],
        );
        if let Err(error) = insert_result {
            unsafe {
                libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
            }
            let _ = child.wait();
            return Err(error.to_string());
        }
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    let exit_code = status
        .code()
        .map(i64::from)
        .unwrap_or_else(|| i64::from(128 + status.signal().unwrap_or(0)));
    let connection = database
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    connection
        .execute(
            "UPDATE task_instances
             SET state = 'exited', exit_code = ?1, ended_at = ?2
             WHERE id = ?3",
            params![exit_code, now(), instance_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(exit_code)
}

fn error(request_id: &str, code: &str, message: &str) -> String {
    format!(
        r#"{{"ok":false,"request_id":"{}","error":"{}","message":"{}"}}"#,
        json_escape(request_id),
        code,
        json_escape(message)
    )
}
