use rusqlite::Connection;
use thiserror::Error;

pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("database schema is newer than this binary")]
    SchemaTooNew,
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
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<(), StorageError> {
        let current: i64 = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if current > SCHEMA_VERSION {
            return Err(StorageError::SchemaTooNew);
        }
        if current < 1 {
            self.connection.execute_batch(
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
            )?;
        }
        Ok(())
    }

    #[must_use]
    pub fn schema_version(&self) -> i64 {
        self.connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_versioned_schema() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.schema_version(), SCHEMA_VERSION);
    }
}
