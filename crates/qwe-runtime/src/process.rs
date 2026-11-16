use qwe_core::ProcessIdentity;
use std::io;
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct PidFd {
    descriptor: OwnedFd,
}

#[cfg(target_os = "linux")]
impl PidFd {
    pub fn open(pid: u32) -> io::Result<Self> {
        let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0_u32) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            descriptor: unsafe { OwnedFd::from_raw_fd(descriptor as i32) },
        })
    }

    pub fn send_signal(&self, signal: libc::c_int) -> io::Result<()> {
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.descriptor.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0_u32,
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn is_running(&self) -> io::Result<bool> {
        let mut descriptor = libc::pollfd {
            fd: self.descriptor.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(result == 0 || descriptor.revents & libc::POLLIN == 0)
    }
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
pub struct PidFd {
    _private: (),
}

#[cfg(not(target_os = "linux"))]
impl PidFd {
    pub fn open(_pid: u32) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pidfd is supported only on Linux",
        ))
    }

    pub fn send_signal(&self, _signal: libc::c_int) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pidfd is supported only on Linux",
        ))
    }

    pub fn is_running(&self) -> io::Result<bool> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pidfd is supported only on Linux",
        ))
    }
}

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

    #[cfg(target_os = "linux")]
    #[test]
    fn pidfd_tracks_specific_process_lifetime_and_signal() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pidfd = PidFd::open(child.id()).unwrap();
        assert!(pidfd.is_running().unwrap());
        pidfd.send_signal(libc::SIGTERM).unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
        assert!(!pidfd.is_running().unwrap());
    }
}
