use clap::{Args, Parser, Subcommand};
use docker_git_android_connection::{
    android_resource_limits, android_runtime_options, android_spec, docker_run_args,
    docker_stop_args, no_vnc_endpoint, parse_docker_no_vnc_port, AndroidResourceLimits,
    AndroidRuntimeOptions, NoVncEndpoint, APP_TEST_ANDROID_RUNTIME_PROFILE,
    APP_TEST_VNC_ANDROID_RUNTIME_PROFILE, DEFAULT_ADB_ENDPOINT, DEFAULT_ANDROID_CPUS,
    DEFAULT_ANDROID_IMAGE, DEFAULT_ANDROID_MEMORY_LIMIT, DEFAULT_ANDROID_MEMORY_SWAP_LIMIT,
    DEFAULT_ANDROID_RUNTIME_PROFILE, DEFAULT_NOVNC_CONTAINER_PORT, DEFAULT_NOVNC_HOST,
    DEFAULT_NOVNC_PORT,
};
use serde_json::{json, Value};
use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, ExitCode, Output, Stdio};

mod docker_command;
mod web_bridge;

use docker_command::{command_error, copy_to_stderr, docker_binary, docker_error};
use web_bridge::{phone, web, PhoneArgs, WebArgs, PHONE_COMMAND_NAME, WEB_COMMAND_NAME};

const NO_VNC_PORT_FALLBACK_SPAN: u16 = 100;
const DEFAULT_ADB_MODE: &str = "container";
const CONTAINER_ADB_SERIAL: &str = "emulator-5554";
const CONTAINER_INSTALL_APK_PATH: &str = "/tmp/docker-git-install.apk";

#[derive(Parser, Debug)]
#[command(
    version,
    about = "rust-android-connection lifecycle, ADB proxy, and browser WebUSB CLI",
    after_help = "Global browser UI: rust-android-connection web --port 8080"
)]
struct Cli {
    #[arg(value_name = "PROJECT")]
    project: String,
    #[command(subcommand)]
    command: LifecycleCommand,
}

#[derive(Subcommand, Debug)]
enum LifecycleCommand {
    Start(LifecycleArgs),
    Status(LifecycleArgs),
    Stop(LifecycleArgs),
    Adb(AdbArgs),
    InstallApk(InstallApkArgs),
    LaunchApp(LaunchAppArgs),
}

#[derive(Args, Clone, Debug)]
struct AndroidConnectionArgs {
    #[arg(long, default_value = "docker-git-shared")]
    network: String,
    #[arg(long, default_value = DEFAULT_ADB_ENDPOINT)]
    endpoint: String,
    #[arg(long, default_value = DEFAULT_ANDROID_IMAGE)]
    image: String,
}

#[derive(Args, Clone, Debug)]
struct LifecycleArgs {
    #[command(flatten)]
    connection: AndroidConnectionArgs,
    #[arg(long = "memory", default_value = DEFAULT_ANDROID_MEMORY_LIMIT, value_parser = parse_docker_size)]
    memory: String,
    #[arg(long = "memory-swap", default_value = DEFAULT_ANDROID_MEMORY_SWAP_LIMIT, value_parser = parse_docker_size)]
    memory_swap: String,
    #[arg(long = "cpus", default_value = DEFAULT_ANDROID_CPUS, value_parser = parse_cpus)]
    cpus: String,
    #[arg(long = "runtime-profile", default_value = DEFAULT_ANDROID_RUNTIME_PROFILE, value_parser = parse_runtime_profile)]
    runtime_profile: String,
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

#[derive(Args, Clone, Debug)]
#[command(disable_help_flag = true)]
struct AdbArgs {
    #[command(flatten)]
    connection: AndroidConnectionArgs,
    #[arg(long = "adb-mode", default_value = DEFAULT_ADB_MODE, value_parser = parse_adb_mode)]
    adb_mode: AdbMode,
    #[arg(long)]
    dry_run: bool,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Args, Clone, Debug)]
struct InstallApkArgs {
    #[command(flatten)]
    connection: AndroidConnectionArgs,
    #[arg(long = "adb-mode", default_value = DEFAULT_ADB_MODE, value_parser = parse_adb_mode)]
    adb_mode: AdbMode,
    #[arg(long)]
    dry_run: bool,
    path: String,
}

#[derive(Args, Clone, Debug)]
struct LaunchAppArgs {
    #[command(flatten)]
    connection: AndroidConnectionArgs,
    #[arg(long = "adb-mode", default_value = DEFAULT_ADB_MODE, value_parser = parse_adb_mode)]
    adb_mode: AdbMode,
    #[arg(long)]
    dry_run: bool,
    #[arg(long = "package")]
    package_name: String,
    #[arg(long)]
    activity: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdbMode {
    Auto,
    Host,
    Container,
}

impl AdbMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Host => "host",
            Self::Container => "container",
        }
    }
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
    let raw_args = env::args_os().collect::<Vec<_>>();
    if raw_args
        .get(1)
        .is_some_and(|argument| argument == OsStr::new(WEB_COMMAND_NAME))
    {
        let mut web_args = Vec::<OsString>::with_capacity(raw_args.len().saturating_sub(1));
        if let Some(binary) = raw_args.first() {
            web_args.push(binary.clone());
        }
        web_args.extend(raw_args.into_iter().skip(2));
        return web(&WebArgs::parse_from(web_args));
    }
    if raw_args
        .get(1)
        .is_some_and(|argument| argument == OsStr::new(PHONE_COMMAND_NAME))
    {
        let mut phone_args = Vec::<OsString>::with_capacity(raw_args.len().saturating_sub(1));
        if let Some(binary) = raw_args.first() {
            phone_args.push(binary.clone());
        }
        phone_args.extend(raw_args.into_iter().skip(2));
        return phone(&PhoneArgs::parse_from(phone_args));
    }

    let cli = Cli::parse_from(raw_args);
    let project = cli.project;
    match cli.command {
        LifecycleCommand::Start(args) => start(&project, &args),
        LifecycleCommand::Status(args) => status(&project, &args),
        LifecycleCommand::Stop(args) => stop(&project, &args),
        LifecycleCommand::Adb(args) => adb(&project, &args),
        LifecycleCommand::InstallApk(args) => install_apk(&project, &args),
        LifecycleCommand::LaunchApp(args) => launch_app(&project, &args),
    }
}

fn start(project: &str, args: &LifecycleArgs) -> Result<(), Box<dyn std::error::Error>> {
    let spec = configured_spec(project, &args.connection)?;
    let runtime_options = configured_runtime_options(args)?;
    let no_vnc = configured_no_vnc(args, &runtime_options, !args.dry_run)?;
    let resource_limits = configured_resource_limits(args);
    let docker_args = docker_run_args(&spec, no_vnc.as_ref(), &resource_limits, &runtime_options);
    if args.dry_run {
        print_json(&with_docker_args(
            lifecycle_output(
                &spec,
                no_vnc.as_ref(),
                &resource_limits,
                &runtime_options,
                None,
                None,
            ),
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
        &runtime_options,
        Some(container_id.trim()),
        None,
    ))
}

fn status(project: &str, args: &LifecycleArgs) -> Result<(), Box<dyn std::error::Error>> {
    let spec = configured_spec(project, &args.connection)?;
    let runtime_options = configured_runtime_options(args)?;
    let resource_limits = configured_resource_limits(args);
    let no_vnc = if args.no_novnc_publish || !runtime_options.web_vnc_enabled.as_bool() {
        None
    } else {
        docker_no_vnc_endpoint(&spec.android_container_name, &args.novnc_host)
            .or(configured_no_vnc(args, &runtime_options, false)?)
    };
    print_json(&lifecycle_output(
        &spec,
        no_vnc.as_ref(),
        &resource_limits,
        &runtime_options,
        None,
        None,
    ))
}

fn stop(project: &str, args: &LifecycleArgs) -> Result<(), Box<dyn std::error::Error>> {
    let spec = configured_spec(project, &args.connection)?;
    let runtime_options = configured_runtime_options(args)?;
    let resource_limits = configured_resource_limits(args);
    let docker_args = docker_stop_args(&spec);
    if args.dry_run {
        print_json(&with_docker_args(
            lifecycle_output(&spec, None, &resource_limits, &runtime_options, None, None),
            &docker_args,
        ))?;
        return Ok(());
    }

    run_docker_capture_stdout(&docker_args)?;
    print_json(&lifecycle_output(
        &spec,
        None,
        &resource_limits,
        &runtime_options,
        None,
        Some(true),
    ))
}

fn adb(project: &str, args: &AdbArgs) -> Result<(), Box<dyn std::error::Error>> {
    let spec = configured_spec(project, &args.connection)?;
    if args.dry_run {
        print_json(&adb_proxy_dry_run_output(&spec, args.adb_mode, &args.args))?;
        return Ok(());
    }

    let output = run_adb_proxy_command(&spec, args.adb_mode, &args.args)?;
    write_process_output(&output)?;
    if output.status.success() {
        Ok(())
    } else {
        std::process::exit(output.status.code().unwrap_or(1));
    }
}

fn install_apk(project: &str, args: &InstallApkArgs) -> Result<(), Box<dyn std::error::Error>> {
    let spec = configured_spec(project, &args.connection)?;
    if args.dry_run {
        print_json(&install_apk_dry_run_output(
            &spec,
            args.adb_mode,
            Path::new(&args.path),
        ))?;
        return Ok(());
    }

    let output = run_install_apk(&spec, args.adb_mode, Path::new(&args.path))?;
    write_process_output(&output)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error("adb install", &output).into())
    }
}

fn launch_app(project: &str, args: &LaunchAppArgs) -> Result<(), Box<dyn std::error::Error>> {
    let spec = configured_spec(project, &args.connection)?;
    let adb_args = launch_app_adb_args(&args.package_name, args.activity.as_deref());
    if args.dry_run {
        print_json(&targeted_adb_dry_run_output(
            &spec,
            args.adb_mode,
            &adb_args,
        ))?;
        return Ok(());
    }

    let output = run_targeted_adb_command(&spec, args.adb_mode, &adb_args)?;
    write_process_output(&output)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error("adb launch", &output).into())
    }
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

fn parse_runtime_profile(value: &str) -> Result<String, String> {
    android_runtime_options(value)
        .map(|_| value.to_string())
        .ok_or_else(|| {
            format!(
                "runtime profile must be one of: {DEFAULT_ANDROID_RUNTIME_PROFILE}, {APP_TEST_ANDROID_RUNTIME_PROFILE}, {APP_TEST_VNC_ANDROID_RUNTIME_PROFILE}"
            )
        })
}

fn parse_adb_mode(value: &str) -> Result<AdbMode, String> {
    match value {
        "auto" => Ok(AdbMode::Auto),
        "host" => Ok(AdbMode::Host),
        "container" => Ok(AdbMode::Container),
        _ => Err("ADB mode must be one of: auto, host, container".to_string()),
    }
}

fn configured_spec(
    project: &str,
    args: &AndroidConnectionArgs,
) -> Result<docker_git_android_connection::AndroidSpec, docker_git_android_connection::EndpointError>
{
    android_spec(project, &args.network, &args.endpoint, &args.image)
}

fn configured_resource_limits(args: &LifecycleArgs) -> AndroidResourceLimits {
    android_resource_limits(&args.memory, &args.memory_swap, &args.cpus)
}

fn configured_runtime_options(
    args: &LifecycleArgs,
) -> Result<AndroidRuntimeOptions, Box<dyn std::error::Error>> {
    android_runtime_options(&args.runtime_profile)
        .ok_or_else(|| format!("invalid runtime profile {:?}", args.runtime_profile).into())
}

fn configured_no_vnc(
    args: &LifecycleArgs,
    runtime_options: &AndroidRuntimeOptions,
    reserve_free_port: bool,
) -> Result<Option<NoVncEndpoint>, Box<dyn std::error::Error>> {
    if args.no_novnc_publish || !runtime_options.web_vnc_enabled.as_bool() {
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
    runtime_options: &AndroidRuntimeOptions,
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
        "runtime": runtime_options,
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

fn adb_proxy_dry_run_output(
    spec: &docker_git_android_connection::AndroidSpec,
    mode: AdbMode,
    adb_args: &[String],
) -> Value {
    json!({
        "projectId": spec.project_id,
        "androidContainerName": spec.android_container_name,
        "adbEndpoint": spec.adb_endpoint,
        "adbMode": mode.as_str(),
        "host": host_adb_proxy_command(&spec.adb_endpoint, adb_args),
        "container": container_adb_proxy_command(&spec.android_container_name, adb_args)
    })
}

fn targeted_adb_dry_run_output(
    spec: &docker_git_android_connection::AndroidSpec,
    mode: AdbMode,
    adb_args: &[String],
) -> Value {
    json!({
        "projectId": spec.project_id,
        "androidContainerName": spec.android_container_name,
        "adbEndpoint": spec.adb_endpoint,
        "adbMode": mode.as_str(),
        "host": host_targeted_adb_command(&spec.adb_endpoint, adb_args),
        "container": container_targeted_adb_command(&spec.android_container_name, adb_args)
    })
}

fn install_apk_dry_run_output(
    spec: &docker_git_android_connection::AndroidSpec,
    mode: AdbMode,
    apk_path: &Path,
) -> Value {
    json!({
        "projectId": spec.project_id,
        "androidContainerName": spec.android_container_name,
        "adbEndpoint": spec.adb_endpoint,
        "adbMode": mode.as_str(),
        "host": host_targeted_adb_command(
            &spec.adb_endpoint,
            &["install".to_string(), apk_path.display().to_string()]
        ),
        "containerCopy": docker_cp_args(apk_path, &spec.android_container_name),
        "container": container_targeted_adb_command(
            &spec.android_container_name,
            &["install".to_string(), CONTAINER_INSTALL_APK_PATH.to_string()]
        )
    })
}

fn run_adb_proxy_command(
    spec: &docker_git_android_connection::AndroidSpec,
    mode: AdbMode,
    adb_args: &[String],
) -> Result<Output, Box<dyn std::error::Error>> {
    match mode {
        AdbMode::Host => run_host_adb_proxy(&spec.adb_endpoint, adb_args),
        AdbMode::Container => run_container_adb_proxy(&spec.android_container_name, adb_args),
        AdbMode::Auto => {
            if host_adb_ready(&spec.adb_endpoint) {
                run_host_adb_proxy(&spec.adb_endpoint, adb_args)
            } else {
                run_container_adb_proxy(&spec.android_container_name, adb_args)
            }
        }
    }
}

fn run_targeted_adb_command(
    spec: &docker_git_android_connection::AndroidSpec,
    mode: AdbMode,
    adb_args: &[String],
) -> Result<Output, Box<dyn std::error::Error>> {
    match mode {
        AdbMode::Host => run_host_targeted_adb(&spec.adb_endpoint, adb_args),
        AdbMode::Container => run_container_targeted_adb(&spec.android_container_name, adb_args),
        AdbMode::Auto => {
            if host_adb_ready(&spec.adb_endpoint) {
                run_host_targeted_adb(&spec.adb_endpoint, adb_args)
            } else {
                run_container_targeted_adb(&spec.android_container_name, adb_args)
            }
        }
    }
}

fn run_install_apk(
    spec: &docker_git_android_connection::AndroidSpec,
    mode: AdbMode,
    apk_path: &Path,
) -> Result<Output, Box<dyn std::error::Error>> {
    match mode {
        AdbMode::Host => run_host_targeted_adb(
            &spec.adb_endpoint,
            &["install".to_string(), apk_path.display().to_string()],
        ),
        AdbMode::Container => run_container_install_apk(&spec.android_container_name, apk_path),
        AdbMode::Auto => {
            if host_adb_ready(&spec.adb_endpoint) {
                run_host_targeted_adb(
                    &spec.adb_endpoint,
                    &["install".to_string(), apk_path.display().to_string()],
                )
            } else {
                run_container_install_apk(&spec.android_container_name, apk_path)
            }
        }
    }
}

fn run_container_install_apk(
    container_name: &str,
    apk_path: &Path,
) -> Result<Output, Box<dyn std::error::Error>> {
    let copy_args = docker_cp_args(apk_path, container_name);
    run_docker_status(&copy_args)?;
    run_container_targeted_adb(
        container_name,
        &[
            "install".to_string(),
            CONTAINER_INSTALL_APK_PATH.to_string(),
        ],
    )
}

fn run_host_adb_proxy(
    endpoint: &str,
    adb_args: &[String],
) -> Result<Output, Box<dyn std::error::Error>> {
    let connect_output = Command::new("adb").arg("connect").arg(endpoint).output()?;
    if !connect_output.status.success() {
        return Err(command_error("adb connect", &connect_output).into());
    }

    Command::new("adb")
        .args(adb_args)
        .output()
        .map_err(Into::into)
}

fn run_host_targeted_adb(
    endpoint: &str,
    adb_args: &[String],
) -> Result<Output, Box<dyn std::error::Error>> {
    let connect_output = Command::new("adb").arg("connect").arg(endpoint).output()?;
    if !connect_output.status.success() {
        return Err(command_error("adb connect", &connect_output).into());
    }

    Command::new("adb")
        .args(host_targeted_adb_args(endpoint, adb_args))
        .output()
        .map_err(Into::into)
}

fn host_adb_ready(endpoint: &str) -> bool {
    Command::new("adb")
        .arg("connect")
        .arg(endpoint)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn run_container_adb_proxy(
    container_name: &str,
    adb_args: &[String],
) -> Result<Output, Box<dyn std::error::Error>> {
    let docker_args = container_adb_proxy_args(container_name, adb_args);
    Command::new(docker_binary())
        .args(docker_args)
        .output()
        .map_err(Into::into)
}

fn run_container_targeted_adb(
    container_name: &str,
    adb_args: &[String],
) -> Result<Output, Box<dyn std::error::Error>> {
    let docker_args = container_targeted_adb_args(container_name, adb_args);
    Command::new(docker_binary())
        .args(docker_args)
        .output()
        .map_err(Into::into)
}

fn launch_app_adb_args(package_name: &str, activity: Option<&str>) -> Vec<String> {
    match activity {
        Some(activity) if !activity.is_empty() => vec![
            "shell".to_string(),
            "am".to_string(),
            "start".to_string(),
            "-n".to_string(),
            format!("{package_name}/{activity}"),
        ],
        _ => vec![
            "shell".to_string(),
            "monkey".to_string(),
            "-p".to_string(),
            package_name.to_string(),
            "-c".to_string(),
            "android.intent.category.LAUNCHER".to_string(),
            "1".to_string(),
        ],
    }
}

fn host_adb_proxy_command(endpoint: &str, adb_args: &[String]) -> Vec<String> {
    let adb_command = if adb_args.is_empty() {
        "adb".to_string()
    } else {
        format!("adb {}", shell_join(adb_args))
    };
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("adb connect {} && {adb_command}", shell_quote(endpoint)),
    ]
}

fn host_targeted_adb_command(endpoint: &str, adb_args: &[String]) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "adb connect {} && adb {}",
            shell_quote(endpoint),
            shell_join(&host_targeted_adb_args(endpoint, adb_args))
        ),
    ]
}

fn host_targeted_adb_args(endpoint: &str, adb_args: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    if should_add_host_serial(adb_args) {
        args.extend(["-s".to_string(), endpoint.to_string()]);
    }
    args.extend(adb_args.iter().cloned());
    args
}

fn container_adb_proxy_command(container_name: &str, adb_args: &[String]) -> Vec<String> {
    let mut command = vec!["docker".to_string()];
    command.extend(container_adb_proxy_args(container_name, adb_args));
    command
}

fn container_targeted_adb_command(container_name: &str, adb_args: &[String]) -> Vec<String> {
    let mut command = vec!["docker".to_string()];
    command.extend(container_targeted_adb_args(container_name, adb_args));
    command
}

fn container_adb_proxy_args(container_name: &str, adb_args: &[String]) -> Vec<String> {
    let mut docker_args = vec![
        "exec".to_string(),
        container_name.to_string(),
        "adb".to_string(),
    ];
    docker_args.extend(adb_args.iter().cloned());
    docker_args
}

fn container_targeted_adb_args(container_name: &str, adb_args: &[String]) -> Vec<String> {
    let mut docker_args = vec![
        "exec".to_string(),
        container_name.to_string(),
        "adb".to_string(),
    ];
    if should_add_container_serial(adb_args) {
        docker_args.extend(["-s".to_string(), CONTAINER_ADB_SERIAL.to_string()]);
    }
    docker_args.extend(adb_args.iter().cloned());
    docker_args
}

fn docker_cp_args(apk_path: &Path, container_name: &str) -> Vec<String> {
    vec![
        "cp".to_string(),
        apk_path.display().to_string(),
        format!("{container_name}:{CONTAINER_INSTALL_APK_PATH}"),
    ]
}

fn should_add_container_serial(adb_args: &[String]) -> bool {
    !matches!(adb_args.first().map(String::as_str), Some("devices"))
        && !adb_args.iter().any(|argument| argument == "-s")
}

fn should_add_host_serial(adb_args: &[String]) -> bool {
    should_add_container_serial(adb_args)
}

fn shell_join(values: &[String]) -> String {
    values
        .iter()
        .map(|value| shell_quote(value))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn write_process_output(output: &Output) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(&output.stdout)?;
    stdout.flush()?;
    let mut stderr = io::stderr().lock();
    stderr.write_all(&output.stderr)?;
    stderr.flush()
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
