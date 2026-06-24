use clap::{Args, Parser, Subcommand};
use docker_git_android_connection::{
    android_resource_limits, android_spec, docker_run_args, docker_stop_args, no_vnc_endpoint,
    parse_docker_no_vnc_port, AndroidResourceLimits, NoVncEndpoint, DEFAULT_ADB_ENDPOINT,
    DEFAULT_ANDROID_CPUS, DEFAULT_ANDROID_IMAGE, DEFAULT_ANDROID_MEMORY_LIMIT,
    DEFAULT_ANDROID_MEMORY_SWAP_LIMIT, DEFAULT_NOVNC_CONTAINER_PORT, DEFAULT_NOVNC_HOST,
    DEFAULT_NOVNC_PORT, DEFAULT_PROJECT_ID,
};
use serde_json::{json, Value};
use std::env;
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::process::{Command, ExitCode, Output, Stdio};
use std::thread;

const DOCKER_BIN_ENV: &str = "DOCKER_GIT_ANDROID_DOCKER";
const NO_VNC_PORT_FALLBACK_SPAN: u16 = 100;

#[derive(Parser, Debug)]
#[command(version, about = "docker-git Android runtime lifecycle CLI")]
struct Cli {
    #[command(subcommand)]
    command: LifecycleCommand,
}

#[derive(Subcommand, Debug)]
enum LifecycleCommand {
    Start(LifecycleArgs),
    Status(LifecycleArgs),
    Stop(LifecycleArgs),
}

#[derive(Args, Clone, Debug)]
struct LifecycleArgs {
    #[arg(long, default_value = DEFAULT_PROJECT_ID)]
    project: String,
    #[arg(long, default_value = "docker-git-shared")]
    network: String,
    #[arg(long, default_value = DEFAULT_ADB_ENDPOINT)]
    endpoint: String,
    #[arg(long, default_value = DEFAULT_ANDROID_IMAGE)]
    image: String,
    #[arg(long = "memory", default_value = DEFAULT_ANDROID_MEMORY_LIMIT, value_parser = parse_docker_size)]
    memory: String,
    #[arg(long = "memory-swap", default_value = DEFAULT_ANDROID_MEMORY_SWAP_LIMIT, value_parser = parse_docker_size)]
    memory_swap: String,
    #[arg(long = "cpus", default_value = DEFAULT_ANDROID_CPUS, value_parser = parse_cpus)]
    cpus: String,
    #[arg(long = "novnc-bind-host", default_value = DEFAULT_NOVNC_HOST)]
    novnc_bind_host: String,
    #[arg(long = "novnc-host", default_value = DEFAULT_NOVNC_HOST)]
    novnc_host: String,
    #[arg(
        long = "novnc-port",
        default_value_t = DEFAULT_NOVNC_PORT,
        value_parser = parse_no_vnc_port
    )]
    novnc_port: u16,
    #[arg(long)]
    no_novnc_publish: bool,
    #[arg(long)]
    dry_run: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        LifecycleCommand::Start(args) => start(&args),
        LifecycleCommand::Status(args) => status(&args),
        LifecycleCommand::Stop(args) => stop(&args),
    }
}

fn start(args: &LifecycleArgs) -> Result<(), Box<dyn std::error::Error>> {
    let spec = android_spec(&args.project, &args.network, &args.endpoint, &args.image)?;
    let no_vnc = configured_no_vnc(args, !args.dry_run)?;
    let resource_limits = configured_resource_limits(args);
    let docker_args = docker_run_args(&spec, no_vnc.as_ref(), &resource_limits);
    if args.dry_run {
        print_json(&with_docker_args(
            lifecycle_output(&spec, no_vnc.as_ref(), &resource_limits, None, None),
            &docker_args,
        ))?;
        return Ok(());
    }

    ensure_image_available(&spec.image)?;
    let container_id = run_docker_capture_stdout(&docker_args)?;
    print_json(&lifecycle_output(
        &spec,
        no_vnc.as_ref(),
        &resource_limits,
        Some(container_id.trim()),
        None,
    ))
}

fn status(args: &LifecycleArgs) -> Result<(), Box<dyn std::error::Error>> {
    let spec = android_spec(&args.project, &args.network, &args.endpoint, &args.image)?;
    let resource_limits = configured_resource_limits(args);
    let no_vnc = if args.no_novnc_publish {
        None
    } else {
        docker_no_vnc_endpoint(&spec.android_container_name, &args.novnc_host)
            .or(configured_no_vnc(args, false)?)
    };
    print_json(&lifecycle_output(
        &spec,
        no_vnc.as_ref(),
        &resource_limits,
        None,
        None,
    ))
}

fn stop(args: &LifecycleArgs) -> Result<(), Box<dyn std::error::Error>> {
    let spec = android_spec(&args.project, &args.network, &args.endpoint, &args.image)?;
    let resource_limits = configured_resource_limits(args);
    let docker_args = docker_stop_args(&spec);
    if args.dry_run {
        print_json(&with_docker_args(
            lifecycle_output(&spec, None, &resource_limits, None, None),
            &docker_args,
        ))?;
        return Ok(());
    }

    run_docker_capture_stdout(&docker_args)?;
    print_json(&lifecycle_output(
        &spec,
        None,
        &resource_limits,
        None,
        Some(true),
    ))
}

fn parse_docker_size(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("Docker size must not be empty".to_string());
    }

    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return Err(format!("Docker size {value:?} must start with a number"));
    }

    let suffix = &trimmed[digits..];
    if suffix.is_empty()
        || matches!(
            suffix,
            "b" | "B" | "k" | "K" | "m" | "M" | "g" | "G" | "kb" | "KB" | "mb" | "MB" | "gb" | "GB"
        )
    {
        Ok(trimmed.to_string())
    } else {
        Err(format!(
            "Docker size {value:?} must use bytes or b/k/m/g suffix"
        ))
    }
}

fn parse_cpus(value: &str) -> Result<String, String> {
    let cpus = value
        .parse::<f64>()
        .map_err(|error| format!("invalid CPU limit {value:?}: {error}"))?;
    if cpus.is_finite() && cpus > 0.0 {
        Ok(value.to_string())
    } else {
        Err("CPU limit must be a positive finite number".to_string())
    }
}

fn parse_no_vnc_port(value: &str) -> Result<u16, String> {
    let port = value
        .parse::<u16>()
        .map_err(|error| format!("invalid noVNC port {value:?}: {error}"))?;
    if port == 0 {
        Err("noVNC port must be in 1..=65535".to_string())
    } else {
        Ok(port)
    }
}

fn configured_resource_limits(args: &LifecycleArgs) -> AndroidResourceLimits {
    android_resource_limits(&args.memory, &args.memory_swap, &args.cpus)
}

fn configured_no_vnc(
    args: &LifecycleArgs,
    reserve_free_port: bool,
) -> Result<Option<NoVncEndpoint>, Box<dyn std::error::Error>> {
    if args.no_novnc_publish {
        return Ok(None);
    }

    let host_port = if reserve_free_port {
        first_available_port(&args.novnc_bind_host, args.novnc_port)?
    } else {
        args.novnc_port
    };
    Ok(Some(no_vnc_endpoint(
        &args.novnc_bind_host,
        &args.novnc_host,
        host_port,
    )))
}

fn first_available_port(bind_host: &str, requested_port: u16) -> Result<u16, String> {
    let last_port = requested_port.saturating_add(NO_VNC_PORT_FALLBACK_SPAN - 1);
    for port in requested_port..=last_port {
        if TcpListener::bind((bind_host, port)).is_ok() {
            return Ok(port);
        }
    }

    Err(format!(
        "no free noVNC port on {bind_host} in range {requested_port}..={last_port}"
    ))
}

fn docker_no_vnc_endpoint(container_name: &str, default_url_host: &str) -> Option<NoVncEndpoint> {
    let container_port = format!("{DEFAULT_NOVNC_CONTAINER_PORT}/tcp");
    let output = Command::new(docker_binary())
        .args(["port", container_name, &container_port])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    parse_docker_no_vnc_port(&String::from_utf8_lossy(&output.stdout), default_url_host)
}

fn lifecycle_output(
    spec: &docker_git_android_connection::AndroidSpec,
    no_vnc: Option<&NoVncEndpoint>,
    resource_limits: &AndroidResourceLimits,
    container_id: Option<&str>,
    removed: Option<bool>,
) -> Value {
    json!({
        "projectId": spec.project_id,
        "projectContainerName": spec.project_container_name,
        "androidContainerName": spec.android_container_name,
        "androidVolumeName": spec.android_volume_name,
        "dockerNetwork": spec.docker_network,
        "adbEndpoint": spec.adb_endpoint,
        "image": spec.image,
        "resourceLimits": resource_limits,
        "noVncPublished": no_vnc.is_some(),
        "noVncUrl": no_vnc.map(|endpoint| endpoint.url.as_str()),
        "noVncBindHost": no_vnc.map(|endpoint| endpoint.bind_host.as_str()),
        "noVncHost": no_vnc.map(|endpoint| endpoint.url_host.as_str()),
        "noVncPort": no_vnc.map(|endpoint| endpoint.host_port),
        "noVncContainerPort": no_vnc.map(|endpoint| endpoint.container_port),
        "containerId": container_id,
        "removed": removed
    })
}

fn with_docker_args(mut output: Value, docker_args: &[String]) -> Value {
    if let Value::Object(object) = &mut output {
        object.insert("docker".to_string(), json!(docker_args));
    }
    output
}

fn print_json(value: &Value) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn ensure_image_available(image: &str) -> Result<(), Box<dyn std::error::Error>> {
    let inspect_args = vec![
        "image".to_string(),
        "inspect".to_string(),
        image.to_string(),
    ];
    if run_docker_status(&inspect_args).is_ok() {
        return Ok(());
    }

    let pull_args = vec!["pull".to_string(), image.to_string()];
    run_docker_status_streaming_to_stderr(&pull_args)
}

fn run_docker_status(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(docker_binary()).args(args).output()?;
    if output.status.success() {
        return Ok(());
    }

    Err(docker_error(args, &output).into())
}

fn run_docker_capture_stdout(args: &[String]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new(docker_binary()).args(args).output()?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }

    Err(docker_error(args, &output).into())
}

fn run_docker_status_streaming_to_stderr(
    args: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new(docker_binary())
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().map(copy_to_stderr);
    let stderr = child.stderr.take().map(copy_to_stderr);
    let status = child.wait()?;

    if let Some(handle) = stdout {
        handle
            .join()
            .map_err(|_| "failed to join docker stdout")??;
    }
    if let Some(handle) = stderr {
        handle
            .join()
            .map_err(|_| "failed to join docker stderr")??;
    }

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "docker {} failed with status {:?}",
            args.join(" "),
            status.code()
        )
        .into())
    }
}

fn copy_to_stderr<R>(mut reader: R) -> thread::JoinHandle<io::Result<()>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut stderr = io::stderr().lock();
        io::copy(&mut reader, &mut stderr)?;
        stderr.flush()
    })
}

fn docker_error(args: &[String], output: &Output) -> String {
    format!(
        "docker {} failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn docker_binary() -> String {
    env::var(DOCKER_BIN_ENV).unwrap_or_else(|_| "docker".to_string())
}
