use clap::{Parser, ValueEnum};
use qwe_runtime::{LandlockPolicy, apply_landlock};
use std::ffi::CString;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WorkspaceMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Parser)]
#[command(version, about = "Internal QWE Landlock sandbox initializer")]
struct Args {
    #[arg(long)]
    workspace: PathBuf,
    #[arg(long, value_enum)]
    workspace_mode: WorkspaceMode,
    #[arg(long)]
    private_tmp: bool,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let abi = apply_landlock(&LandlockPolicy {
        workspace: &args.workspace,
        workspace_read_write: matches!(args.workspace_mode, WorkspaceMode::ReadWrite),
        private_tmp: args.private_tmp,
    })?;
    unsafe {
        std::env::set_var("QWE_LANDLOCK_ABI", abi.to_string());
    }

    let program = CString::new(args.command[0].as_bytes())?;
    let arguments = args
        .command
        .iter()
        .map(|value| CString::new(value.as_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut pointers = arguments
        .iter()
        .map(|value| value.as_ptr())
        .collect::<Vec<_>>();
    pointers.push(std::ptr::null());
    unsafe {
        libc::execvp(program.as_ptr(), pointers.as_ptr());
    }
    let failure = std::io::Error::last_os_error();
    Err(format!("failed to exec {}: {}", args.command[0], failure).into())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run(Args::parse())
}
