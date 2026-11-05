use qwe_core::{
    LeaseRecord, PaneId, PaneRecord, ProcessIdentity, SessionRecord, TaskId, TaskRecord, TaskState,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::str::FromStr;
use thiserror::Error;

pub const SCHEMA_VERSION: i64 = 5;

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
                CREATE TABLE panes (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL UNIQUE,
                    updated_at_ms INTEGER NOT NULL
                );
                CREATE TABLE leases (
                    session_id TEXT PRIMARY KEY,
                    owner TEXT NOT NULL,
                    generation INTEGER NOT NULL,
                    deadline_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    released_at_ms INTEGER
                );
                PRAGMA user_version = 5;
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
            "INSERT INTO panes (id, session_id, updated_at_ms) VALUES (?1, ?2, ?3)",
            params![pane.id.to_string(), pane.session_id, pane.updated_at_ms],
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
                "SELECT id, session_id, updated_at_ms FROM panes WHERE session_id = ?1",
                [session_id],
                |row| {
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
                        updated_at_ms: row.get(2)?,
                    })
                },
            )
            .optional()
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
            updated_at_ms: 10,
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
        let created_pane = pane("one");
        let created_task = task(TaskId::new(), "one");
        store
            .insert_created_session(&session, &created_pane, &created_task)
            .unwrap();
        assert_eq!(
            store.find_pane_for_session("one").unwrap().unwrap().id,
            created_pane.id
        );
        assert_eq!(
            store.find_task_for_session("one").unwrap().unwrap().id,
            created_task.id
        );
        assert!(
            store.insert_task(&task(TaskId::new(), "one")).is_err(),
            "a second TaskInstance for one Session must be rejected"
        );
    }

    #[test]
    fn lease_claim_touch_release_and_takeover() {
        let mut store = Store::open_in_memory().unwrap();
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
}
