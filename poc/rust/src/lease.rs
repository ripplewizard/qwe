use crate::runtime::json_escape;
use std::collections::HashMap;
use std::time::{Duration, Instant};

struct LeaseRecord {
    owner: String,
    deadline: Instant,
}

pub struct LeaseManager {
    idle_timeout: Duration,
    sessions: HashMap<String, LeaseRecord>,
    output_bytes: HashMap<String, i64>,
}

impl LeaseManager {
    pub fn new() -> Self {
        let idle_timeout = std::env::var("QWE_LEASE_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|milliseconds| *milliseconds > 0)
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(600));
        Self {
            idle_timeout,
            sessions: HashMap::new(),
            output_bytes: HashMap::new(),
        }
    }

    fn state(&self, session_id: &str, now: Instant) -> (&'static str, Option<String>) {
        match self.sessions.get(session_id) {
            Some(record) if now < record.deadline => ("owned", Some(record.owner.clone())),
            Some(_) => ("expired", None),
            None => ("idle", None),
        }
    }

    fn response(
        &self,
        request_id: &str,
        session_id: &str,
        state: &str,
        owner: Option<&str>,
        remaining_ms: u128,
    ) -> String {
        let owner = owner
            .map(|value| format!("\"{}\"", json_escape(value)))
            .unwrap_or_else(|| "null".to_string());
        let output_bytes = self.output_bytes.get(session_id).copied().unwrap_or(0);
        format!(
            r#"{{"ok":true,"request_id":"{}","session_id":"{}","state":"{}","owner":{},"remaining_ms":{},"output_bytes":{}}}"#,
            json_escape(request_id),
            json_escape(session_id),
            state,
            owner,
            remaining_ms,
            output_bytes
        )
    }

    fn error(&self, request_id: &str, code: &str, message: &str) -> String {
        format!(
            r#"{{"ok":false,"request_id":"{}","error":"{}","message":"{}"}}"#,
            json_escape(request_id),
            code,
            json_escape(message)
        )
    }

    pub fn claim(&mut self, request_id: &str, session_id: &str, client_id: &str) -> String {
        let now = Instant::now();
        let (state, owner) = self.state(session_id, now);
        if state == "owned" {
            return self.error(
                request_id,
                "already_owned",
                &format!("session is already owned by {}", owner.unwrap_or_default()),
            );
        }
        let deadline = now + self.idle_timeout;
        self.sessions.insert(
            session_id.to_string(),
            LeaseRecord {
                owner: client_id.to_string(),
                deadline,
            },
        );
        self.output_bytes.insert(session_id.to_string(), 0);
        self.response(
            request_id,
            session_id,
            "owned",
            Some(client_id),
            deadline.saturating_duration_since(now).as_millis(),
        )
    }

    pub fn touch(&mut self, request_id: &str, session_id: &str, client_id: &str) -> String {
        let now = Instant::now();
        let (state, owner) = self.state(session_id, now);
        if state != "owned" || owner.as_deref() != Some(client_id) {
            return self.error(
                request_id,
                "not_owner",
                &format!("client does not own session {session_id}"),
            );
        }
        let deadline = now + self.idle_timeout;
        self.sessions.insert(
            session_id.to_string(),
            LeaseRecord {
                owner: client_id.to_string(),
                deadline,
            },
        );
        self.response(
            request_id,
            session_id,
            "owned",
            Some(client_id),
            deadline.saturating_duration_since(now).as_millis(),
        )
    }

    pub fn release(&mut self, request_id: &str, session_id: &str, client_id: &str) -> String {
        let now = Instant::now();
        let (state, owner) = self.state(session_id, now);
        if state != "owned" || owner.as_deref() != Some(client_id) {
            return self.error(
                request_id,
                "not_owner",
                &format!("client does not own session {session_id}"),
            );
        }
        self.sessions.remove(session_id);
        self.response(request_id, session_id, "idle", None, 0)
    }

    pub fn status(&mut self, request_id: &str, session_id: &str) -> String {
        let now = Instant::now();
        let (state, owner) = self.state(session_id, now);
        let remaining = self
            .sessions
            .get(session_id)
            .map(|record| record.deadline.saturating_duration_since(now).as_millis())
            .unwrap_or(0);
        self.response(request_id, session_id, state, owner.as_deref(), remaining)
    }

    pub fn takeover(&mut self, request_id: &str, session_id: &str, client_id: &str) -> String {
        let now = Instant::now();
        let (state, _) = self.state(session_id, now);
        if state != "expired" {
            return self.error(request_id, "not_expired", "session lease has not expired");
        }
        let deadline = now + self.idle_timeout;
        self.sessions.insert(
            session_id.to_string(),
            LeaseRecord {
                owner: client_id.to_string(),
                deadline,
            },
        );
        self.output_bytes.insert(session_id.to_string(), 0);
        self.response(
            request_id,
            session_id,
            "owned",
            Some(client_id),
            deadline.saturating_duration_since(now).as_millis(),
        )
    }

    pub fn record_output(&mut self, session_id: &str, bytes: i64) {
        *self.output_bytes.entry(session_id.to_string()).or_insert(0) += bytes;
    }

    pub fn is_owner(&self, session_id: &str, client_id: &str) -> bool {
        let (state, owner) = self.state(session_id, Instant::now());
        state == "owned" && owner.as_deref() == Some(client_id)
    }

    pub fn authorize_input(&mut self, session_id: &str, client_id: &str) -> bool {
        let now = Instant::now();
        let (state, owner) = self.state(session_id, now);
        if state != "owned" || owner.as_deref() != Some(client_id) {
            return false;
        }
        self.sessions.insert(
            session_id.to_string(),
            LeaseRecord {
                owner: client_id.to_string(),
                deadline: now + self.idle_timeout,
            },
        );
        true
    }
}
