//! Native test-only launcher for the adjacent JavaScript provider fixture.
// Preserves fixture arguments and standard input/output streams.
use std::{env, fs, io, process::{Command, ExitCode}};

/// Runs the adjacent fixture with the test runner's Node executable.
fn run() -> io::Result<i32> {
    let executable = env::current_exe()?;
    let script = executable.with_extension("mjs");
    let node = fs::read_to_string(executable.with_extension("node-path"))?;
    let status = Command::new(node).arg(script).args(env::args_os().skip(1)).status()?;
    Ok(status.code().unwrap_or(1))
}

/// Reports fixture startup failures without inventing a successful provider response.
fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(error) => { eprintln!("Provider fixture failed: {error}"); ExitCode::FAILURE }
    }
}
