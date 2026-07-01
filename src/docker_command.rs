use std::env;
use std::io::{self, Read, Write};
use std::process::Output;
use std::thread;

const DOCKER_BIN_ENV: &str = "DOCKER_GIT_ANDROID_DOCKER";

pub fn copy_to_stderr<R>(mut reader: R) -> thread::JoinHandle<io::Result<()>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut stderr = io::stderr().lock();
        io::copy(&mut reader, &mut stderr)?;
        stderr.flush()
    })
}

pub fn docker_error(args: &[String], output: &Output) -> String {
    command_error(&format!("docker {}", args.join(" ")), output)
}

pub fn command_error(label: &str, output: &Output) -> String {
    format!(
        "{label} failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

pub fn docker_binary() -> String {
    env::var(DOCKER_BIN_ENV).unwrap_or_else(|_| "docker".to_string())
}
