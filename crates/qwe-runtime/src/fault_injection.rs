use std::io;

pub const SANDBOX_FAULT_ENV: &str = "QWE_SANDBOX_FAULT_STAGE";

pub fn check_sandbox_fault(stage: &str) -> io::Result<()> {
    #[cfg(feature = "fault-injection")]
    {
        if std::env::var_os(SANDBOX_FAULT_ENV)
            .as_deref()
            .is_some_and(|value| value == std::ffi::OsStr::new(stage))
        {
            return Err(io::Error::other(format!(
                "injected sandbox setup failure at {stage}"
            )));
        }
    }
    let _ = stage;
    Ok(())
}
