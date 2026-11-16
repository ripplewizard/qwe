use std::collections::VecDeque;
use thiserror::Error;

mod cgroup;
mod fault_injection;
mod landlock;
mod path;
mod process;
mod sandbox_control;
mod terminal;
mod terminal_output;

pub use cgroup::{
    CgroupStats, Pressure, ResourceSupport, apply_resource_policy, current_cgroup_path,
    detect_resource_support, kill_cgroup, pane_scope_unit, read_cgroup_stats,
    read_cpu_usage_microseconds, read_effective_resource_policy, read_enforcement_event,
    read_resource_usage, stop_pane_scope, stop_systemd_scope, systemd_scope_available,
};
pub use fault_injection::{SANDBOX_FAULT_ENV, check_sandbox_fault};
pub use landlock::{
    LandlockError, LandlockPolicy, abi_version as landlock_abi_version, apply as apply_landlock,
};
pub use path::resolve_beneath;
pub use process::{PidFd, process_identity, process_matches};
pub use sandbox_control::{
    CONTROL_HEADER_BYTES, CONTROL_KIND_SIGNAL, CONTROL_MAX_BUFFER, CONTROL_MAX_PAYLOAD,
    CONTROL_VERSION, PaneControlDecoder, PaneControlMessage,
};
pub use terminal::{
    CursorStyle, MouseProtocolEncoding, MouseProtocolMode, TerminalScreen, TerminalSnapshot,
};
pub use terminal_output::RestrictedTerminalFilter;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum JournalError {
    #[error("journal limit must be greater than zero")]
    ZeroLimit,
}

#[derive(Debug)]
pub struct BoundedJournal {
    limit: usize,
    bytes: VecDeque<u8>,
    total_written: u64,
}

impl BoundedJournal {
    pub fn new(limit: usize) -> Result<Self, JournalError> {
        if limit == 0 {
            return Err(JournalError::ZeroLimit);
        }
        Ok(Self {
            limit,
            bytes: VecDeque::with_capacity(limit),
            total_written: 0,
        })
    }

    pub fn append(&mut self, data: &[u8]) {
        self.total_written += data.len() as u64;
        if data.len() >= self.limit {
            self.bytes.clear();
            self.bytes.extend(&data[data.len() - self.limit..]);
            return;
        }
        let combined = self.bytes.len() + data.len();
        let overflow = combined.saturating_sub(self.limit);
        self.bytes.drain(..overflow);
        self.bytes.extend(data);
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }

    #[must_use]
    pub fn tail(&self, limit: usize) -> Vec<u8> {
        let start = self.bytes.len().saturating_sub(limit);
        self.bytes.iter().skip(start).copied().collect()
    }

    #[must_use]
    pub fn truncated_from(&self) -> u64 {
        self.total_written.saturating_sub(self.bytes.len() as u64)
    }

    #[must_use]
    pub fn total_written(&self) -> u64 {
        self.total_written
    }
}

pub trait TerminalState {
    fn process(&mut self, bytes: &[u8]);
    fn snapshot(&self) -> Vec<u8>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_stays_bounded_and_reports_truncation() {
        let mut journal = BoundedJournal::new(8).unwrap();
        journal.append(b"abcdef");
        journal.append(b"ghijkl");
        assert_eq!(journal.snapshot(), b"efghijkl");
        assert_eq!(journal.total_written(), 12);
        assert_eq!(journal.truncated_from(), 4);
        assert_eq!(journal.tail(4), b"ijkl");
    }
}
