use qwe_core::{
    AttachmentAuthority, AttachmentAuthorityState, LeaseRecord, PaneId, PaneRecord,
    PaneWorkerRecord, PrincipalId, PrincipalRecord, ProcessIdentity, ResourceBackend,
    ResourcePolicy, ResourceScopeId, ResourceScopeRecord, ResourceScopeState,
    ResourceScopeTargetKind, SessionRecord, TaskId, TaskRecord, TaskState,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::json;
use std::str::FromStr;
use thiserror::Error;

pub const SCHEMA_VERSION: i64 = 13;
pub const IDEMPOTENCY_RETENTION_MS: i64 = 24 * 60 * 60 * 1_000;
pub const HISTORY_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
pub const HISTORY_MAX_TERMINAL_SESSIONS: i64 = 10_000;
pub const IDEMPOTENCY_PENDING: &str = "pending";
pub const IDEMPOTENCY_COMPLETE: &str = "complete";

const V11_SCHEMA_SQL: &str = "
    CREATE TABLE sessions (
        id TEXT PRIMARY KEY,
        owner TEXT NOT NULL,
        state TEXT NOT NULL,
        pid INTEGER,
        pgid INTEGER,
        start_token TEXT,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE TABLE panes (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL
            REFERENCES sessions(id) ON DELETE RESTRICT,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE INDEX panes_by_session ON panes(session_id);
    CREATE TABLE pane_workers (
        pane_id TEXT PRIMARY KEY
            REFERENCES panes(id) ON DELETE RESTRICT,
        pid INTEGER NOT NULL,
        pgid INTEGER NOT NULL,
        start_token TEXT NOT NULL,
        state TEXT NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE TABLE tasks (
        id TEXT PRIMARY KEY,
        pane_id TEXT NOT NULL
            REFERENCES panes(id) ON DELETE RESTRICT,
        pid INTEGER NOT NULL,
        pgid INTEGER NOT NULL,
        start_token TEXT NOT NULL,
        state TEXT NOT NULL,
        exit_code INTEGER,
        exit_signal TEXT,
        started_at_ms INTEGER NOT NULL,
        ended_at_ms INTEGER
    );
    CREATE INDEX tasks_by_pane ON tasks(pane_id);
    CREATE TABLE resource_scopes (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL
            REFERENCES sessions(id) ON DELETE RESTRICT,
        target_kind TEXT NOT NULL,
        target_id TEXT NOT NULL,
        parent_scope_id TEXT
            REFERENCES resource_scopes(id) ON DELETE RESTRICT,
        backend TEXT NOT NULL,
        requested_policy_json TEXT NOT NULL,
        lifecycle_state TEXT NOT NULL,
        cleanup_outcome TEXT,
        updated_at_ms INTEGER NOT NULL,
        UNIQUE(session_id, target_kind, target_id)
    );
    CREATE INDEX resource_scopes_by_session
        ON resource_scopes(session_id, target_kind, target_id);
    CREATE TABLE principals (
        id TEXT PRIMARY KEY,
        parent_id TEXT
            REFERENCES principals(id) ON DELETE RESTRICT,
        epoch INTEGER NOT NULL,
        created_at_ms INTEGER NOT NULL,
        revoked_at_ms INTEGER
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
        pane_id TEXT PRIMARY KEY
            REFERENCES panes(id) ON DELETE RESTRICT,
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
";

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
    pub pane_workers: u64,
    pub resource_scopes: u64,
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
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > 0 && version < SCHEMA_VERSION {
            backup_before_migration(&connection, path, version)?;
        }
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
            transaction.execute_batch(V11_SCHEMA_SQL)?;
            transaction.execute(
                "INSERT INTO principals (id, parent_id, epoch, created_at_ms, revoked_at_ms)
                 VALUES (?1, NULL, 1, ?2, NULL)",
                params![PrincipalId::local_root().to_string(), unix_ms()],
            )?;
            transaction.execute_batch("PRAGMA user_version = 11;")?;
            transaction.commit()?;
            current = 11;
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
            current = 10;
        }
        if current == 10 {
            self.migrate_v10_to_v11()?;
            current = 11;
        }
        if current == 11 {
            self.migrate_v11_to_v12()?;
            current = 12;
        }
        if current == 12 {
            self.migrate_v12_to_v13()?;
        }
        Ok(())
    }

    fn migrate_v10_to_v11(&mut self) -> Result<(), StorageError> {
        migrate_v10_to_v11(self)
    }

    fn migrate_v11_to_v12(&mut self) -> Result<(), StorageError> {
        migrate_v11_to_v12(self)
    }

    fn migrate_v12_to_v13(&mut self) -> Result<(), StorageError> {
        migrate_v12_to_v13(self)
    }

    #[must_use]
    pub fn schema_version(&self) -> i64 {
        self.connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or_default()
    }

    pub fn upsert_session(&mut self, session: &SessionRecord) -> Result<(), StorageError> {
        let identity = session.identity.as_ref();
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "
            INSERT INTO sessions (
                id, owner, state, pid, pgid, start_token,
                updated_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(id) DO UPDATE SET
                owner = excluded.owner,
                state = excluded.state,
                pid = excluded.pid,
                pgid = excluded.pgid,
                start_token = excluded.start_token,
                updated_at_ms = excluded.updated_at_ms
            ",
            params![
                session.id,
                session.owner,
                session.state,
                identity.map(|value| value.pid),
                identity.map(|value| value.pgid),
                identity.map(|value| value.start_token.as_str()),
                session.updated_at_ms
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner, state, pid, pgid, start_token, updated_at_ms
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
            Ok(SessionRecord {
                id: row.get(0)?,
                owner: row.get(1)?,
                state: row.get(2)?,
                identity,
                worker_identity: None,
                updated_at_ms: row.get(6)?,
            })
        })?;
        let mut sessions = rows.collect::<Result<Vec<_>, _>>()?;
        for session in &mut sessions {
            session.worker_identity = self.worker_identity_for_session(&session.id)?;
        }
        Ok(sessions)
    }

    pub fn find_session(&self, id: &str) -> Result<Option<SessionRecord>, StorageError> {
        Ok(self
            .list_sessions()?
            .into_iter()
            .find(|session| session.id == id))
    }

    fn worker_identity_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<ProcessIdentity>, StorageError> {
        self.connection
            .query_row(
                "SELECT pw.pid, pw.pgid, pw.start_token
                 FROM panes p
                 JOIN pane_workers pw ON pw.pane_id = p.id
                 WHERE p.session_id = ?1
                 ORDER BY p.id LIMIT 1",
                [session_id],
                |row| {
                    Ok(ProcessIdentity {
                        pid: row.get(0)?,
                        pgid: row.get(1)?,
                        start_token: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    fn default_pane_id(&self, session_id: &str) -> Result<Option<PaneId>, StorageError> {
        let focused: Option<String> = self
            .connection
            .query_row(
                "SELECT sl.focused_pane_id
                 FROM session_layouts sl
                 JOIN pane_workers pw ON pw.pane_id = sl.focused_pane_id
                 WHERE sl.session_id = ?1
                   AND sl.focused_pane_id IS NOT NULL
                   AND pw.state IN ('active', 'exited')",
                [session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::from)?;
        if let Some(value) = focused {
            return PaneId::from_str(&value)
                .map(Some)
                .map_err(|error| StorageError::Invariant(error.to_string()));
        }
        let value = self
            .connection
            .query_row(
                "SELECT id FROM panes
                 WHERE session_id = ?1
                   AND EXISTS (
                       SELECT 1 FROM pane_workers pw
                       WHERE pw.pane_id = panes.id AND pw.state IN ('active', 'exited')
                   )
                 ORDER BY layout_order, id LIMIT 1",
                [session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::from)?;
        let value = if let Some(value) = value {
            Some(value)
        } else {
            let fallback_focus: Option<String> = self
                .connection
                .query_row(
                    "SELECT focused_pane_id
                     FROM session_layouts
                     WHERE session_id = ?1 AND focused_pane_id IS NOT NULL",
                    [session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(StorageError::from)?;
            match fallback_focus {
                Some(value) => Some(value),
                None => self
                    .connection
                    .query_row(
                        "SELECT id FROM panes
                         WHERE session_id = ?1
                         ORDER BY layout_order, id LIMIT 1",
                        [session_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(StorageError::from)?,
            }
        };
        value
            .map(|value: String| {
                PaneId::from_str(&value).map_err(|error| StorageError::Invariant(error.to_string()))
            })
            .transpose()
    }

    pub fn get_attachment_authority(
        &self,
        session_id: &str,
    ) -> Result<Option<AttachmentAuthority>, StorageError> {
        let Some(pane_id) = self.default_pane_id(session_id)? else {
            return Ok(None);
        };
        self.get_attachment_authority_for_pane(&pane_id)
    }

    pub fn get_attachment_authority_for_pane(
        &self,
        pane_id: &PaneId,
    ) -> Result<Option<AttachmentAuthority>, StorageError> {
        self.connection
            .query_row(
                "SELECT state, owner, generation, reclaim_token_hash, deadline_ms
                 FROM attachment_authorities WHERE pane_id = ?1",
                [pane_id.to_string()],
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
        let pane_id = self.default_pane_id(session_id)?.ok_or_else(|| {
            StorageError::Invariant(format!("Session '{session_id}' has no Pane"))
        })?;
        self.write_attachment_authority_for_pane(&pane_id, authority, now_ms)
    }

    pub fn write_attachment_authority_for_pane(
        &mut self,
        pane_id: &PaneId,
        authority: &AttachmentAuthority,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        write_attachment_authority(&self.connection, pane_id, authority, now_ms)
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
        let pane_id = self.default_pane_id(session_id)?.ok_or_else(|| {
            StorageError::Invariant(format!("Session '{session_id}' has no Pane"))
        })?;
        self.activate_attachment_for_pane(session_id, &pane_id, authority, release_hold, now_ms)
    }

    pub fn activate_attachment_for_pane(
        &mut self,
        session_id: &str,
        pane_id: &PaneId,
        authority: &AttachmentAuthority,
        release_hold: Option<(&str, u64)>,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        if authority.state != AttachmentAuthorityState::Active {
            return Err(StorageError::Invariant(
                "activated attachment authority must be active".into(),
            ));
        }
        let pane_session: Option<String> = self
            .connection
            .query_row(
                "SELECT session_id FROM panes WHERE id = ?1",
                [pane_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if pane_session.as_deref() != Some(session_id) {
            return Err(StorageError::Invariant(
                "attachment Pane does not belong to the requested Session".into(),
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
        write_attachment_authority(&transaction, pane_id, authority, now_ms)?;
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
        let pane_id = self.default_pane_id(session_id)?.ok_or_else(|| {
            StorageError::Invariant(format!("Session '{session_id}' has no Pane"))
        })?;
        self.persist_attachment_hold_for_pane(
            &pane_id,
            session_id,
            owner,
            generation,
            deadline_ms,
            now_ms,
        )
    }

    pub fn persist_attachment_hold_for_pane(
        &mut self,
        pane_id: &PaneId,
        session_id: &str,
        owner: &str,
        generation: u64,
        deadline_ms: i64,
        now_ms: i64,
    ) -> Result<AttachmentAuthority, StorageError> {
        let authority = AttachmentAuthority::held(owner, generation, deadline_ms);
        let pane_session: Option<String> = self
            .connection
            .query_row(
                "SELECT session_id FROM panes WHERE id = ?1",
                [pane_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if pane_session.as_deref() != Some(session_id) {
            return Err(StorageError::Invariant(
                "attachment Pane does not belong to the requested Session".into(),
            ));
        }
        let transaction = self.connection.transaction()?;
        write_attachment_authority(&transaction, pane_id, &authority, now_ms)?;
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
        validate_pane_layout(pane)?;
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
        if task.pane_id != pane.id {
            return Err(StorageError::Invariant(
                "created Task must belong to the created Pane".into(),
            ));
        }
        let identity = session.identity.as_ref();
        let worker_identity = session.worker_identity.as_ref();
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO principals (
                id, parent_id, epoch, created_at_ms, revoked_at_ms
             ) VALUES (?1, NULL, 1, ?2, NULL)",
            params![PrincipalId::local_root().to_string(), unix_ms()],
        )?;
        transaction.execute(
            "
            INSERT INTO sessions (
                id, owner, state, pid, pgid, start_token,
                updated_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ",
            params![
                session.id,
                session.owner,
                session.state,
                identity.map(|value| value.pid),
                identity.map(|value| value.pgid),
                identity.map(|value| value.start_token.as_str()),
                session.updated_at_ms
            ],
        )?;
        transaction.execute(
            "INSERT INTO panes (
                id, session_id, kind, layout_order, layout_weight, z_index,
                floating_x, floating_y, floating_rows, floating_columns, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                pane.id.to_string(),
                pane.session_id,
                pane.kind.as_str(),
                i64::from(pane.layout_order),
                i64::from(pane.layout_weight),
                i64::from(pane.z_index),
                pane.floating_rect.map(|rect| i64::from(rect.x)),
                pane.floating_rect.map(|rect| i64::from(rect.y)),
                pane.floating_rect.map(|rect| i64::from(rect.rows)),
                pane.floating_rect.map(|rect| i64::from(rect.columns)),
                pane.updated_at_ms
            ],
        )?;
        transaction.execute(
            "INSERT INTO session_layouts (session_id, focused_pane_id, updated_at_ms)
             VALUES (?1, ?2, ?3)",
            params![pane.session_id, pane.id.to_string(), pane.updated_at_ms],
        )?;
        let session_scope_id = ResourceScopeId::new();
        transaction.execute(
            "INSERT INTO resource_scopes (
                id, session_id, target_kind, target_id, parent_scope_id, backend,
                requested_policy_json, lifecycle_state, cleanup_outcome, updated_at_ms
             ) VALUES (?1, ?2, 'session', ?2, NULL, 'logical', '{}', 'active', NULL, ?3)",
            params![
                session_scope_id.to_string(),
                pane.session_id,
                pane.updated_at_ms
            ],
        )?;
        let backend = if cfg!(target_os = "linux") {
            ResourceBackend::LinuxCgroupV2.as_str()
        } else if cfg!(target_os = "macos") {
            ResourceBackend::MacosMonitored.as_str()
        } else {
            ResourceBackend::Logical.as_str()
        };
        let scope_id = pane.resource_scope_id.clone();
        transaction.execute(
            "INSERT INTO resource_scopes (
                id, session_id, target_kind, target_id, parent_scope_id, backend,
                requested_policy_json, lifecycle_state, cleanup_outcome, updated_at_ms
             ) VALUES (?1, ?2, 'pane', ?3, ?4, ?5, ?6, 'active', NULL, ?7)",
            params![
                scope_id.to_string(),
                pane.session_id,
                pane.id.to_string(),
                session_scope_id.to_string(),
                backend,
                serde_json::to_string(&pane.resource_policy)
                    .map_err(|error| StorageError::Invariant(error.to_string()))?,
                pane.updated_at_ms
            ],
        )?;
        transaction.execute(
            "
            INSERT INTO tasks (
                id, pane_id, pid, pgid, start_token, state,
                exit_code, exit_signal, started_at_ms, ended_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ",
            params![
                task.id.to_string(),
                task.pane_id.to_string(),
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
        if let Some(worker_identity) = worker_identity {
            transaction.execute(
                "INSERT INTO pane_workers (
                    pane_id, pid, pgid, start_token, state, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    pane.id.to_string(),
                    worker_identity.pid,
                    worker_identity.pgid,
                    worker_identity.start_token,
                    if session.state == "closed" {
                        "closed"
                    } else {
                        "active"
                    },
                    session.updated_at_ms
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn find_pane_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<PaneRecord>, StorageError> {
        let Some(pane_id) = self.default_pane_id(session_id)? else {
            return Ok(None);
        };
        self.find_pane(&pane_id)
    }

    pub fn list_panes_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<PaneRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT p.id, p.session_id, rs.id, rs.requested_policy_json,
                    p.kind, p.layout_order, p.layout_weight, p.z_index,
                    p.floating_x, p.floating_y, p.floating_rows, p.floating_columns,
                    p.updated_at_ms
             FROM panes p
             JOIN resource_scopes rs
               ON rs.target_kind = 'pane' AND rs.target_id = p.id
             WHERE p.session_id = ?1
             ORDER BY p.layout_order, p.id",
        )?;
        let rows = statement.query_map([session_id], pane_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_layout_panes_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<PaneRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT p.id, p.session_id, rs.id, rs.requested_policy_json,
                    p.kind, p.layout_order, p.layout_weight, p.z_index,
                    p.floating_x, p.floating_y, p.floating_rows, p.floating_columns,
                    p.updated_at_ms
             FROM panes p
             JOIN resource_scopes rs
               ON rs.target_kind = 'pane' AND rs.target_id = p.id
             JOIN pane_workers pw
               ON pw.pane_id = p.id AND pw.state IN ('active', 'exited')
             WHERE p.session_id = ?1
             ORDER BY p.layout_order, p.id",
        )?;
        let rows = statement.query_map([session_id], pane_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_tiled_panes_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<PaneRecord>, StorageError> {
        Ok(self
            .list_layout_panes_for_session(session_id)?
            .into_iter()
            .filter(|pane| pane.kind == qwe_core::PaneKind::Normal)
            .collect())
    }

    pub fn find_pane(&self, pane_id: &PaneId) -> Result<Option<PaneRecord>, StorageError> {
        self.connection
            .query_row(
                "SELECT p.id, p.session_id, rs.id, rs.requested_policy_json,
                        p.kind, p.layout_order, p.layout_weight, p.z_index,
                        p.floating_x, p.floating_y, p.floating_rows, p.floating_columns,
                        p.updated_at_ms
                 FROM panes p
                 JOIN resource_scopes rs
                   ON rs.target_kind = 'pane' AND rs.target_id = p.id
                 WHERE p.id = ?1",
                [pane_id.to_string()],
                pane_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn list_panes(&self) -> Result<Vec<PaneRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT p.id, p.session_id, rs.id, rs.requested_policy_json,
                    p.kind, p.layout_order, p.layout_weight, p.z_index,
                    p.floating_x, p.floating_y, p.floating_rows, p.floating_columns,
                    p.updated_at_ms
             FROM panes p
             JOIN resource_scopes rs
               ON rs.target_kind = 'pane' AND rs.target_id = p.id
             ORDER BY p.session_id, p.layout_order, p.id",
        )?;
        let rows = statement.query_map([], pane_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn next_pane_layout_order(&self, session_id: &str) -> Result<u32, StorageError> {
        let next: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(p.layout_order), -1) + 1
             FROM panes p
             JOIN pane_workers pw ON pw.pane_id = p.id
             WHERE p.session_id = ?1 AND pw.state IN ('active', 'exited')",
            [session_id],
            |row| row.get(0),
        )?;
        u32::try_from(next)
            .map_err(|_| StorageError::Invariant("Pane layout order overflowed u32".into()))
    }

    pub fn next_pane_z_index(&self, session_id: &str) -> Result<u32, StorageError> {
        let next: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(p.z_index), -1) + 1
             FROM panes p
             JOIN pane_workers pw ON pw.pane_id = p.id
             WHERE p.session_id = ?1 AND pw.state IN ('active', 'exited')",
            [session_id],
            |row| row.get(0),
        )?;
        u32::try_from(next)
            .map_err(|_| StorageError::Invariant("Pane z-index overflowed u32".into()))
    }

    pub fn session_focus(&self, session_id: &str) -> Result<Option<PaneId>, StorageError> {
        self.connection
            .query_row(
                "SELECT focused_pane_id FROM session_layouts
                 WHERE session_id = ?1 AND focused_pane_id IS NOT NULL",
                [session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StorageError::from)?
            .map(|value| {
                PaneId::from_str(&value).map_err(|error| StorageError::Invariant(error.to_string()))
            })
            .transpose()
    }

    pub fn set_session_focus(
        &mut self,
        session_id: &str,
        pane_id: &PaneId,
        updated_at_ms: i64,
    ) -> Result<(), StorageError> {
        let changed = self.connection.execute(
            "UPDATE session_layouts
             SET focused_pane_id = ?2, updated_at_ms = ?3
             WHERE session_id = ?1
               AND EXISTS (
                   SELECT 1 FROM panes
                   WHERE id = ?2 AND session_id = ?1
               )",
            params![session_id, pane_id.to_string(), updated_at_ms],
        )?;
        if changed == 0 {
            return Err(StorageError::Invariant(
                "focused Pane does not belong to the Session".into(),
            ));
        }
        Ok(())
    }

    pub fn update_pane_floating_rect(
        &mut self,
        pane_id: &PaneId,
        rect: qwe_core::LayoutRect,
        updated_at_ms: i64,
    ) -> Result<PaneRecord, StorageError> {
        let pane = self
            .find_pane(pane_id)?
            .ok_or_else(|| StorageError::Invariant("Pane does not exist".into()))?;
        if pane.kind != qwe_core::PaneKind::Floating {
            return Err(StorageError::Invariant(
                "floating bounds require a floating Pane".into(),
            ));
        }
        rect.validate()
            .map_err(|error| StorageError::Invariant(error.to_string()))?;
        self.connection.execute(
            "UPDATE panes
             SET floating_x = ?2, floating_y = ?3, floating_rows = ?4,
                 floating_columns = ?5, updated_at_ms = ?6
             WHERE id = ?1",
            params![
                pane_id.to_string(),
                i64::from(rect.x),
                i64::from(rect.y),
                i64::from(rect.rows),
                i64::from(rect.columns),
                updated_at_ms
            ],
        )?;
        self.find_pane(pane_id)?
            .ok_or_else(|| StorageError::Invariant("Pane disappeared after update".into()))
    }

    pub fn raise_floating_pane(
        &mut self,
        pane_id: &PaneId,
        updated_at_ms: i64,
    ) -> Result<PaneRecord, StorageError> {
        let pane = self
            .find_pane(pane_id)?
            .ok_or_else(|| StorageError::Invariant("Pane does not exist".into()))?;
        if pane.kind != qwe_core::PaneKind::Floating {
            return Err(StorageError::Invariant(
                "raising requires a floating Pane".into(),
            ));
        }
        let next_z = self.next_pane_z_index(&pane.session_id)?;
        self.connection.execute(
            "UPDATE panes SET z_index = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![pane_id.to_string(), i64::from(next_z), updated_at_ms],
        )?;
        self.find_pane(pane_id)?
            .ok_or_else(|| StorageError::Invariant("Pane disappeared after raise".into()))
    }

    pub fn update_pane_resource_policy(
        &mut self,
        session_id: &str,
        policy: &ResourcePolicy,
        updated_at_ms: i64,
    ) -> Result<PaneRecord, StorageError> {
        let pane = self.find_pane_for_session(session_id)?.ok_or_else(|| {
            StorageError::Invariant(format!("Pane for Session '{session_id}' does not exist"))
        })?;
        self.update_resource_scope_policy(&pane.resource_scope_id, policy, updated_at_ms)?;
        self.connection.execute(
            "UPDATE panes SET updated_at_ms = ?2 WHERE id = ?1",
            params![pane.id.to_string(), updated_at_ms],
        )?;
        self.find_pane(&pane.id)?
            .ok_or_else(|| StorageError::Invariant("Pane disappeared after update".into()))
    }

    pub fn update_resource_scope_policy(
        &mut self,
        scope_id: &ResourceScopeId,
        policy: &ResourcePolicy,
        updated_at_ms: i64,
    ) -> Result<(), StorageError> {
        let policy_json = serde_json::to_string(policy)
            .map_err(|error| StorageError::Invariant(error.to_string()))?;
        let changed = self.connection.execute(
            "UPDATE resource_scopes SET requested_policy_json = ?2, updated_at_ms = ?3
             WHERE id = ?1",
            params![scope_id.to_string(), policy_json, updated_at_ms],
        )?;
        if changed == 0 {
            return Err(StorageError::Invariant(format!(
                "ResourceScope '{scope_id}' does not exist"
            )));
        }
        Ok(())
    }

    pub fn insert_created_pane(
        &mut self,
        pane: &PaneRecord,
        worker: &PaneWorkerRecord,
        task: &TaskRecord,
    ) -> Result<(), StorageError> {
        validate_pane_layout(pane)?;
        if pane.id != worker.pane_id
            || pane.id != task.pane_id
            || pane.session_id != task.session_id
        {
            return Err(StorageError::Invariant(
                "created Pane, worker, and Task identities do not match".into(),
            ));
        }
        let transaction = self.connection.transaction()?;
        let session_exists: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = ?1",
            [&pane.session_id],
            |row| row.get(0),
        )?;
        if session_exists != 1 {
            return Err(StorageError::Invariant(format!(
                "Session '{}' does not exist",
                pane.session_id
            )));
        }
        if pane.kind == qwe_core::PaneKind::Normal {
            transaction.execute(
                "UPDATE panes
                 SET layout_order = -layout_order - 1
                 WHERE session_id = ?1 AND layout_order >= ?2
                   AND kind = 'normal'
                   AND EXISTS (
                       SELECT 1 FROM pane_workers pw
                       WHERE pw.pane_id = panes.id AND pw.state IN ('active', 'exited')
                   )",
                params![pane.session_id, i64::from(pane.layout_order)],
            )?;
            transaction.execute(
                "UPDATE panes SET layout_order = -layout_order
                 WHERE session_id = ?1 AND layout_order < 0
                   AND kind = 'normal'
                   AND EXISTS (
                       SELECT 1 FROM pane_workers pw
                       WHERE pw.pane_id = panes.id AND pw.state IN ('active', 'exited')
                   )",
                [&pane.session_id],
            )?;
        }
        transaction.execute(
            "INSERT INTO panes (
                id, session_id, kind, layout_order, layout_weight, z_index,
                floating_x, floating_y, floating_rows, floating_columns, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                pane.id.to_string(),
                pane.session_id,
                pane.kind.as_str(),
                i64::from(pane.layout_order),
                i64::from(pane.layout_weight),
                i64::from(pane.z_index),
                pane.floating_rect.map(|rect| i64::from(rect.x)),
                pane.floating_rect.map(|rect| i64::from(rect.y)),
                pane.floating_rect.map(|rect| i64::from(rect.rows)),
                pane.floating_rect.map(|rect| i64::from(rect.columns)),
                pane.updated_at_ms
            ],
        )?;
        transaction.execute(
            "INSERT INTO session_layouts (session_id, focused_pane_id, updated_at_ms)
             VALUES (?1, NULL, ?2)
             ON CONFLICT(session_id) DO NOTHING",
            params![pane.session_id, pane.updated_at_ms],
        )?;
        let session_scope: String = transaction.query_row(
            "SELECT id FROM resource_scopes
             WHERE session_id = ?1 AND target_kind = 'session'",
            [&pane.session_id],
            |row| row.get(0),
        )?;
        let backend = if cfg!(target_os = "linux") {
            ResourceBackend::LinuxCgroupV2.as_str()
        } else if cfg!(target_os = "macos") {
            ResourceBackend::MacosMonitored.as_str()
        } else {
            ResourceBackend::Logical.as_str()
        };
        transaction.execute(
            "INSERT INTO resource_scopes (
                id, session_id, target_kind, target_id, parent_scope_id, backend,
                requested_policy_json, lifecycle_state, cleanup_outcome, updated_at_ms
             ) VALUES (?1, ?2, 'pane', ?3, ?4, ?5, ?6, 'active', NULL, ?7)",
            params![
                pane.resource_scope_id.to_string(),
                pane.session_id,
                pane.id.to_string(),
                session_scope,
                backend,
                serde_json::to_string(&pane.resource_policy)
                    .map_err(|error| StorageError::Invariant(error.to_string()))?,
                pane.updated_at_ms
            ],
        )?;
        transaction.execute(
            "INSERT INTO pane_workers (
                pane_id, pid, pgid, start_token, state, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                worker.pane_id.to_string(),
                worker.identity.pid,
                worker.identity.pgid,
                worker.identity.start_token,
                worker.state,
                worker.updated_at_ms
            ],
        )?;
        transaction.execute(
            "INSERT INTO tasks (
                id, pane_id, pid, pgid, start_token, state, exit_code,
                exit_signal, started_at_ms, ended_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task.id.to_string(),
                task.pane_id.to_string(),
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
        transaction.commit()?;
        Ok(())
    }

    pub fn reorder_panes(
        &mut self,
        session_id: &str,
        ordered: &[PaneId],
        updated_at_ms: i64,
    ) -> Result<(), StorageError> {
        if ordered.is_empty() {
            return Err(StorageError::Invariant(
                "Pane reorder requires at least one Pane".into(),
            ));
        }
        let mut expected = ordered.iter().map(ToString::to_string).collect::<Vec<_>>();
        expected.sort();
        if expected.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(StorageError::Invariant(
                "Pane reorder contains duplicate identities".into(),
            ));
        }
        let current = self
            .list_tiled_panes_for_session(session_id)?
            .into_iter()
            .map(|pane| pane.id.to_string())
            .collect::<Vec<_>>();
        let mut current = current;
        current.sort();
        if current != expected {
            return Err(StorageError::Invariant(
                "Pane reorder must include every Pane in the Session exactly once".into(),
            ));
        }

        let transaction = self.connection.transaction()?;
        transaction.execute(
            "UPDATE panes
             SET layout_order = -layout_order - 1, updated_at_ms = ?2
             WHERE session_id = ?1
               AND kind = 'normal'
               AND EXISTS (
                   SELECT 1 FROM pane_workers pw
                   WHERE pw.pane_id = panes.id AND pw.state IN ('active', 'exited')
               )",
            params![session_id, updated_at_ms],
        )?;
        for (order, pane_id) in ordered.iter().enumerate() {
            let changed = transaction.execute(
                "UPDATE panes
                 SET layout_order = ?3, updated_at_ms = ?4
                 WHERE session_id = ?1 AND id = ?2
                   AND kind = 'normal'
                   AND EXISTS (
                       SELECT 1 FROM pane_workers pw
                       WHERE pw.pane_id = panes.id AND pw.state IN ('active', 'exited')
                   )",
                params![
                    session_id,
                    pane_id.to_string(),
                    i64::try_from(order).map_err(|_| {
                        StorageError::Invariant("Pane layout order overflowed i64".into())
                    })?,
                    updated_at_ms
                ],
            )?;
            if changed != 1 {
                return Err(StorageError::Invariant(
                    "Pane disappeared during reorder".into(),
                ));
            }
        }
        transaction.execute(
            "UPDATE session_layouts SET updated_at_ms = ?2 WHERE session_id = ?1",
            params![session_id, updated_at_ms],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn upsert_pane_worker(&mut self, worker: &PaneWorkerRecord) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT INTO pane_workers (
                pane_id, pid, pgid, start_token, state, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(pane_id) DO UPDATE SET
                pid = excluded.pid,
                pgid = excluded.pgid,
                start_token = excluded.start_token,
                state = excluded.state,
                updated_at_ms = excluded.updated_at_ms",
            params![
                worker.pane_id.to_string(),
                worker.identity.pid,
                worker.identity.pgid,
                worker.identity.start_token,
                worker.state,
                worker.updated_at_ms
            ],
        )?;
        Ok(())
    }

    pub fn find_pane_worker(
        &self,
        pane_id: &PaneId,
    ) -> Result<Option<PaneWorkerRecord>, StorageError> {
        self.connection
            .query_row(
                "SELECT pane_id, pid, pgid, start_token, state, updated_at_ms
                 FROM pane_workers WHERE pane_id = ?1",
                [pane_id.to_string()],
                pane_worker_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn list_pane_workers(&self) -> Result<Vec<PaneWorkerRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT pane_id, pid, pgid, start_token, state, updated_at_ms
             FROM pane_workers ORDER BY pane_id",
        )?;
        let rows = statement.query_map([], pane_worker_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_resource_scopes(&self) -> Result<Vec<ResourceScopeRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, session_id, target_kind, target_id, parent_scope_id, backend,
                    requested_policy_json, lifecycle_state, cleanup_outcome, updated_at_ms
             FROM resource_scopes ORDER BY session_id, target_kind, target_id",
        )?;
        let rows = statement.query_map([], resource_scope_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_resource_scope_state(
        &mut self,
        scope_id: &ResourceScopeId,
        state: ResourceScopeState,
        cleanup_outcome: Option<&str>,
        updated_at_ms: i64,
    ) -> Result<(), StorageError> {
        let changed = self.connection.execute(
            "UPDATE resource_scopes
             SET lifecycle_state = ?2, cleanup_outcome = ?3, updated_at_ms = ?4
             WHERE id = ?1",
            params![
                scope_id.to_string(),
                state.as_str(),
                cleanup_outcome,
                updated_at_ms
            ],
        )?;
        if changed == 0 {
            return Err(StorageError::Invariant(format!(
                "ResourceScope '{scope_id}' does not exist"
            )));
        }
        Ok(())
    }

    pub fn close_session_pane_records(
        &mut self,
        session_id: &str,
        updated_at_ms: i64,
    ) -> Result<(), StorageError> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "UPDATE tasks
             SET state = 'stopped', exit_code = NULL, exit_signal = NULL,
                 ended_at_ms = ?2
             WHERE state = 'running'
               AND pane_id IN (SELECT id FROM panes WHERE session_id = ?1)",
            params![session_id, updated_at_ms],
        )?;
        transaction.execute(
            "UPDATE pane_workers
             SET state = 'closed', updated_at_ms = ?2
             WHERE pane_id IN (SELECT id FROM panes WHERE session_id = ?1)",
            params![session_id, updated_at_ms],
        )?;
        transaction.execute(
            "UPDATE resource_scopes
             SET lifecycle_state = 'closed',
                 cleanup_outcome = 'complete scope stopped',
                 updated_at_ms = ?2
             WHERE session_id = ?1 AND target_kind = 'pane'",
            params![session_id, updated_at_ms],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn local_root_principal(&self) -> Result<PrincipalRecord, StorageError> {
        self.connection
            .query_row(
                "SELECT id, parent_id, epoch, created_at_ms, revoked_at_ms
                 FROM principals WHERE id = ?1",
                [PrincipalId::local_root().to_string()],
                |row| {
                    Ok(PrincipalRecord {
                        id: PrincipalId::from_str(&row.get::<_, String>(0)?).unwrap(),
                        parent_id: row
                            .get::<_, Option<String>>(1)?
                            .map(|value| PrincipalId::from_str(&value).unwrap()),
                        epoch: row.get::<_, i64>(2)? as u64,
                        created_at_ms: row.get(3)?,
                        revoked_at_ms: row.get(4)?,
                    })
                },
            )
            .map_err(StorageError::from)
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
                id, pane_id, pid, pgid, start_token, state,
                exit_code, exit_signal, started_at_ms, ended_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ",
            params![
                task.id.to_string(),
                task.pane_id.to_string(),
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
                "SELECT t.id, p.session_id, t.pane_id, t.pid, t.pgid, t.start_token,
                        t.state, t.exit_code, t.exit_signal, t.started_at_ms, t.ended_at_ms
                 FROM tasks t JOIN panes p ON p.id = t.pane_id
                 WHERE t.id = ?1",
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
                "SELECT t.id, p.session_id, t.pane_id, t.pid, t.pgid, t.start_token,
                        t.state, t.exit_code, t.exit_signal, t.started_at_ms, t.ended_at_ms
                 FROM tasks t JOIN panes p ON p.id = t.pane_id
                 WHERE p.session_id = ?1
                 ORDER BY t.started_at_ms DESC, t.id DESC LIMIT 1",
                [session_id],
                task_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn find_task_for_pane(&self, pane_id: &PaneId) -> Result<Option<TaskRecord>, StorageError> {
        self.connection
            .query_row(
                "SELECT t.id, p.session_id, t.pane_id, t.pid, t.pgid, t.start_token,
                        t.state, t.exit_code, t.exit_signal, t.started_at_ms, t.ended_at_ms
                 FROM tasks t JOIN panes p ON p.id = t.pane_id
                 WHERE t.pane_id = ?1
                 ORDER BY t.started_at_ms DESC, t.id DESC LIMIT 1",
                [pane_id.to_string()],
                task_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn list_tasks(&self) -> Result<Vec<TaskRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT t.id, p.session_id, t.pane_id, t.pid, t.pgid, t.start_token,
                    t.state, t.exit_code, t.exit_signal, t.started_at_ms, t.ended_at_ms
             FROM tasks t JOIN panes p ON p.id = t.pane_id
             ORDER BY t.started_at_ms, t.id",
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
                  SELECT 1
                  FROM attachment_authorities aa
                  JOIN panes p ON p.id = aa.pane_id
                  WHERE p.session_id = sessions.id
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
            report.tasks += transaction.execute(
                "DELETE FROM tasks WHERE pane_id IN (
                        SELECT id FROM panes WHERE session_id = ?1
                     )",
                [&session_id],
            )? as u64;
            report.attachment_authorities += transaction.execute(
                "DELETE FROM attachment_authorities WHERE pane_id IN (
                    SELECT id FROM panes WHERE session_id = ?1
                 )",
                [&session_id],
            )? as u64;
            report.pane_workers += transaction.execute(
                "DELETE FROM pane_workers WHERE pane_id IN (
                    SELECT id FROM panes WHERE session_id = ?1
                 )",
                [&session_id],
            )? as u64;
            report.resource_scopes += transaction.execute(
                "DELETE FROM resource_scopes
                 WHERE session_id = ?1 AND target_kind = 'pane'",
                [&session_id],
            )? as u64;
            report.resource_scopes += transaction.execute(
                "DELETE FROM resource_scopes
                 WHERE session_id = ?1 AND target_kind = 'session'",
                [&session_id],
            )? as u64;
            transaction.execute(
                "DELETE FROM session_layouts WHERE session_id = ?1",
                [&session_id],
            )?;
            report.panes += transaction
                .execute("DELETE FROM panes WHERE session_id = ?1", [&session_id])?
                as u64;
            report.leases += transaction
                .execute("DELETE FROM leases WHERE session_id = ?1", [&session_id])?
                as u64;
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

fn validate_pane_layout(pane: &PaneRecord) -> Result<(), StorageError> {
    if pane.layout_weight == 0 || pane.layout_weight > 1_000 {
        return Err(StorageError::Invariant(format!(
            "Pane layout weight must be between 1 and 1000, received {}",
            pane.layout_weight
        )));
    }
    if pane.kind == qwe_core::PaneKind::Normal && pane.floating_rect.is_some() {
        return Err(StorageError::Invariant(
            "tiled Panes cannot store floating bounds".into(),
        ));
    }
    if let Some(rect) = pane.floating_rect {
        rect.validate()
            .map_err(|error| StorageError::Invariant(error.to_string()))?;
    }
    Ok(())
}

fn migrate_v10_to_v11(store: &mut Store) -> Result<(), StorageError> {
    use std::collections::HashMap;

    store.connection.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let result = (|| {
        let transaction = store.connection.transaction()?;
        transaction.execute_batch(
            "
            ALTER TABLE sessions RENAME TO sessions_v10;
            ALTER TABLE panes RENAME TO panes_v10;
            ALTER TABLE tasks RENAME TO tasks_v10;
            ALTER TABLE leases RENAME TO leases_v10;
            ALTER TABLE attachment_authorities RENAME TO attachment_authorities_v10;
            ALTER TABLE idempotency_records RENAME TO idempotency_records_v10;
            DROP INDEX IF EXISTS tasks_by_session;
            DROP INDEX IF EXISTS idempotency_expiry;
            ",
        )?;
        transaction.execute_batch(V11_SCHEMA_SQL)?;
        transaction.execute(
            "INSERT INTO principals (id, parent_id, epoch, created_at_ms, revoked_at_ms)
             VALUES (?1, NULL, 1, ?2, NULL)",
            params![PrincipalId::local_root().to_string(), unix_ms()],
        )?;

        struct LegacySession {
            id: String,
            owner: String,
            state: String,
            pid: Option<u32>,
            pgid: Option<u32>,
            start_token: Option<String>,
            worker_pid: Option<u32>,
            worker_pgid: Option<u32>,
            worker_start_token: Option<String>,
            updated_at_ms: i64,
        }

        let sessions = {
            let mut statement = transaction.prepare(
                "SELECT id, owner, state, pid, pgid, start_token,
                        worker_pid, worker_pgid, worker_start_token, updated_at_ms
                 FROM sessions_v10 ORDER BY id",
            )?;
            statement
                .query_map([], |row| {
                    Ok(LegacySession {
                        id: row.get(0)?,
                        owner: row.get(1)?,
                        state: row.get(2)?,
                        pid: row.get(3)?,
                        pgid: row.get(4)?,
                        start_token: row.get(5)?,
                        worker_pid: row.get(6)?,
                        worker_pgid: row.get(7)?,
                        worker_start_token: row.get(8)?,
                        updated_at_ms: row.get(9)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?
        };

        struct LegacyPane {
            id: String,
            session_id: String,
            cpu_seconds: Option<i64>,
            memory_high_bytes: Option<i64>,
            memory_bytes: Option<i64>,
            memory_swap_max_bytes: Option<i64>,
            process_limit: Option<i64>,
            cpu_quota_percent: Option<i64>,
            updated_at_ms: i64,
        }

        let panes = {
            let mut statement = transaction.prepare(
                "SELECT id, session_id, cpu_seconds, memory_high_bytes, memory_bytes,
                        memory_swap_max_bytes, process_limit, cpu_quota_percent,
                        updated_at_ms
                 FROM panes_v10 ORDER BY session_id, id",
            )?;
            statement
                .query_map([], |row| {
                    Ok(LegacyPane {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        cpu_seconds: row.get(2)?,
                        memory_high_bytes: row.get(3)?,
                        memory_bytes: row.get(4)?,
                        memory_swap_max_bytes: row.get(5)?,
                        process_limit: row.get(6)?,
                        cpu_quota_percent: row.get(7)?,
                        updated_at_ms: row.get(8)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?
        };

        for session in &sessions {
            transaction.execute(
                "INSERT INTO sessions (
                    id, owner, state, pid, pgid, start_token, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    session.id,
                    session.owner,
                    session.state,
                    session.pid,
                    session.pgid,
                    session.start_token,
                    session.updated_at_ms
                ],
            )?;
        }

        let mut pane_by_session = HashMap::new();
        let mut session_scope_by_session = HashMap::new();
        for pane in &panes {
            let session_scope_id = ResourceScopeId::new();
            transaction.execute(
                "INSERT INTO resource_scopes (
                    id, session_id, target_kind, target_id, parent_scope_id, backend,
                    requested_policy_json, lifecycle_state, cleanup_outcome, updated_at_ms
                 ) VALUES (?1, ?2, 'session', ?2, NULL, 'logical', '{}', 'active', NULL, ?3)",
                params![
                    session_scope_id.to_string(),
                    pane.session_id,
                    pane.updated_at_ms
                ],
            )?;
            session_scope_by_session.insert(pane.session_id.clone(), session_scope_id);
            transaction.execute(
                "INSERT INTO panes (id, session_id, updated_at_ms) VALUES (?1, ?2, ?3)",
                params![pane.id, pane.session_id, pane.updated_at_ms],
            )?;
            let requested = json!({
                "cpu_seconds": pane.cpu_seconds,
                "memory_high_bytes": pane.memory_high_bytes,
                "memory_max_bytes": pane.memory_bytes,
                "memory_swap_max_bytes": pane.memory_swap_max_bytes,
                "tasks_max": pane.process_limit,
                "cpu_quota_percent": pane.cpu_quota_percent
            });
            let backend = if cfg!(target_os = "linux") {
                ResourceBackend::LinuxCgroupV2.as_str()
            } else if cfg!(target_os = "macos") {
                ResourceBackend::MacosMonitored.as_str()
            } else {
                ResourceBackend::Logical.as_str()
            };
            transaction.execute(
                "INSERT INTO resource_scopes (
                    id, session_id, target_kind, target_id, parent_scope_id, backend,
                    requested_policy_json, lifecycle_state, cleanup_outcome, updated_at_ms
                 ) VALUES (?1, ?2, 'pane', ?3, ?4, ?5, ?6, 'active', NULL, ?7)",
                params![
                    ResourceScopeId::new().to_string(),
                    pane.session_id,
                    pane.id,
                    session_scope_by_session
                        .get(&pane.session_id)
                        .expect("session scope was inserted")
                        .to_string(),
                    backend,
                    requested.to_string(),
                    pane.updated_at_ms
                ],
            )?;
            pane_by_session
                .entry(pane.session_id.clone())
                .or_insert_with(|| pane.id.clone());
        }

        for session in &sessions {
            let pane_id = pane_by_session.get(&session.id).ok_or_else(|| {
                StorageError::Invariant(format!(
                    "v10 Session '{}' has no Pane to own its worker",
                    session.id
                ))
            })?;
            if let (Some(pid), Some(pgid), Some(start_token)) = (
                session.worker_pid,
                session.worker_pgid,
                session.worker_start_token.as_ref(),
            ) {
                transaction.execute(
                    "INSERT INTO pane_workers (
                        pane_id, pid, pgid, start_token, state, updated_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        pane_id,
                        pid,
                        pgid,
                        start_token,
                        if session.state == "closed" {
                            "closed"
                        } else {
                            "active"
                        },
                        session.updated_at_ms
                    ],
                )?;
            }
        }

        let task_rows = {
            let mut statement = transaction.prepare(
                "SELECT id, session_id, pid, pgid, start_token, state, exit_code,
                        exit_signal, started_at_ms, ended_at_ms
                 FROM tasks_v10 ORDER BY started_at_ms, id",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, u32>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<i32>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, Option<i64>>(9)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for task in task_rows {
            let pane_id = pane_by_session.get(&task.1).ok_or_else(|| {
                StorageError::Invariant(format!(
                    "v10 Task '{}' references Session '{}' without a Pane",
                    task.0, task.1
                ))
            })?;
            transaction.execute(
                "INSERT INTO tasks (
                    id, pane_id, pid, pgid, start_token, state, exit_code,
                    exit_signal, started_at_ms, ended_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    task.0, pane_id, task.2, task.3, task.4, task.5, task.6, task.7, task.8, task.9
                ],
            )?;
        }

        let authority_rows = {
            let mut statement = transaction.prepare(
                "SELECT session_id, state, owner, generation, reclaim_token_hash,
                        deadline_ms, updated_at_ms
                 FROM attachment_authorities_v10",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for authority in authority_rows {
            let pane_id = pane_by_session.get(&authority.0).ok_or_else(|| {
                StorageError::Invariant(format!(
                    "v10 attachment authority references Session '{}' without a Pane",
                    authority.0
                ))
            })?;
            transaction.execute(
                "INSERT INTO attachment_authorities (
                    pane_id, state, owner, generation, reclaim_token_hash,
                    deadline_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    pane_id,
                    authority.1,
                    authority.2,
                    authority.3,
                    authority.4,
                    authority.5,
                    authority.6
                ],
            )?;
        }

        transaction.execute_batch(
            "
            INSERT INTO leases (
                session_id, owner, generation, deadline_ms, updated_at_ms, released_at_ms
            )
            SELECT session_id, owner, generation, deadline_ms, updated_at_ms, released_at_ms
            FROM leases_v10;
            INSERT INTO idempotency_records (
                idempotency_key, operation, request_fingerprint, state, response_json,
                created_at_ms, expires_at_ms
            )
            SELECT idempotency_key, operation, request_fingerprint, state, response_json,
                   created_at_ms, expires_at_ms
            FROM idempotency_records_v10;
            DROP TABLE attachment_authorities_v10;
            DROP TABLE tasks_v10;
            DROP TABLE panes_v10;
            DROP TABLE leases_v10;
            DROP TABLE idempotency_records_v10;
            DROP TABLE sessions_v10;
            PRAGMA user_version = 11;
            ",
        )?;

        let orphaned: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM tasks t
             LEFT JOIN panes p ON p.id = t.pane_id
             WHERE p.id IS NULL",
            [],
            |row| row.get(0),
        )?;
        if orphaned != 0 {
            return Err(StorageError::Invariant(format!(
                "v11 migration produced {orphaned} Tasks without a Pane"
            )));
        }
        let pane_scopes: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM panes p
             LEFT JOIN resource_scopes s
               ON s.target_kind = 'pane' AND s.target_id = p.id
             WHERE s.id IS NULL",
            [],
            |row| row.get(0),
        )?;
        if pane_scopes != 0 {
            return Err(StorageError::Invariant(format!(
                "v11 migration produced {pane_scopes} Panes without a ResourceScope"
            )));
        }
        transaction.commit()?;
        Ok(())
    })();
    store.connection.execute_batch("PRAGMA foreign_keys=ON;")?;
    result?;
    let foreign_key_errors: i64 =
        store
            .connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
    if foreign_key_errors != 0 {
        return Err(StorageError::Invariant(format!(
            "v11 migration left {foreign_key_errors} foreign-key violations"
        )));
    }
    let integrity: String = store
        .connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(StorageError::Invariant(format!(
            "v11 migration integrity check failed: {integrity}"
        )));
    }
    Ok(())
}

fn migrate_v11_to_v12(store: &mut Store) -> Result<(), StorageError> {
    let transaction = store.connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE panes ADD COLUMN kind TEXT NOT NULL DEFAULT 'normal';
        ALTER TABLE panes ADD COLUMN layout_order INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE panes ADD COLUMN layout_weight INTEGER NOT NULL DEFAULT 1;
        ALTER TABLE panes ADD COLUMN z_index INTEGER NOT NULL DEFAULT 0;

        UPDATE panes
        SET layout_order = (
            SELECT COUNT(*)
            FROM panes AS preceding
            WHERE preceding.session_id = panes.session_id
              AND preceding.id < panes.id
        );

        CREATE INDEX panes_by_layout_order
            ON panes(session_id, layout_order);
        CREATE TABLE session_layouts (
            session_id TEXT PRIMARY KEY
                REFERENCES sessions(id) ON DELETE RESTRICT,
            focused_pane_id TEXT
                REFERENCES panes(id) ON DELETE SET NULL,
            updated_at_ms INTEGER NOT NULL
        );
        INSERT INTO session_layouts (session_id, focused_pane_id, updated_at_ms)
        SELECT p.session_id,
               (
                   SELECT first_pane.id
                   FROM panes AS first_pane
                   WHERE first_pane.session_id = p.session_id
                   ORDER BY first_pane.layout_order, first_pane.id
                   LIMIT 1
               ),
               MAX(p.updated_at_ms)
        FROM panes AS p
        GROUP BY p.session_id;
        PRAGMA user_version = 12;
        ",
    )?;
    let invalid_orders: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM (
            SELECT session_id, layout_order, COUNT(*) AS count
            FROM panes
            GROUP BY session_id, layout_order
            HAVING count != 1
         )",
        [],
        |row| row.get(0),
    )?;
    if invalid_orders != 0 {
        return Err(StorageError::Invariant(format!(
            "v12 migration produced {invalid_orders} duplicate Pane layout orders"
        )));
    }
    transaction.commit()?;
    let foreign_key_errors: i64 =
        store
            .connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
    if foreign_key_errors != 0 {
        return Err(StorageError::Invariant(format!(
            "v12 migration left {foreign_key_errors} foreign-key violations"
        )));
    }
    Ok(())
}

fn migrate_v12_to_v13(store: &mut Store) -> Result<(), StorageError> {
    let transaction = store.connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE panes ADD COLUMN floating_x INTEGER;
        ALTER TABLE panes ADD COLUMN floating_y INTEGER;
        ALTER TABLE panes ADD COLUMN floating_rows INTEGER;
        ALTER TABLE panes ADD COLUMN floating_columns INTEGER;
        PRAGMA user_version = 13;
        ",
    )?;
    let invalid_bounds: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM panes
         WHERE (floating_x IS NULL) != (floating_y IS NULL)
            OR (floating_x IS NULL) != (floating_rows IS NULL)
            OR (floating_x IS NULL) != (floating_columns IS NULL)",
        [],
        |row| row.get(0),
    )?;
    if invalid_bounds != 0 {
        return Err(StorageError::Invariant(format!(
            "v13 migration produced {invalid_bounds} partial floating bounds"
        )));
    }
    transaction.commit()?;
    Ok(())
}

fn unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}

fn backup_before_migration(
    connection: &Connection,
    path: &str,
    schema_version: i64,
) -> Result<(), StorageError> {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let backup_path = format!("{path}.pre-v{SCHEMA_VERSION}-{suffix}.sqlite");
    connection.execute("VACUUM INTO ?1", [&backup_path])?;
    let backup = Connection::open(&backup_path)?;
    let backed_up_version: i64 = backup.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let integrity: String = backup.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if backed_up_version != schema_version || integrity != "ok" {
        return Err(StorageError::Invariant(format!(
            "pre-migration backup verification failed at {backup_path}"
        )));
    }
    Ok(())
}

fn write_attachment_authority(
    connection: &Connection,
    pane_id: &PaneId,
    authority: &AttachmentAuthority,
    now_ms: i64,
) -> Result<(), StorageError> {
    if authority.state == AttachmentAuthorityState::Unattached {
        connection.execute(
            "DELETE FROM attachment_authorities WHERE pane_id = ?1",
            [pane_id.to_string()],
        )?;
        return Ok(());
    }
    connection.execute(
        "
        INSERT INTO attachment_authorities (
            pane_id, state, owner, generation, reclaim_token_hash,
            deadline_ms, updated_at_ms
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(pane_id) DO UPDATE SET
            state = excluded.state,
            owner = excluded.owner,
            generation = excluded.generation,
            reclaim_token_hash = excluded.reclaim_token_hash,
            deadline_ms = excluded.deadline_ms,
            updated_at_ms = excluded.updated_at_ms
        ",
        params![
            pane_id.to_string(),
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

fn pane_worker_from_row(row: &Row<'_>) -> rusqlite::Result<PaneWorkerRecord> {
    let pane_id: String = row.get(0)?;
    Ok(PaneWorkerRecord {
        pane_id: PaneId::from_str(&pane_id).map_err(|failure| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(failure),
            )
        })?,
        identity: ProcessIdentity {
            pid: row.get(1)?,
            pgid: row.get(2)?,
            start_token: row.get(3)?,
        },
        state: row.get(4)?,
        updated_at_ms: row.get(5)?,
    })
}

fn resource_scope_from_row(row: &Row<'_>) -> rusqlite::Result<ResourceScopeRecord> {
    let id: String = row.get(0)?;
    let parent_scope_id = row.get::<_, Option<String>>(4)?;
    let requested_policy_json: String = row.get(6)?;
    Ok(ResourceScopeRecord {
        id: ResourceScopeId::from_str(&id).map_err(|failure| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(failure),
            )
        })?,
        session_id: row.get(1)?,
        target_kind: ResourceScopeTargetKind::from_db(&row.get::<_, String>(2)?),
        target_id: row.get(3)?,
        parent_scope_id: parent_scope_id
            .map(|value| {
                ResourceScopeId::from_str(&value).map_err(|failure| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        Box::new(failure),
                    )
                })
            })
            .transpose()?,
        backend: ResourceBackend::from_db(&row.get::<_, String>(5)?),
        requested_policy: serde_json::from_str(&requested_policy_json).map_err(|failure| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                Box::new(failure),
            )
        })?,
        lifecycle_state: ResourceScopeState::from_db(&row.get::<_, String>(7)?),
        cleanup_outcome: row.get(8)?,
        updated_at_ms: row.get(9)?,
    })
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<TaskRecord> {
    let id: String = row.get(0)?;
    let pane_id: String = row.get(2)?;
    Ok(TaskRecord {
        id: TaskId::from_str(&id).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        session_id: row.get(1)?,
        pane_id: PaneId::from_str(&pane_id).map_err(|failure| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(failure),
            )
        })?,
        identity: ProcessIdentity {
            pid: row.get(3)?,
            pgid: row.get(4)?,
            start_token: row.get(5)?,
        },
        state: TaskState::from_db(&row.get::<_, String>(6)?),
        exit_code: row.get(7)?,
        exit_signal: row.get(8)?,
        started_at_ms: row.get(9)?,
        ended_at_ms: row.get(10)?,
    })
}

fn pane_from_row(row: &Row<'_>) -> rusqlite::Result<PaneRecord> {
    let id: String = row.get(0)?;
    let scope_id: String = row.get(2)?;
    let policy_json: String = row.get(3)?;
    let resource_policy =
        serde_json::from_str::<ResourcePolicy>(&policy_json).map_err(|failure| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(failure),
            )
        })?;
    let floating = [
        row.get::<_, Option<i64>>(8)?,
        row.get::<_, Option<i64>>(9)?,
        row.get::<_, Option<i64>>(10)?,
        row.get::<_, Option<i64>>(11)?,
    ];
    let floating_rect = match floating {
        [None, None, None, None] => None,
        [Some(x), Some(y), Some(rows), Some(columns)] => Some(qwe_core::LayoutRect {
            x: u16::try_from(x).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    8,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })?,
            y: u16::try_from(y).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    9,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })?,
            rows: u16::try_from(rows).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    10,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })?,
            columns: u16::try_from(columns).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    11,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })?,
        }),
        _ => {
            return Err(rusqlite::Error::InvalidColumnType(
                8,
                "floating bounds".into(),
                rusqlite::types::Type::Null,
            ));
        }
    };
    Ok(PaneRecord {
        id: PaneId::from_str(&id).map_err(|failure| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(failure),
            )
        })?,
        session_id: row.get(1)?,
        resource_scope_id: ResourceScopeId::from_str(&scope_id).map_err(|failure| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(failure),
            )
        })?,
        resource_policy,
        kind: qwe_core::PaneKind::from_db(&row.get::<_, String>(4)?),
        layout_order: row.get::<_, i64>(5)? as u32,
        layout_weight: row.get::<_, i64>(6)? as u16,
        z_index: row.get::<_, i64>(7)? as u32,
        floating_rect,
        updated_at_ms: row.get(12)?,
    })
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

    fn task(id: TaskId, session_id: &str, pane_id: &PaneId) -> TaskRecord {
        TaskRecord {
            id,
            session_id: session_id.into(),
            pane_id: pane_id.clone(),
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
            resource_scope_id: ResourceScopeId::new(),
            resource_policy: ResourcePolicy::empty(),
            kind: qwe_core::PaneKind::Normal,
            layout_order: 0,
            layout_weight: 1,
            z_index: 0,
            floating_rect: None,
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

    fn create_running_session(store: &mut Store, id: &str) -> PaneRecord {
        let pane = pane(id);
        let mut record = session(id);
        record.worker_identity = Some(ProcessIdentity {
            pid: 900,
            pgid: 900,
            start_token: "worker:900".into(),
        });
        store
            .insert_created_session(&record, &pane, &task(TaskId::new(), id, &pane.id))
            .unwrap();
        pane
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
        let pane = create_running_session(&mut store, "session-1");
        store
            .insert_task(&task(id.clone(), "session-1", &pane.id))
            .unwrap();
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
        let existing_pane = create_running_session(&mut store, "existing");
        store
            .insert_task(&task(existing_id.clone(), "existing", &existing_pane.id))
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
        let created_pane = pane("new");
        let mut conflicting_task = task(existing_id, "new", &created_pane.id);
        conflicting_task.identity.pid = 200;
        assert!(
            store
                .insert_created_session(&session, &created_pane, &conflicting_task)
                .is_err()
        );
        assert!(store.find_session("new").unwrap().is_none());
    }

    #[test]
    fn pane_targeted_attachment_hold_does_not_write_the_default_pane() {
        let mut store = Store::open_in_memory().unwrap();
        let first = create_running_session(&mut store, "layout-hold");
        let mut second = pane("layout-hold");
        second.layout_order = 1;
        let worker = PaneWorkerRecord {
            pane_id: second.id.clone(),
            identity: ProcessIdentity {
                pid: 300,
                pgid: 300,
                start_token: "worker:300".into(),
            },
            state: "active".into(),
            updated_at_ms: 20,
        };
        let second_task = task(TaskId::new(), "layout-hold", &second.id);
        store
            .insert_created_pane(&second, &worker, &second_task)
            .unwrap();

        store
            .persist_attachment_hold_for_pane(&second.id, "layout-hold", "alice", 7, 1_000, 100)
            .unwrap();

        assert!(
            store
                .get_attachment_authority_for_pane(&first.id)
                .unwrap()
                .is_none()
        );
        let held = store
            .get_attachment_authority_for_pane(&second.id)
            .unwrap()
            .unwrap();
        assert_eq!(held.state, AttachmentAuthorityState::Held);
        assert_eq!(held.owner.as_deref(), Some("alice"));
        assert_eq!(held.generation, 7);
        assert!(matches!(
            store.persist_attachment_hold_for_pane(
                &second.id,
                "another-session",
                "alice",
                7,
                1_000,
                100
            ),
            Err(StorageError::Invariant(_))
        ));
    }

    #[test]
    fn pane_insertion_and_reorder_preserve_unique_layout_order() {
        let mut store = Store::open_in_memory().unwrap();
        let first = create_running_session(&mut store, "layout-order");
        let mut second = pane("layout-order");
        second.layout_order = 1;
        let second_task = task(TaskId::new(), "layout-order", &second.id);
        store
            .insert_created_pane(
                &second,
                &PaneWorkerRecord {
                    pane_id: second.id.clone(),
                    identity: ProcessIdentity {
                        pid: 300,
                        pgid: 300,
                        start_token: "worker:300".into(),
                    },
                    state: "active".into(),
                    updated_at_ms: 20,
                },
                &second_task,
            )
            .unwrap();

        let mut inserted = pane("layout-order");
        inserted.layout_order = 1;
        let inserted_task = task(TaskId::new(), "layout-order", &inserted.id);
        store
            .insert_created_pane(
                &inserted,
                &PaneWorkerRecord {
                    pane_id: inserted.id.clone(),
                    identity: ProcessIdentity {
                        pid: 310,
                        pgid: 310,
                        start_token: "worker:310".into(),
                    },
                    state: "active".into(),
                    updated_at_ms: 30,
                },
                &inserted_task,
            )
            .unwrap();
        let panes = store.list_panes_for_session("layout-order").unwrap();
        assert_eq!(
            panes.iter().map(|pane| pane.id.clone()).collect::<Vec<_>>(),
            vec![first.id.clone(), inserted.id.clone(), second.id.clone()]
        );
        assert_eq!(
            panes
                .iter()
                .map(|pane| pane.layout_order)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );

        store
            .reorder_panes(
                "layout-order",
                &[second.id.clone(), first.id.clone(), inserted.id.clone()],
                40,
            )
            .unwrap();
        assert_eq!(
            store
                .list_panes_for_session("layout-order")
                .unwrap()
                .iter()
                .map(|pane| pane.id.clone())
                .collect::<Vec<_>>(),
            vec![second.id.clone(), first.id.clone(), inserted.id.clone()]
        );
        let mut floating = pane("layout-order");
        floating.kind = qwe_core::PaneKind::Floating;
        floating.layout_order = 3;
        floating.z_index = 1;
        store
            .insert_created_pane(
                &floating,
                &PaneWorkerRecord {
                    pane_id: floating.id.clone(),
                    identity: ProcessIdentity {
                        pid: 320,
                        pgid: 320,
                        start_token: "worker:320".into(),
                    },
                    state: "active".into(),
                    updated_at_ms: 45,
                },
                &task(TaskId::new(), "layout-order", &floating.id),
            )
            .unwrap();
        assert_eq!(
            store
                .list_layout_panes_for_session("layout-order")
                .unwrap()
                .len(),
            4
        );
        assert_eq!(
            store
                .list_tiled_panes_for_session("layout-order")
                .unwrap()
                .len(),
            3
        );
        let bounds = qwe_core::LayoutRect {
            x: 7,
            y: 2,
            rows: 12,
            columns: 40,
        };
        let updated = store
            .update_pane_floating_rect(&floating.id, bounds, 50)
            .unwrap();
        assert_eq!(updated.floating_rect, Some(bounds));
        assert!(matches!(
            store.update_pane_floating_rect(&first.id, bounds, 55),
            Err(StorageError::Invariant(_))
        ));
        assert!(matches!(
            store.reorder_panes("layout-order", &[second.id.clone(), second.id.clone()], 50),
            Err(StorageError::Invariant(_))
        ));
    }

    #[test]
    fn created_session_uses_pane_keyed_tasks_and_resource_scopes() {
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
        let created_task = task(TaskId::new(), "one", &created_pane.id);
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
        let second_task = task(TaskId::new(), "one", &created_pane.id);
        store.insert_task(&second_task).unwrap();
        assert_eq!(
            store
                .list_tasks()
                .unwrap()
                .into_iter()
                .filter(|task| task.pane_id == created_pane.id)
                .count(),
            2
        );
        let mut second_pane = pane("one");
        second_pane.layout_order = 1;
        let second_worker = PaneWorkerRecord {
            pane_id: second_pane.id.clone(),
            identity: ProcessIdentity {
                pid: 310,
                pgid: 310,
                start_token: "worker:310".into(),
            },
            state: "active".into(),
            updated_at_ms: 30,
        };
        let second_pane_task = task(TaskId::new(), "one", &second_pane.id);
        store
            .insert_created_pane(&second_pane, &second_worker, &second_pane_task)
            .unwrap();
        assert_eq!(store.list_panes_for_session("one").unwrap().len(), 2);
        store.set_session_focus("one", &second_pane.id, 40).unwrap();
        assert_eq!(
            store.session_focus("one").unwrap(),
            Some(second_pane.id.clone())
        );
        assert_eq!(
            store.find_pane_for_session("one").unwrap().unwrap().id,
            second_pane.id
        );
        store
            .set_session_focus("missing-session", &second_pane.id, 50)
            .unwrap_err();
        assert!(
            store.find_pane(&second_pane.id).unwrap().is_some(),
            "Pane lookup must not depend on the Session's first Pane"
        );
        let missing_pane = PaneId::new();
        assert!(
            store
                .insert_task(&task(TaskId::new(), "missing-session", &missing_pane))
                .is_err(),
            "TaskInstance must reference an existing Pane"
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
        create_running_session(&mut store, "authority");
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
    fn migrates_populated_v10_database_to_v13_with_backup() {
        let path = std::env::temp_dir().join(format!(
            "qwe-storage-v10-{}-{}.sqlite",
            std::process::id(),
            TaskId::new()
        ));
        let pane_id = PaneId::new();
        let task_id = TaskId::new();
        let token_hash = qwe_core::ReclaimTokenHash::from_token("legacy-token");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
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
                    CREATE TABLE panes (
                        id TEXT PRIMARY KEY,
                        session_id TEXT NOT NULL UNIQUE,
                        cpu_seconds INTEGER,
                        memory_high_bytes INTEGER,
                        memory_bytes INTEGER,
                        memory_swap_max_bytes INTEGER,
                        process_limit INTEGER,
                        cpu_quota_percent INTEGER,
                        updated_at_ms INTEGER NOT NULL
                    );
                    CREATE TABLE tasks (
                        id TEXT PRIMARY KEY,
                        session_id TEXT NOT NULL,
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
                    CREATE TABLE leases (
                        session_id TEXT PRIMARY KEY,
                        owner TEXT NOT NULL,
                        generation INTEGER NOT NULL,
                        deadline_ms INTEGER NOT NULL,
                        updated_at_ms INTEGER NOT NULL,
                        released_at_ms INTEGER
                    );
                    CREATE TABLE attachment_authorities (
                        session_id TEXT PRIMARY KEY,
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
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO sessions VALUES (
                        'legacy', 'local', 'running', 100, 100, 'task:100',
                        90, 90, 'worker:90', 10
                     )",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO panes VALUES (
                        ?1, 'legacy', 5, 67108864, 134217728, 0, 32, 150, 10
                     )",
                    [pane_id.to_string()],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO tasks VALUES (
                        ?1, 'legacy', 100, 100, 'task:100', 'running', NULL, NULL, 9, NULL
                     )",
                    [task_id.to_string()],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO leases VALUES ('legacy', 'alice', 3, 5000, 10, NULL)",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO attachment_authorities VALUES (
                        'legacy', 'active', 'alice', 4, ?1, 5000, 10
                     )",
                    [token_hash.as_str()],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO idempotency_records VALUES (
                        'legacy-key', 'SESSION_CREATE', 'fingerprint', 'complete',
                        '{\"Ok\":{\"session_id\":\"legacy\"}}', 10, 999999
                     )",
                    [],
                )
                .unwrap();
        }

        let store = Store::open(path.to_str().unwrap()).unwrap();
        assert_eq!(store.schema_version(), 13);
        let session = store.find_session("legacy").unwrap().unwrap();
        assert_eq!(session.identity.unwrap().pid, 100);
        assert_eq!(session.worker_identity.unwrap().pid, 90);
        let pane = store.find_pane(&pane_id).unwrap().unwrap();
        assert_eq!(pane.resource_policy.memory_max_bytes, Some(134217728));
        assert_eq!(pane.kind, qwe_core::PaneKind::Normal);
        assert_eq!(pane.layout_order, 0);
        assert_eq!(pane.layout_weight, 1);
        assert_eq!(pane.z_index, 0);
        assert_eq!(
            store.session_focus("legacy").unwrap(),
            Some(pane_id.clone())
        );
        let worker = store.find_pane_worker(&pane_id).unwrap().unwrap();
        assert_eq!(worker.identity.start_token, "worker:90");
        let task = store.find_task(&task_id).unwrap().unwrap();
        assert_eq!(task.pane_id, pane_id);
        assert_eq!(
            store
                .get_attachment_authority_for_pane(&pane_id)
                .unwrap()
                .unwrap()
                .reclaim_token_hash,
            Some(token_hash)
        );
        assert_eq!(
            store
                .find_idempotency("legacy-key", 11)
                .unwrap()
                .unwrap()
                .response_json
                .as_deref(),
            Some(r#"{"Ok":{"session_id":"legacy"}}"#)
        );
        assert_eq!(store.list_resource_scopes().unwrap().len(), 2);
        assert_eq!(store.local_root_principal().unwrap().epoch, 1);

        let backup_prefix = format!("{}.pre-v13-", path.file_name().unwrap().to_string_lossy());
        let backup = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|candidate| {
                candidate
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(&backup_prefix))
            })
            .expect("v13 migration must leave a verified v10 backup");
        let backup = Connection::open(&backup).unwrap();
        let backup_version: i64 = backup
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(backup_version, 10);
        drop(backup);
        drop(store);

        let backup_prefix = format!("{}.pre-v13-", path.file_name().unwrap().to_string_lossy());
        for entry in std::fs::read_dir(path.parent().unwrap()).unwrap().flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(&backup_prefix)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    }

    #[test]
    fn migrates_v11_multi_pane_layout_with_stable_order_and_focus() {
        let path = std::env::temp_dir().join(format!(
            "qwe-storage-v11-layout-{}-{}.sqlite",
            std::process::id(),
            TaskId::new()
        ));
        let first = PaneId::new();
        let second = PaneId::new();
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "PRAGMA foreign_keys=ON;
                     PRAGMA user_version = 11;",
                )
                .unwrap();
            connection.execute_batch(V11_SCHEMA_SQL).unwrap();
            connection
                .execute(
                    "INSERT INTO sessions (
                        id, owner, state, pid, pgid, start_token, updated_at_ms
                     ) VALUES ('layout', 'local', 'running', NULL, NULL, NULL, 10)",
                    [],
                )
                .unwrap();
            let session_scope = ResourceScopeId::new();
            connection
                .execute(
                    "INSERT INTO resource_scopes (
                        id, session_id, target_kind, target_id, parent_scope_id, backend,
                        requested_policy_json, lifecycle_state, cleanup_outcome, updated_at_ms
                     ) VALUES (?1, 'layout', 'session', 'layout', NULL, 'logical', '{}',
                               'active', NULL, 10)",
                    [session_scope.to_string()],
                )
                .unwrap();
            for pane in [&first, &second] {
                connection
                    .execute(
                        "INSERT INTO panes (id, session_id, updated_at_ms)
                         VALUES (?1, 'layout', 10)",
                        [pane.to_string()],
                    )
                    .unwrap();
                connection
                    .execute(
                        "INSERT INTO resource_scopes (
                            id, session_id, target_kind, target_id, parent_scope_id, backend,
                            requested_policy_json, lifecycle_state, cleanup_outcome, updated_at_ms
                         ) VALUES (?1, 'layout', 'pane', ?2, ?3, 'logical', '{}',
                                   'active', NULL, 10)",
                        params![
                            ResourceScopeId::new().to_string(),
                            pane.to_string(),
                            session_scope.to_string()
                        ],
                    )
                    .unwrap();
            }
            connection
                .execute_batch("PRAGMA user_version = 11;")
                .unwrap();
        }

        let store = Store::open(path.to_str().unwrap()).unwrap();
        assert_eq!(store.schema_version(), 13);
        let panes = store.list_panes_for_session("layout").unwrap();
        assert_eq!(panes.len(), 2);
        let mut expected = [first.clone(), second.clone()];
        expected.sort_by_key(ToString::to_string);
        assert_eq!(
            panes.iter().map(|pane| pane.id.clone()).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            panes
                .iter()
                .map(|pane| pane.layout_order)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            store.session_focus("layout").unwrap(),
            Some(expected[0].clone())
        );
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        for entry in std::fs::read_dir(path.parent().unwrap()).unwrap().flatten() {
            if entry.file_name().to_string_lossy().starts_with(&format!(
                "{}.pre-v13-",
                path.file_name().unwrap().to_string_lossy()
            )) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
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
            let pane = pane(id);
            store
                .insert_created_session(&record, &pane, &task(TaskId::new(), id, &pane.id))
                .unwrap();
        }
        let old_pane = store
            .find_pane_for_session("old-outside-capacity")
            .unwrap()
            .unwrap();
        let pruned_id = TaskId::new();
        store
            .insert_task(&task(
                pruned_id.clone(),
                "old-outside-capacity",
                &old_pane.id,
            ))
            .unwrap();

        let report = store.prune_terminal_history(10_000, 1_000, 2).unwrap();
        assert_eq!(report.sessions, 1);
        assert_eq!(report.tasks, 2);
        assert_eq!(report.panes, 1);
        assert_eq!(report.resource_scopes, 2);
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
        let pane = pane("authority-protected");
        store
            .insert_created_session(
                &closed,
                &pane,
                &task(TaskId::new(), "authority-protected", &pane.id),
            )
            .unwrap();
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
