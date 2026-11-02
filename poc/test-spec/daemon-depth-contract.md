# Daemon Depth POC Contract

Status: automated coverage complete.

## Scope

- D01 validates request IDs, malformed commands, not-found errors, and explicit
  error codes.
- D02 records concurrent request latency and state invariants.
- D04 validates corruption rejection, SQLite lock contention, and a real
  isolated-filesystem full-disk/ENOSPC path.
- D05 validates that a partial/disconnected client cannot block later requests.
- D08 performs a sustained request loop and records errors plus daemon RSS
  before/after.

The full-disk case uses a temporary ext4 image mounted through `fuse2fs` on
Linux and a temporary HFS+ disk image on macOS. It fills the image through real
writes until `ENOSPC`, starts a new SQLite-backed task, verifies an explicit
error while read access remains available, removes the filler, and verifies
that a new task can start.

No mocked daemon state, filesystem error, or canned success is permitted.
