# Daemon Schema Upgrade POC Contract

Status: implemented and passing for Rust, Go, and C++.

## Schema Versions

- Version 0: unversioned legacy database containing the `tasks` table.
- Version 1: explicitly versioned task schema.
- Version 2: adds `restart_tasks` and `task_instances`.

The current implementation supports database schema version 2.

## Upgrade Rules

1. Opening an older known schema creates missing tables and sets
   `PRAGMA user_version` to 2.
2. Existing task rows remain queryable after migration.
3. Reopening an already migrated database is idempotent.
4. A database with a version newer than the implementation must be rejected
   with a non-zero daemon exit before accepting IPC requests.

This POC covers additive migration and task compatibility. Destructive schema
changes and online upgrade while another daemon owns the database are outside
this slice.
