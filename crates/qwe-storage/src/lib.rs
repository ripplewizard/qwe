use qwe_core::{
    AttachmentAuthority, AttachmentAuthorityState, LeaseRecord, PaneId, PaneRecord,
    ProcessIdentity, ResourcePolicy, SessionRecord, TaskId, TaskRecord, TaskState,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::str::FromStr;
use thiserror::Error;

pub const SCHEMA_VERSION: i64 = 10;
pub const IDEMPOTENCY_RETENTION_MS: i64 = 24 * 60 * 60 * 1_000;
pub const HISTORY_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
pub const HISTORY_MAX_TERMINAL_SESSIONS: i64 = 10_000;
pub const IDEMPOTENCY_PENDING: &str = "pending";
pub const IDEMPOTENCY_COMPLETE: &str = "complete";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyRecord {
    pub key: String,
    pub operation: String,
    pub request_fingerprint: String,
    pub state: String,
    pub response_json: Option<String>,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HistoryPruneReport {
    pub sessions: u64,
    pub tasks: u64,
    pub panes: u64,
    pub leases: u64,
    pub attachment_authorities: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LeaseError {
    #[error("lease is held by another owner")]
    AlreadyHeld,
    #[error("lease has expired")]
    Expired,
    #[error("lease owner or generation does not match")]
    NotOwner,
    #[error("lease does not exist")]
    NotFound,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Lease(#[from] LeaseError),
    #[error(transparent)]
    Uuid(#[from] uuid::Error),
    #[error("database schema is newer than this binary")]
    SchemaTooNew,
    #[error("invalid storage invariant: {0}")]
    Invariant(String),
}

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: &str) -> Result<Self, StorageError> {
        let connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch("PRAGMA foreign_keys=ON;")?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<(), StorageError> {
        let mut current: i64 = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if current > SCHEMA_VERSION {
            return Err(StorageError::SchemaTooNew);
        }
        if current == 0 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    owner TEXT NOT NULL,
                    state TEXT NOT NULL,
                    pid INTEGER,
                    pgid INTEGER,
                    start_token TEXT,
                    worker_pid INTEGER,
                    worker_pgid INTEGER,
                    worker_start_token TEXT,
                    updated_at_ms INTEGER NOT NULL
                );
                CREATE TABLE tasks (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL
                        REFERENCES sessions(id) ON DELETE RESTRICT,
                    pid INTEGER NOT NULL,
                    pgid INTEGER NOT NULL,
                    start_token TEXT NOT NULL,
                    state TEXT NOT NULL,
                    exit_code INTEGER,
                    exit_signal TEXT,
                    started_at_ms INTEGER NOT NULL,
                    ended_at_ms INTEGER
                );
                CREATE UNIQUE INDEX tasks_by_session ON tasks(session_id);
                CREATE TABLE panes (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL UNIQUE
                        REFERENCES sessions(id) ON DELETE RESTRICT,
                    cpu_seconds INTEGER,
                    memory_high_bytes INTEGER,
                    memory_bytes INTEGER,
                    memory_swap_max_bytes INTEGER,
                    process_limit INTEGER,
                    cpu_quota_percent INTEGER,
                    updated_at_ms INTEGER NOT NULL
                );
                CREATE TABLE leases (
                    session_id TEXT PRIMARY KEY
                        REFERENCES sessions(id) ON DELETE RESTRICT,
                    owner TEXT NOT NULL,
                    generation INTEGER NOT NULL,
                    deadline_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    released_at_ms INTEGER
                );
                CREATE TABLE attachment_authorities (
                    session_id TEXT PRIMARY KEY
                        REFERENCES sessions(id) ON DELETE RESTRICT,
                    state TEXT NOT NULL,
                    owner TEXT,
                    generation INTEGER NOT NULL,
                    reclaim_token_hash TEXT,
                    deadline_ms INTEGER,
                    updated_at_ms INTEGER NOT NULL
                );
                CREATE TABLE idempotency_records (
                    idempotency_key TEXT PRIMARY KEY,
                    operation TEXT NOT NULL,
                    request_fingerprint TEXT NOT NULL,
                    state TEXT NOT NULL,
                    response_json TEXT,
                    created_at_ms INTEGER NOT NULL,
                    expires_at_ms INTEGER NOT NULL
                );
                CREATE INDEX idempotency_expiry
                    ON idempotency_records(expires_at_ms);
                PRAGMA user_version = 10;
                ",
            )?;
            transaction.commit()?;
            return Ok(());
        }
        if current == 1 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "
                ALTER TABLE sessions ADD COLUMN pid INTEGER;
                ALTER TABLE sessions ADD COLUMN pgid INTEGER;
                ALTER TABLE sessions ADD COLUMN start_token TEXT;
                ALTER TABLE sessions ADD COLUMN updated_at_ms INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE tasks ADD COLUMN started_at_ms INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE tasks ADD COLUMN ended_at_ms INTEGER;
                ALTER TABLE tasks ADD COLUMN exit_signal TEXT;
                CREATE TABLE leases (
                    session_id TEXT PRIMARY KEY,
                    owner TEXT NOT NULL,
                    generation INTEGER NOT NULL,
                    deadline_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL
                );
                CREATE INDEX tasks_by_session ON tasks(session_id);
                PRAGMA user_version = 2;
                ",
            )?;
            transaction.commit()?;
            current = 2;
        }
        if current == 2 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "
                ALTER TABLE leases ADD COLUMN released_at_ms INTEGER;
                PRAGMA user_version = 3;
                ",
            )?;
            transaction.commit()?;
            current = 3;
        }
        if current == 3 {
            let duplicates: i64 = self.connection.query_row(
                "SELECT COUNT(*) FROM (
                    SELECT session_id FROM tasks GROUP BY session_id HAVING COUNT(*) > 1
                )",
                [],
                |row| row.get(0),
            )?;
            if duplicates > 0 {
                return Err(StorageError::Invariant(format!(
                    "schema cutover requires one Task per Session; {duplicates} Sessions have duplicate Tasks"
                )));
            }
            let mut statement = self.connection.prepare("SELECT id FROM sessions")?;
            let session_ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "
                DROP INDEX IF EXISTS tasks_by_session;
                CREATE UNIQUE INDEX tasks_by_session ON tasks(session_id);
                CREATE TABLE panes (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL UNIQUE,
                    updated_at_ms INTEGER NOT NULL
                );
                ",
            )?;
            for session_id in session_ids {
                transaction.execute(
                    "INSERT INTO panes (id, session_id, updated_at_ms) VALUES (?1, ?2, ?3)",
                    params![PaneId::new().to_string(), session_id, 0_i64],
                )?;
            }
            transaction.execute_batch("PRAGMA user_version = 4;")?;
            transaction.commit()?;
            current = 4;
        }
        if current == 4 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "
                ALTER TABLE sessions ADD COLUMN worker_pid INTEGER;
                ALTER TABLE sessions ADD COLUMN worker_pgid INTEGER;
                ALTER TABLE sessions ADD COLUMN worker_start_token TEXT;
                PRAGMA user_version = 5;
                ",
            )?;
            transaction.commit()?;
            current = 5;
        }
        if current == 5 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "
                CREATE TABLE tasks_v6 (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL
                        REFERENCES sessions(id) ON DELETE RESTRICT,
                    pid INTEGER NOT NULL,
                    pgid INTEGER NOT NULL,
                    start_token TEXT NOT NULL,
                    state TEXT NOT NULL,
                    exit_code INTEGER,
                    exit_signal TEXT,
                    started_at_ms INTEGER NOT NULL,
                    ended_at_ms INTEGER
                );
                INSERT INTO tasks_v6
                    SELECT id, session_id, pid, pgid, start_token, state,
                           exit_code, exit_signal, started_at_ms, ended_at_ms
                    FROM tasks;
                DROP INDEX tasks_by_session;
                DROP TABLE tasks;
                ALTER TABLE tasks_v6 RENAME TO tasks;
                CREATE UNIQUE INDEX tasks_by_session ON tasks(session_id);

                CREATE TABLE panes_v6 (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL UNIQUE
                        REFERENCES sessions(id) ON DELETE RESTRICT,
                    updated_at_ms INTEGER NOT NULL
                );
                INSERT INTO panes_v6 SELECT id, session_id, updated_at_ms FROM panes;
                DROP TABLE panes;
                ALTER TABLE panes_v6 RENAME TO panes;

                CREATE TABLE leases_v6 (
                    session_id TEXT PRIMARY KEY
                        REFERENCES sessions(id) ON DELETE RESTRICT,
                    owner TEXT NOT NULL,
                    generation INTEGER NOT NULL,
                    deadline_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    released_at_ms INTEGER
                );
                INSERT INTO leases_v6
                    SELECT session_id, owner, generation, deadline_ms,
                           updated_at_ms, released_at_ms
                    FROM leases;
                DROP TABLE leases;
                ALTER TABLE leases_v6 RENAME TO leases;
                PRAGMA user_version = 6;
                ",
            )?;
            transaction.commit()?;
            current = 6;
        }
        if current == 6 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "
                CREATE TABLE attachment_authorities (
                    session_id TEXT PRIMARY KEY
                        REFERENCES sessions(id) ON DELETE RESTRICT,
                    state TEXT NOT NULL,
                    owner TEXT,
                    generation INTEGER NOT NULL,
                    reclaim_token_hash TEXT,
                    deadline_ms INTEGER,
                    updated_at_ms INTEGER NOT NULL
                );
                CREATE TABLE idempotency_records (
                    idempotency_key TEXT PRIMARY KEY,
                    operation TEXT NOT NULL,
                    request_fingerprint TEXT NOT NULL,
                    response_json TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    expires_at_ms INTEGER NOT NULL
                );
                CREATE INDEX idempotency_expiry
                    ON idempotency_records(expires_at_ms);
                PRAGMA user_version = 7;
                ",
            )?;
            transaction.commit()?;
            current = 7;
        }
        if current == 7 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "
                CREATE TABLE idempotency_records_v8 (
                    idempotency_key TEXT PRIMARY KEY,
                    operation TEXT NOT NULL,
                    request_fingerprint TEXT NOT NULL,
                    state TEXT NOT NULL,
                    response_json TEXT,
                    created_at_ms INTEGER NOT NULL,
                    expires_at_ms INTEGER NOT NULL
                );
                INSERT INTO idempotency_records_v8 (
                    idempotency_key, operation, request_fingerprint, state,
                    response_json, created_at_ms, expires_at_ms
                )
                SELECT idempotency_key, operation, request_fingerprint, 'complete',
                       response_json, created_at_ms, expires_at_ms
                FROM idempotency_records;
                DROP INDEX idempotency_expiry;
                DROP TABLE idempotency_records;
                ALTER TABLE idempotency_records_v8 RENAME TO idempotency_records;
                CREATE INDEX idempotency_expiry
                    ON idempotency_records(expires_at_ms);
                PRAGMA user_version = 8;
                ",
            )?;
            transaction.commit()?;
            current = 8;
        }
        if current == 8 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "
                ALTER TABLE panes ADD COLUMN cpu_seconds INTEGER;
                ALTER TABLE panes ADD COLUMN memory_bytes INTEGER;
                ALTER TABLE panes ADD COLUMN process_limit INTEGER;
                PRAGMA user_version = 9;
                ",
            )?;
            transaction.commit()?;
            current = 9;
        }
        if current == 9 {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "
                ALTER TABLE panes ADD COLUMN memory_high_bytes INTEGER;
                ALTER TABLE panes ADD COLUMN memory_swap_max_bytes INTEGER;
                ALTER TABLE panes ADD COLUMN cpu_quota_percent INTEGER;
                PRAGMA user_version = 10;
                ",
            )?;
            transaction.commit()?;
        }
        Ok(())
    }

    #[must_use]
    pub fn schema_version(&self) -> i64 {
        self.connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or_default()
    }

    pub fn upsert_session(&mut self, session: &SessionRecord) -> Result<(), StorageError> {
        let identity = session.identity.as_ref();
        let worker_identity = session.worker_identity.as_ref();
        self.connection.execute(
            "
            INSERT INTO sessions (
                id, owner, state, pid, pgid, start_token,
                worker_pid, worker_pgid, worker_start_token, updated_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO UPDATE SET
                owner = excluded.owner,
                state = excluded.state,
                pid = excluded.pid,
                pgid = excluded.pgid,
                start_token = excluded.start_token,
                worker_pid = excluded.worker_pid,
                worker_pgid = excluded.worker_pgid,
                worker_start_token = excluded.worker_start_token,
                updated_at_ms = excluded.updated_at_ms
            ",
            params![
                session.id,
                session.owner,
                session.state,
                identity.map(|value| value.pid),
                identity.map(|value| value.pgid),
                identity.map(|value| value.start_token.as_str()),
                worker_identity.map(|value| value.pid),
                worker_identity.map(|value| value.pgid),
                worker_identity.map(|value| value.start_token.as_str()),
                session.updated_at_ms
            ],
        )?;
        Ok(())
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner, state, pid, pgid, start_token,
                    worker_pid, worker_pgid, worker_start_token, updated_at_ms
             FROM sessions ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            let pid = row.get::<_, Option<u32>>(3)?;
            let pgid = row.get::<_, Option<u32>>(4)?;
            let start_token = row.get::<_, Option<String>>(5)?;
            let identity = match (pid, pgid, start_token) {
                (Some(pid), Some(pgid), Some(start_token)) => Some(ProcessIdentity {
                    pid,
                    pgid,
                    start_token,
                }),
                _ => None,
            };
            let worker_pid = row.get::<_, Option<u32>>(6)?;
            let worker_pgid = row.get::<_, Option<u32>>(7)?;
            let worker_start_token = row.get::<_, Option<String>>(8)?;
            let worker_identity = match (worker_pid, worker_pgid, worker_start_token) {
                (Some(pid), Some(pgid), Some(start_token)) => Some(ProcessIdentity {
                    pid,
                    pgid,
                    start_token,
                }),
                _ => None,
            };
            Ok(SessionRecord {
                id: row.get(0)?,
                owner: row.get(1)?,
                state: row.get(2)?,
                identity,
                worker_identity,
                updated_at_ms: row.get(9)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn find_session(&self, id: &str) -> Result<Option<SessionRecord>, StorageError> {
        Ok(self
            .list_sessions()?
            .into_iter()
            .find(|session| session.id == id))
    }

    pub fn get_attachment_authority(
        &self,
        session_id: &str,
    ) -> Result<Option<AttachmentAuthority>, StorageError> {
        self.connection
            .query_row(
                "SELECT state, owner, generation, reclaim_token_hash, deadline_ms
                 FROM attachment_authorities WHERE session_id = ?1",
                [session_id],
                |row| {
                    let state = AttachmentAuthorityState::from_db(&row.get::<_, String>(0)?);
                    let token_hash = row
                        .get::<_, Option<String>>(3)?
                        .map(qwe_core::ReclaimTokenHash::from_persisted);
                    Ok(AttachmentAuthority {
                        state,
                        owner: row.get(1)?,
                        generation: row.get::<_, i64>(2)? as u64,
                        reclaim_token_hash: token_hash,
                        deadline_ms: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn write_attachment_authority(
        &mut self,
        session_id: &str,
        authority: &AttachmentAuthority,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        write_attachment_authority(&self.connection, session_id, authority, now_ms)
    }

    pub fn enter_reclaim_grace(&mut self, now_ms: i64, grace_ms: i64) -> Result<u64, StorageError> {
        let changed = self.connection.execute(
            "UPDATE attachment_authorities
             SET state = 'reclaim_grace', deadline_ms = ?1, updated_at_ms = ?2
             WHERE state = 'active'",
            params![now_ms.saturating_add(grace_ms), now_ms],
        )?;
        Ok(changed as u64)
    }

    pub fn activate_attachment(
        &mut self,
        session_id: &str,
        authority: &AttachmentAuthority,
        release_hold: Option<(&str, u64)>,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        if authority.state != AttachmentAuthorityState::Active {
            return Err(StorageError::Invariant(
                "activated attachment authority must be active".into(),
            ));
        }
        let transaction = self.connection.transaction()?;
        if let Some((owner, generation)) = release_hold {
            let lease = transaction
                .query_row(
                    "SELECT session_id, owner, generation, deadline_ms,
                            updated_at_ms, released_at_ms
                     FROM leases WHERE session_id = ?1",
                    [session_id],
                    lease_from_row,
                )
                .optional()?
                .ok_or(LeaseError::NotFound)?;
            if lease.owner != owner || lease.generation != generation {
                return Err(LeaseError::NotOwner.into());
            }
            if lease.released_at_ms.is_some() || lease.deadline_ms <= now_ms {
                return Err(LeaseError::Expired.into());
            }
            if authority.generation != generation + 1 {
                return Err(StorageError::Invariant(
                    "attachment generation must consume one Lease generation".into(),
                ));
            }
            transaction.execute(
                "UPDATE leases SET owner = '', generation = ?2, deadline_ms = ?3,
                    updated_at_ms = ?3, released_at_ms = ?3
                 WHERE session_id = ?1",
                params![session_id, authority.generation as i64, now_ms],
            )?;
        }
        write_attachment_authority(&transaction, session_id, authority, now_ms)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn persist_attachment_hold(
        &mut self,
        session_id: &str,
        owner: &str,
        generation: u64,
        deadline_ms: i64,
        now_ms: i64,
    ) -> Result<AttachmentAuthority, StorageError> {
        let authority = AttachmentAuthority::held(owner, generation, deadline_ms);
        let transaction = self.connection.transaction()?;
        write_attachment_authority(&transaction, session_id, &authority, now_ms)?;
        transaction.execute(
            "
            INSERT INTO leases (
                session_id, owner, generation, deadline_ms, updated_at_ms,
                released_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, NULL)
            ON CONFLICT(session_id) DO UPDATE SET
                owner = excluded.owner,
                generation = excluded.generation,
                deadline_ms = excluded.deadline_ms,
                updated_at_ms = excluded.updated_at_ms,
                released_at_ms = NULL
            ",
            params![session_id, owner, generation as i64, deadline_ms, now_ms],
        )?;
        transaction.commit()?;
        Ok(authority)
    }

    pub fn insert_created_session(
        &mut self,
        session: &SessionRecord,
        pane: &PaneRecord,
        task: &TaskRecord,
    ) -> Result<(), StorageError> {
        if task.session_id != session.id {
            return Err(StorageError::Invariant(
                "created Task must belong to the created Session".into(),
            ));
        }
        if pane.session_id != session.id {
            return Err(StorageError::Invariant(
                "created Pane must belong to the created Session".into(),
            ));
        }
        let identity = session.identity.as_ref();
        let worker_identity = session.worker_identity.as_ref();
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "
            INSERT INTO sessions (
                id, owner, state, pid, pgid, start_token,
                worker_pid, worker_pgid, worker_start_token, updated_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ",
            params![
                session.id,
                session.owner,
                session.state,
                identity.map(|value| value.pid),
                identity.map(|value| value.pgid),
                identity.map(|value| value.start_token.as_str()),
                worker_identity.map(|value| value.pid),
                worker_identity.map(|value| value.pgid),
                worker_identity.map(|value| value.start_token.as_str()),
                session.updated_at_ms
            ],
        )?;
        transaction.execute(
            "
            INSERT INTO tasks (
                id, session_id, pid, pgid, start_token, state,
                exit_code, exit_signal, started_at_ms, ended_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ",
            params![
                task.id.to_string(),
                task.session_id,
                task.identity.pid,
                task.identity.pgid,
                task.identity.start_token,
                task.state.as_str(),
                task.exit_code,
                task.exit_signal,
                task.started_at_ms,
                task.ended_at_ms
            ],
        )?;
        transaction.execute(
            "
            INSERT INTO panes (
                id, session_id, cpu_seconds, memory_high_bytes, memory_bytes,
                memory_swap_max_bytes, process_limit, cpu_quota_percent, updated_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ",
            params![
                pane.id.to_string(),
                pane.session_id,
                sqlite_limit(pane.resource_policy.cpu_seconds),
                sqlite_limit(pane.resource_policy.memory_high_bytes),
                sqlite_limit(pane.resource_policy.memory_max_bytes),
                sqlite_limit(pane.resource_policy.memory_swap_max_bytes),
                sqlite_limit(pane.resource_policy.tasks_max),
                pane.resource_policy.cpu_quota_percent.map(i64::from),
                pane.updated_at_ms
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn find_pane_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<PaneRecord>, StorageError> {
        self.connection
            .query_row(
                "SELECT id, session_id, cpu_seconds, memory_high_bytes, memory_bytes,
                        memory_swap_max_bytes, process_limit, cpu_quota_percent,
                        updated_at_ms
                 FROM panes WHERE session_id = ?1",
                [session_id],
                pane_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn list_panes(&self) -> Result<Vec<PaneRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, session_id, cpu_seconds, memory_high_bytes, memory_bytes,
                    memory_swap_max_bytes, process_limit, cpu_quota_percent,
                    updated_at_ms
                 FROM panes ORDER BY session_id",
        )?;
        let rows = statement.query_map([], pane_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_pane_resource_policy(
        &mut self,
        session_id: &str,
        policy: &ResourcePolicy,
        updated_at_ms: i64,
    ) -> Result<PaneRecord, StorageError> {
        let changed = self.connection.execute(
            "
            UPDATE panes SET
                cpu_seconds = ?2,
                memory_high_bytes = ?3,
                memory_bytes = ?4,
                memory_swap_max_bytes = ?5,
                process_limit = ?6,
                cpu_quota_percent = ?7,
                updated_at_ms = ?8
            WHERE session_id = ?1
            ",
            params![
                session_id,
                sqlite_limit(policy.cpu_seconds),
                sqlite_limit(policy.memory_high_bytes),
                sqlite_limit(policy.memory_max_bytes),
                sqlite_limit(policy.memory_swap_max_bytes),
                sqlite_limit(policy.tasks_max),
                policy.cpu_quota_percent.map(i64::from),
                updated_at_ms
            ],
        )?;
        if changed == 0 {
            return Err(StorageError::Invariant(format!(
                "Pane for Session '{session_id}' does not exist"
            )));
        }
        self.find_pane_for_session(session_id)?
            .ok_or_else(|| StorageError::Invariant("Pane disappeared after update".into()))
    }

    pub fn mark_sessions_missing(
        &mut self,
        live_ids: &[String],
        now_ms: i64,
    ) -> Result<u64, StorageError> {
        let mut changed = 0;
        for mut session in self.list_sessions()? {
            if !live_ids.contains(&session.id)
                && matches!(session.state.as_str(), "running" | "exited")
            {
                session.state = "lost".into();
                session.updated_at_ms = now_ms;
                self.upsert_session(&session)?;
                changed += 1;
            }
        }
        Ok(changed)
    }

    pub fn insert_task(&mut self, task: &TaskRecord) -> Result<(), StorageError> {
        self.connection.execute(
            "
            INSERT INTO tasks (
                id, session_id, pid, pgid, start_token, state,
                exit_code, exit_signal, started_at_ms, ended_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ",
            params![
                task.id.to_string(),
                task.session_id,
                task.identity.pid,
                task.identity.pgid,
                task.identity.start_token,
                task.state.as_str(),
                task.exit_code,
                task.exit_signal,
                task.started_at_ms,
                task.ended_at_ms
            ],
        )?;
        Ok(())
    }

    pub fn find_task(&self, id: &TaskId) -> Result<Option<TaskRecord>, StorageError> {
        self.connection
            .query_row(
                "SELECT id, session_id, pid, pgid, start_token, state, exit_code,
                        exit_signal, started_at_ms, ended_at_ms
                 FROM tasks WHERE id = ?1",
                [id.to_string()],
                task_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn find_task_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<TaskRecord>, StorageError> {
        self.connection
            .query_row(
                "SELECT id, session_id, pid, pgid, start_token, state, exit_code,
                        exit_signal, started_at_ms, ended_at_ms
                 FROM tasks WHERE session_id = ?1
                 ORDER BY started_at_ms DESC LIMIT 1",
                [session_id],
                task_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn list_tasks(&self) -> Result<Vec<TaskRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, session_id, pid, pgid, start_token, state, exit_code,
                    exit_signal, started_at_ms, ended_at_ms
             FROM tasks ORDER BY started_at_ms, id",
        )?;
        let rows = statement.query_map([], task_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_task(
        &mut self,
        id: &TaskId,
        state: TaskState,
        exit_code: Option<i32>,
        exit_signal: Option<String>,
        ended_at_ms: Option<i64>,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE tasks SET state = ?2, exit_code = ?3, exit_signal = ?4,
                ended_at_ms = ?5 WHERE id = ?1",
            params![
                id.to_string(),
                state.as_str(),
                exit_code,
                exit_signal,
                ended_at_ms
            ],
        )?;
        Ok(())
    }

    pub fn get_lease(&self, session_id: &str) -> Result<Option<LeaseRecord>, StorageError> {
        self.connection
            .query_row(
                "SELECT session_id, owner, generation, deadline_ms, updated_at_ms,
                        released_at_ms
                 FROM leases WHERE session_id = ?1",
                [session_id],
                lease_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn find_idempotency(
        &self,
        key: &str,
        now_ms: i64,
    ) -> Result<Option<IdempotencyRecord>, StorageError> {
        self.connection
            .query_row(
                "SELECT idempotency_key, operation, request_fingerprint,
                        state, response_json, created_at_ms, expires_at_ms
                 FROM idempotency_records
                 WHERE idempotency_key = ?1 AND expires_at_ms > ?2",
                params![key, now_ms],
                |row| {
                    Ok(IdempotencyRecord {
                        key: row.get(0)?,
                        operation: row.get(1)?,
                        request_fingerprint: row.get(2)?,
                        state: row.get(3)?,
                        response_json: row.get(4)?,
                        created_at_ms: row.get(5)?,
                        expires_at_ms: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn record_idempotency(
        &mut self,
        key: &str,
        operation: &str,
        request_fingerprint: &str,
        response_json: &str,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "
            INSERT INTO idempotency_records (
                idempotency_key, operation, request_fingerprint,
                state, response_json, created_at_ms, expires_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ",
            params![
                key,
                operation,
                request_fingerprint,
                IDEMPOTENCY_COMPLETE,
                response_json,
                now_ms,
                now_ms.saturating_add(IDEMPOTENCY_RETENTION_MS)
            ],
        )?;
        Ok(())
    }

    pub fn reserve_idempotency(
        &mut self,
        key: &str,
        operation: &str,
        request_fingerprint: &str,
        now_ms: i64,
    ) -> Result<bool, StorageError> {
        let inserted = self.connection.execute(
            "
            INSERT INTO idempotency_records (
                idempotency_key, operation, request_fingerprint,
                state, response_json, created_at_ms, expires_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6)
            ON CONFLICT(idempotency_key) DO NOTHING
            ",
            params![
                key,
                operation,
                request_fingerprint,
                IDEMPOTENCY_PENDING,
                now_ms,
                now_ms.saturating_add(IDEMPOTENCY_RETENTION_MS)
            ],
        )?;
        Ok(inserted == 1)
    }

    pub fn complete_idempotency(
        &mut self,
        key: &str,
        response_json: &str,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE idempotency_records
             SET state = ?2, response_json = ?3
             WHERE idempotency_key = ?1",
            params![key, IDEMPOTENCY_COMPLETE, response_json],
        )?;
        Ok(())
    }

    pub fn delete_idempotency(&mut self, key: &str) -> Result<(), StorageError> {
        self.connection.execute(
            "DELETE FROM idempotency_records WHERE idempotency_key = ?1",
            [key],
        )?;
        Ok(())
    }

    pub fn prune_expired_idempotency(&mut self, now_ms: i64) -> Result<u64, StorageError> {
        let removed = self.connection.execute(
            "DELETE FROM idempotency_records WHERE expires_at_ms <= ?1",
            [now_ms],
        )?;
        Ok(removed as u64)
    }

    pub fn prune_terminal_history(
        &mut self,
        now_ms: i64,
        retention_ms: i64,
        max_terminal_sessions: i64,
    ) -> Result<HistoryPruneReport, StorageError> {
        if retention_ms <= 0 || max_terminal_sessions < 0 {
            return Err(StorageError::Invariant(
                "history retention and capacity must be non-negative".into(),
            ));
        }
        let cutoff_ms = now_ms.saturating_sub(retention_ms);
        let mut statement = self.connection.prepare(
            "
            SELECT id FROM sessions
            WHERE state IN ('closed', 'lost')
              AND updated_at_ms < ?1
              AND id NOT IN (
                  SELECT id FROM sessions
                  WHERE state IN ('closed', 'lost')
                  ORDER BY updated_at_ms DESC, id DESC
                  LIMIT ?2
              )
              AND NOT EXISTS (
                  SELECT 1 FROM attachment_authorities
                  WHERE attachment_authorities.session_id = sessions.id
              )
            ORDER BY updated_at_ms, id
            ",
        )?;
        let session_ids = statement
            .query_map(params![cutoff_ms, max_terminal_sessions], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let mut report = HistoryPruneReport::default();
        let transaction = self.connection.transaction()?;
        for session_id in session_ids {
            report.tasks += transaction
                .execute("DELETE FROM tasks WHERE session_id = ?1", [&session_id])?
                as u64;
            report.panes += transaction
                .execute("DELETE FROM panes WHERE session_id = ?1", [&session_id])?
                as u64;
            report.leases += transaction
                .execute("DELETE FROM leases WHERE session_id = ?1", [&session_id])?
                as u64;
            report.attachment_authorities += transaction.execute(
                "DELETE FROM attachment_authorities WHERE session_id = ?1",
                [&session_id],
            )? as u64;
            report.sessions +=
                transaction.execute("DELETE FROM sessions WHERE id = ?1", [&session_id])? as u64;
        }
        transaction.commit()?;
        Ok(report)
    }

    pub fn claim_lease(
        &mut self,
        session_id: &str,
        owner: &str,
        ttl_ms: i64,
        now_ms: i64,
    ) -> Result<LeaseRecord, StorageError> {
        let transaction = self.connection.transaction()?;
        let existing = transaction
            .query_row(
                "SELECT session_id, owner, generation, deadline_ms, updated_at_ms,
                        released_at_ms
                 FROM leases WHERE session_id = ?1",
                [session_id],
                lease_from_row,
            )
            .optional()?;
        let lease = match existing {
            None => LeaseRecord {
                session_id: session_id.into(),
                owner: owner.into(),
                generation: 1,
                deadline_ms: now_ms + ttl_ms,
                updated_at_ms: now_ms,
                released_at_ms: None,
            },
            Some(lease) if lease.released_at_ms.is_none() && lease.owner == owner => LeaseRecord {
                deadline_ms: now_ms.max(lease.updated_at_ms) + ttl_ms,
                updated_at_ms: now_ms,
                ..lease
            },
            Some(lease) if lease.released_at_ms.is_some() || lease.deadline_ms <= now_ms => {
                LeaseRecord {
                    session_id: session_id.into(),
                    owner: owner.into(),
                    generation: lease.generation + 1,
                    deadline_ms: now_ms.max(lease.updated_at_ms) + ttl_ms,
                    updated_at_ms: now_ms,
                    released_at_ms: None,
                }
            }
            Some(_) => return Err(LeaseError::AlreadyHeld.into()),
        };
        transaction.execute(
            "
            INSERT INTO leases (
                session_id, owner, generation, deadline_ms, updated_at_ms, released_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(session_id) DO UPDATE SET
                owner = excluded.owner,
                generation = excluded.generation,
                deadline_ms = excluded.deadline_ms,
                updated_at_ms = excluded.updated_at_ms,
                released_at_ms = excluded.released_at_ms
            ",
            params![
                lease.session_id,
                lease.owner,
                lease.generation as i64,
                lease.deadline_ms,
                lease.updated_at_ms,
                lease.released_at_ms
            ],
        )?;
        transaction.commit()?;
        Ok(lease)
    }

    pub fn touch_lease(
        &mut self,
        session_id: &str,
        owner: &str,
        generation: u64,
        ttl_ms: i64,
        now_ms: i64,
    ) -> Result<LeaseRecord, StorageError> {
        let mut lease = self.get_lease(session_id)?.ok_or(LeaseError::NotFound)?;
        if lease.owner != owner || lease.generation != generation {
            return Err(LeaseError::NotOwner.into());
        }
        if lease.released_at_ms.is_some() {
            return Err(LeaseError::Expired.into());
        }
        if lease.deadline_ms <= now_ms {
            return Err(LeaseError::Expired.into());
        }
        lease.deadline_ms = now_ms.max(lease.updated_at_ms) + ttl_ms;
        lease.updated_at_ms = now_ms;
        self.write_lease(&lease)?;
        Ok(lease)
    }

    pub fn release_lease(
        &mut self,
        session_id: &str,
        owner: &str,
        generation: u64,
        now_ms: i64,
    ) -> Result<LeaseRecord, StorageError> {
        let mut lease = self.get_lease(session_id)?.ok_or(LeaseError::NotFound)?;
        if lease.owner != owner || lease.generation != generation {
            return Err(LeaseError::NotOwner.into());
        }
        lease.generation += 1;
        lease.owner.clear();
        lease.deadline_ms = now_ms;
        lease.updated_at_ms = now_ms;
        lease.released_at_ms = Some(now_ms);
        self.write_lease(&lease)?;
        Ok(lease)
    }

    pub fn takeover_lease(
        &mut self,
        session_id: &str,
        owner: &str,
        ttl_ms: i64,
        now_ms: i64,
    ) -> Result<LeaseRecord, StorageError> {
        let lease = self.get_lease(session_id)?.ok_or(LeaseError::NotFound)?;
        if lease.released_at_ms.is_none() && lease.deadline_ms > now_ms {
            return Err(LeaseError::AlreadyHeld.into());
        }
        self.claim_lease(session_id, owner, ttl_ms, now_ms)
    }

    fn write_lease(&mut self, lease: &LeaseRecord) -> Result<(), StorageError> {
        self.connection.execute(
            "
            UPDATE leases SET owner = ?2, generation = ?3, deadline_ms = ?4,
                updated_at_ms = ?5, released_at_ms = ?6
            WHERE session_id = ?1
            ",
            params![
                lease.session_id,
                lease.owner,
                lease.generation as i64,
                lease.deadline_ms,
                lease.updated_at_ms,
                lease.released_at_ms
            ],
        )?;
        Ok(())
    }
}

fn write_attachment_authority(
    connection: &Connection,
    session_id: &str,
    authority: &AttachmentAuthority,
    now_ms: i64,
) -> Result<(), StorageError> {
    if authority.state == AttachmentAuthorityState::Unattached {
        connection.execute(
            "DELETE FROM attachment_authorities WHERE session_id = ?1",
            [session_id],
        )?;
        return Ok(());
    }
    connection.execute(
        "
        INSERT INTO attachment_authorities (
            session_id, state, owner, generation, reclaim_token_hash,
            deadline_ms, updated_at_ms
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(session_id) DO UPDATE SET
            state = excluded.state,
            owner = excluded.owner,
            generation = excluded.generation,
            reclaim_token_hash = excluded.reclaim_token_hash,
            deadline_ms = excluded.deadline_ms,
            updated_at_ms = excluded.updated_at_ms
        ",
        params![
            session_id,
            authority.state.as_str(),
            authority.owner,
            authority.generation as i64,
            authority
                .reclaim_token_hash
                .as_ref()
                .map(qwe_core::ReclaimTokenHash::as_str),
            authority.deadline_ms,
            now_ms
        ],
    )?;
    Ok(())
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<TaskRecord> {
    let id: String = row.get(0)?;
    Ok(TaskRecord {
        id: TaskId::from_str(&id).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        session_id: row.get(1)?,
        identity: ProcessIdentity {
            pid: row.get(2)?,
            pgid: row.get(3)?,
            start_token: row.get(4)?,
        },
        state: TaskState::from_db(&row.get::<_, String>(5)?),
        exit_code: row.get(6)?,
        exit_signal: row.get(7)?,
        started_at_ms: row.get(8)?,
        ended_at_ms: row.get(9)?,
    })
}

fn pane_from_row(row: &Row<'_>) -> rusqlite::Result<PaneRecord> {
    let id: String = row.get(0)?;
    Ok(PaneRecord {
        id: PaneId::from_str(&id).map_err(|failure| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(failure),
            )
        })?,
        session_id: row.get(1)?,
        resource_policy: ResourcePolicy {
            cpu_seconds: resource_limit_from_row(row, 2)?,
            memory_high_bytes: resource_limit_from_row(row, 3)?,
            memory_max_bytes: resource_limit_from_row(row, 4)?,
            memory_swap_max_bytes: resource_limit_from_row(row, 5)?,
            tasks_max: resource_limit_from_row(row, 6)?,
            cpu_quota_percent: resource_limit_from_row(row, 7)?
                .map(|value| {
                    u32::try_from(value).map_err(|failure| {
                        rusqlite::Error::FromSqlConversionFailure(
                            7,
                            rusqlite::types::Type::Integer,
                            Box::new(failure),
                        )
                    })
                })
                .transpose()?,
        },
        updated_at_ms: row.get(8)?,
    })
}

fn sqlite_limit(value: Option<u64>) -> Option<i64> {
    value.and_then(|value| i64::try_from(value).ok())
}

fn resource_limit_from_row(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<u64>> {
    row.get::<_, Option<i64>>(index)?
        .map(|value| {
            u64::try_from(value).map_err(|failure| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Integer,
                    Box::new(failure),
                )
            })
        })
        .transpose()
}

fn lease_from_row(row: &Row<'_>) -> rusqlite::Result<LeaseRecord> {
    Ok(LeaseRecord {
        session_id: row.get(0)?,
        owner: row.get(1)?,
        generation: row.get::<_, i64>(2)? as u64,
        deadline_ms: row.get(3)?,
        updated_at_ms: row.get(4)?,
        released_at_ms: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: TaskId, session_id: &str) -> TaskRecord {
        TaskRecord {
            id,
            session_id: session_id.into(),
            identity: ProcessIdentity {
                pid: 100,
                pgid: 100,
                start_token: "boot:42".into(),
            },
            state: TaskState::Running,
            exit_code: None,
            exit_signal: None,
            started_at_ms: 10,
            ended_at_ms: None,
        }
    }

    fn pane(session_id: &str) -> PaneRecord {
        PaneRecord {
            id: PaneId::new(),
            session_id: session_id.into(),
            resource_policy: ResourcePolicy::empty(),
            updated_at_ms: 10,
        }
    }

    fn session(id: &str) -> SessionRecord {
        SessionRecord {
            id: id.into(),
            owner: "local".into(),
            state: "running".into(),
            identity: None,
            worker_identity: None,
            updated_at_ms: 1,
        }
    }

    #[test]
    fn creates_versioned_schema() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.schema_version(), SCHEMA_VERSION);
    }

    #[test]
    fn persists_and_updates_tasks() {
        let mut store = Store::open_in_memory().unwrap();
        let id = TaskId::new();
        store.upsert_session(&session("session-1")).unwrap();
        store.insert_task(&task(id.clone(), "session-1")).unwrap();
        let stored = store.find_task(&id).unwrap().unwrap();
        assert_eq!(stored.identity.pid, 100);
        assert_eq!(stored.state, TaskState::Running);
        store
            .update_task(&id, TaskState::Exited, Some(7), None, Some(20))
            .unwrap();
        let stored = store.find_task(&id).unwrap().unwrap();
        assert_eq!(stored.state, TaskState::Exited);
        assert_eq!(stored.exit_code, Some(7));
    }

    #[test]
    fn created_session_and_task_commit_atomically() {
        let mut store = Store::open_in_memory().unwrap();
        let existing_id = TaskId::new();
        store.upsert_session(&session("existing")).unwrap();
        store
            .insert_task(&task(existing_id.clone(), "existing"))
            .unwrap();
        let session = SessionRecord {
            id: "new".into(),
            owner: "local".into(),
            state: "running".into(),
            identity: Some(ProcessIdentity {
                pid: 200,
                pgid: 200,
                start_token: "boot:200".into(),
            }),
            worker_identity: None,
            updated_at_ms: 10,
        };
        let mut conflicting_task = task(existing_id, "new");
        conflicting_task.identity.pid = 200;
        let created_pane = pane("new");
        assert!(
            store
                .insert_created_session(&session, &created_pane, &conflicting_task)
                .is_err()
        );
        assert!(store.find_session("new").unwrap().is_none());
    }

    #[test]
    fn created_session_has_one_pane_and_one_task_instance() {
        let mut store = Store::open_in_memory().unwrap();
        let session = SessionRecord {
            id: "one".into(),
            owner: "local".into(),
            state: "running".into(),
            identity: Some(ProcessIdentity {
                pid: 300,
                pgid: 300,
                start_token: "boot:300".into(),
            }),
            worker_identity: Some(ProcessIdentity {
                pid: 290,
                pgid: 290,
                start_token: "worker:290".into(),
            }),
            updated_at_ms: 10,
        };
        let mut created_pane = pane("one");
        created_pane.resource_policy = ResourcePolicy {
            memory_high_bytes: Some(64 * 1024 * 1024),
            memory_max_bytes: Some(128 * 1024 * 1024),
            memory_swap_max_bytes: Some(0),
            tasks_max: Some(16),
            cpu_quota_percent: Some(150),
            cpu_seconds: Some(5),
        };
        let created_task = task(TaskId::new(), "one");
        store
            .insert_created_session(&session, &created_pane, &created_task)
            .unwrap();
        let stored_pane = store.find_pane_for_session("one").unwrap().unwrap();
        assert_eq!(stored_pane.id, created_pane.id);
        assert_eq!(stored_pane.resource_policy.cpu_seconds, Some(5));
        assert_eq!(
            stored_pane.resource_policy.memory_high_bytes,
            Some(64 * 1024 * 1024)
        );
        assert_eq!(stored_pane.resource_policy.tasks_max, Some(16));
        assert_eq!(stored_pane.resource_policy.cpu_quota_percent, Some(150));
        let updated = store
            .update_pane_resource_policy(
                "one",
                &ResourcePolicy {
                    memory_max_bytes: Some(256 * 1024 * 1024),
                    tasks_max: Some(32),
                    ..ResourcePolicy::empty()
                },
                20,
            )
            .unwrap();
        assert_eq!(updated.resource_policy.memory_max_bytes, Some(256 << 20));
        assert_eq!(updated.resource_policy.tasks_max, Some(32));
        assert_eq!(updated.updated_at_ms, 20);
        assert_eq!(
            store.find_task_for_session("one").unwrap().unwrap().id,
            created_task.id
        );
        assert!(
            store.insert_task(&task(TaskId::new(), "one")).is_err(),
            "a second TaskInstance for one Session must be rejected"
        );
        assert!(
            store
                .insert_task(&task(TaskId::new(), "missing-session"))
                .is_err(),
            "TaskInstance must reference an existing Session"
        );
    }

    #[test]
    fn lease_claim_touch_release_and_takeover() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_session(&session("s1")).unwrap();
        let claimed = store.claim_lease("s1", "alice", 100, 1_000).unwrap();
        assert_eq!(claimed.generation, 1);
        assert_eq!(
            store
                .claim_lease("s1", "bob", 100, 1_001)
                .unwrap_err()
                .to_string(),
            LeaseError::AlreadyHeld.to_string()
        );
        let touched = store.touch_lease("s1", "alice", 1, 100, 1_050).unwrap();
        assert_eq!(touched.deadline_ms, 1_150);
        assert_eq!(
            store
                .claim_lease("s1", "bob", 100, 1_151)
                .unwrap()
                .generation,
            2
        );
        let released = store.release_lease("s1", "bob", 2, 1_200).unwrap();
        assert_eq!(released.generation, 3);
        assert_eq!(released.released_at_ms, Some(1_200));
        assert_eq!(
            store
                .claim_lease("s1", "carol", 100, 1_201)
                .unwrap()
                .generation,
            4
        );
    }

    #[test]
    fn lease_renewal_does_not_shorten_deadlines_when_the_wall_clock_moves_back() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_session(&session("clock")).unwrap();
        let claimed = store.claim_lease("clock", "alice", 1_000, 10_000).unwrap();
        assert_eq!(claimed.deadline_ms, 11_000);

        let renewed = store.claim_lease("clock", "alice", 1_000, 9_000).unwrap();
        assert_eq!(renewed.deadline_ms, 11_000);
        assert_eq!(renewed.updated_at_ms, 9_000);

        let touched = store
            .touch_lease("clock", "alice", 1, 1_000, 8_500)
            .unwrap();
        assert_eq!(touched.deadline_ms, 10_000);
        assert_eq!(touched.generation, 1);
    }

    #[test]
    fn persisted_active_attachment_enters_reclaim_grace() {
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_session(&session("authority")).unwrap();
        let token_hash = qwe_core::ReclaimTokenHash::from_token("reclaim-token");
        let authority = AttachmentAuthority::active("alice", token_hash.clone());
        store
            .write_attachment_authority("authority", &authority, 1_000)
            .unwrap();

        assert_eq!(
            store
                .get_attachment_authority("authority")
                .unwrap()
                .unwrap(),
            authority
        );
        assert_eq!(store.enter_reclaim_grace(2_000, 5_000).unwrap(), 1);
        let recovered = store
            .get_attachment_authority("authority")
            .unwrap()
            .unwrap();
        assert_eq!(recovered.state, AttachmentAuthorityState::ReclaimGrace);
        assert_eq!(recovered.deadline_ms, Some(7_000));
        assert_eq!(recovered.reclaim_token_hash, Some(token_hash));

        store
            .write_attachment_authority("authority", &AttachmentAuthority::unattached(), 3_000)
            .unwrap();
        assert!(
            store
                .get_attachment_authority("authority")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn idempotency_records_replay_until_the_retention_deadline() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .record_idempotency(
                "key-1",
                "SESSION_CREATE",
                "fingerprint",
                r#"{"result":{"session_id":"s1"}}"#,
                10_000,
            )
            .unwrap();
        let record = store.find_idempotency("key-1", 10_001).unwrap().unwrap();
        assert_eq!(record.operation, "SESSION_CREATE");
        assert_eq!(record.request_fingerprint, "fingerprint");
        assert_eq!(record.expires_at_ms, 10_000 + IDEMPOTENCY_RETENTION_MS);

        assert!(
            store
                .find_idempotency("key-1", record.expires_at_ms)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .prune_expired_idempotency(record.expires_at_ms)
                .unwrap(),
            1
        );
    }

    #[test]
    fn idempotency_reservation_survives_until_completion() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(
            store
                .reserve_idempotency("pending-key", "SESSION_CREATE", "fingerprint", 10_000,)
                .unwrap()
        );
        assert!(
            !store
                .reserve_idempotency("pending-key", "SESSION_CREATE", "fingerprint", 10_001,)
                .unwrap()
        );
        let pending = store
            .find_idempotency("pending-key", 10_002)
            .unwrap()
            .unwrap();
        assert_eq!(pending.state, IDEMPOTENCY_PENDING);
        assert!(pending.response_json.is_none());

        store
            .complete_idempotency("pending-key", r#"{"Ok":{"session_id":"s1"}}"#)
            .unwrap();
        let completed = store
            .find_idempotency("pending-key", 10_003)
            .unwrap()
            .unwrap();
        assert_eq!(completed.state, IDEMPOTENCY_COMPLETE);
        assert_eq!(
            completed.response_json.as_deref(),
            Some(r#"{"Ok":{"session_id":"s1"}}"#)
        );
    }

    #[test]
    fn migrates_version_one_database() {
        let path = std::env::temp_dir().join(format!(
            "qwe-storage-migration-{}-{}.sqlite",
            std::process::id(),
            TaskId::new()
        ));
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "
                    CREATE TABLE sessions (
                        id TEXT PRIMARY KEY,
                        owner TEXT,
                        state TEXT NOT NULL
                    );
                    CREATE TABLE tasks (
                        id TEXT PRIMARY KEY,
                        session_id TEXT,
                        pid INTEGER NOT NULL,
                        pgid INTEGER NOT NULL,
                        start_token TEXT NOT NULL,
                        state TEXT NOT NULL,
                        exit_code INTEGER
                    );
                    PRAGMA user_version = 1;
                    ",
                )
                .unwrap();
        }
        let mut store = Store::open(path.to_str().unwrap()).unwrap();
        assert_eq!(store.schema_version(), SCHEMA_VERSION);
        store.upsert_session(&session("migrated")).unwrap();
        assert_eq!(
            store
                .claim_lease("migrated", "owner", 100, 1_000)
                .unwrap()
                .generation,
            1
        );
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    }

    #[test]
    fn reconciliation_marks_missing_sessions_lost() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .upsert_session(&SessionRecord {
                id: "missing".into(),
                owner: "local".into(),
                state: "running".into(),
                identity: None,
                worker_identity: None,
                updated_at_ms: 1,
            })
            .unwrap();
        assert_eq!(store.mark_sessions_missing(&[], 2).unwrap(), 1);
        assert_eq!(store.list_sessions().unwrap()[0].state, "lost");
    }

    #[test]
    fn reconciliation_marks_exited_sessions_lost_when_worker_is_absent() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .upsert_session(&SessionRecord {
                id: "exited".into(),
                owner: "local".into(),
                state: "exited".into(),
                identity: None,
                worker_identity: None,
                updated_at_ms: 1,
            })
            .unwrap();
        assert_eq!(store.mark_sessions_missing(&[], 2).unwrap(), 1);
        assert_eq!(store.list_sessions().unwrap()[0].state, "lost");
    }

    #[test]
    fn history_pruning_requires_both_age_and_terminal_capacity_pressure() {
        let mut store = Store::open_in_memory().unwrap();
        for (id, state, updated_at_ms) in [
            ("old-outside-capacity", "closed", 100),
            ("newer-retained", "closed", 200),
            ("newest-retained", "lost", 300),
            ("running-retained", "running", 50),
        ] {
            let mut record = session(id);
            record.state = state.into();
            record.updated_at_ms = updated_at_ms;
            store.upsert_session(&record).unwrap();
        }
        let pruned_id = TaskId::new();
        store
            .insert_task(&task(pruned_id.clone(), "old-outside-capacity"))
            .unwrap();

        let report = store.prune_terminal_history(10_000, 1_000, 2).unwrap();
        assert_eq!(report.sessions, 1);
        assert_eq!(report.tasks, 1);
        assert!(
            store
                .find_session("old-outside-capacity")
                .unwrap()
                .is_none()
        );
        assert!(store.find_task(&pruned_id).unwrap().is_none());
        assert!(store.find_session("newer-retained").unwrap().is_some());
        assert!(store.find_session("newest-retained").unwrap().is_some());
        assert!(store.find_session("running-retained").unwrap().is_some());
    }

    #[test]
    fn history_pruning_preserves_sessions_with_attachment_authority() {
        let mut store = Store::open_in_memory().unwrap();
        let mut closed = session("authority-protected");
        closed.state = "closed".into();
        closed.updated_at_ms = 1;
        store.upsert_session(&closed).unwrap();
        let authority =
            AttachmentAuthority::active("owner", qwe_core::ReclaimTokenHash::from_token("token"));
        store
            .write_attachment_authority("authority-protected", &authority, 2)
            .unwrap();

        let report = store.prune_terminal_history(10_000, 1_000, 0).unwrap();
        assert_eq!(report.sessions, 0);
        assert!(store.find_session("authority-protected").unwrap().is_some());
    }
}
