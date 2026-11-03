use qwe_core::ProcessIdentity;
use std::io;

#[cfg(target_os = "linux")]
pub fn process_identity(pid: u32) -> io::Result<ProcessIdentity> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields = stat
        .rsplit_once(") ")
        .map(|(_, fields)| fields)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid /proc stat"))?;
    let values: Vec<&str> = fields.split_whitespace().collect();
    let pgid = values
        .get(2)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing process group"))?
        .parse::<u32>()
        .map_err(|failure| io::Error::new(io::ErrorKind::InvalidData, failure))?;
    let start_ticks = values
        .get(19)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing start time"))?;
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    Ok(ProcessIdentity {
        pid,
        pgid,
        start_token: format!("{}:{start_ticks}", boot_id.trim()),
    })
}

#[cfg(target_os = "macos")]
pub fn process_identity(pid: u32) -> io::Result<ProcessIdentity> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let expected = std::mem::size_of::<libc::proc_bsdinfo>();
    let received = unsafe {
        libc::proc_pidinfo(
            pid as libc::pid_t,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            expected as libc::c_int,
        )
    };
    if received != expected as libc::c_int {
        return Err(io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok(ProcessIdentity {
        pid,
        pgid: info.pbi_pgid,
        start_token: format!("{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec),
    })
}

#[must_use]
pub fn process_matches(identity: &ProcessIdentity) -> bool {
    process_identity(identity.pid).is_ok_and(|current| current == *identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_has_stable_identity() {
        let identity = process_identity(std::process::id()).unwrap();
        assert_eq!(identity.pid, std::process::id());
        assert!(!identity.start_token.is_empty());
        assert!(process_matches(&identity));
    }
}
