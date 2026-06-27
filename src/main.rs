use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DOCKER_BIN_ENV: &str = "DOCKER_GIT_ANDROID_DOCKER";
const NO_VNC_PORT_FALLBACK_SPAN: u16 = 100;
const DEFAULT_ADB_MODE: &str = "container";
const CONTAINER_ADB_SERIAL: &str = "emulator-5554";
const CONTAINER_INSTALL_APK_PATH: &str = "/tmp/docker-git-install.apk";
const WEB_COMMAND_NAME: &str = "web";
const PHONE_COMMAND_NAME: &str = "phone";
const DEFAULT_WEB_BIND_HOST: &str = "127.0.0.1";
const DEFAULT_WEB_PORT: u16 = 8080;
const DEFAULT_PHONE_BRIDGE_URL: &str = "http://127.0.0.1:8080";
const WEB_INDEX_HTML: &str = include_str!("web/index.html");
const WEB_APP_JS: &str = include_str!("web/app.js");
const WEB_STYLES_CSS: &str = include_str!("web/styles.css");
const MAX_WEB_CLIENT_LOG_BYTES: usize = 16 * 1024;
const MAX_WEB_BRIDGE_BODY_BYTES: usize = 32 * 1024 * 1024;
const PHONE_BRIDGE_TIMEOUT: Duration = Duration::from_secs(900);
const PHONE_BRIDGE_POLL_INTERVAL: Duration = Duration::from_millis(500);
const BRIDGE_CLIENT_ID_HEADER: &str = "x-bridge-client-id";

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

#[derive(Parser, Debug)]
#[command(
    name = "rust-android-connection web",
    about = "Serve the browser WebUSB/WebADB phone connector"
)]
struct WebArgs {
    #[arg(long = "bind", default_value = DEFAULT_WEB_BIND_HOST)]
    bind_host: String,
    #[arg(long, default_value_t = DEFAULT_WEB_PORT, value_parser = parse_web_port)]
    port: u16,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Parser, Debug)]
#[command(
    name = "rust-android-connection phone",
    about = "Send commands to an Android phone connected through the browser WebUSB bridge"
)]
struct PhoneArgs {
    #[arg(long = "url", global = true, default_value = DEFAULT_PHONE_BRIDGE_URL)]
    url: String,
    #[command(subcommand)]
    command: PhoneCommand,
}

#[derive(Subcommand, Debug)]
enum PhoneCommand {
    Adb(PhoneAdbArgs),
}

#[derive(Args, Clone, Debug)]
#[command(disable_help_flag = true)]
struct PhoneAdbArgs {
    #[arg(long)]
    dry_run: bool,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
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

#[derive(Default)]
struct BridgeState {
    next_id: u64,
    next_client_id: u64,
    active_client_id: Option<String>,
    queue: VecDeque<BridgeCommand>,
    results: HashMap<String, BridgeCommandResult>,
    files: HashMap<String, PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeCommand {
    id: String,
    kind: String,
    args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<BridgeCommandFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeCommandFile {
    name: String,
    size: u64,
    url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeCommandRequest {
    kind: String,
    args: Vec<String>,
    #[serde(default)]
    file_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeCommandResult {
    id: String,
    exit_code: Option<i32>,
    stdout: Option<String>,
    stdout_base64: Option<String>,
    stderr: Option<String>,
    error: Option<String>,
}

#[derive(Debug)]
struct ParsedHttpUrl {
    host: String,
    port: u16,
    path: String,
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

fn phone(args: &PhoneArgs) -> Result<(), Box<dyn std::error::Error>> {
    match &args.command {
        PhoneCommand::Adb(adb_args) => phone_adb(&args.url, adb_args),
    }
}

fn phone_adb(url: &str, args: &PhoneAdbArgs) -> Result<(), Box<dyn std::error::Error>> {
    let file_path = phone_adb_file_attachment(&args.args)?;
    if args.dry_run {
        print_json(&json!({
            "url": url,
            "kind": "adb",
            "args": args.args,
            "filePath": file_path,
            "transport": "browser-webusb-bridge"
        }))?;
        return Ok(());
    }

    let result = run_phone_bridge_command(url, "adb", &args.args, file_path.as_deref())?;
    if let Some(error) = result.error.as_deref() {
        return Err(error.to_string().into());
    }
    if let Some(stderr) = result.stderr.as_deref() {
        io::stderr().write_all(stderr.as_bytes())?;
    }
    if let Some(stdout_base64) = result.stdout_base64.as_deref() {
        let bytes = BASE64_STANDARD.decode(stdout_base64)?;
        io::stdout().write_all(&bytes)?;
    } else if let Some(stdout) = result.stdout.as_deref() {
        io::stdout().write_all(stdout.as_bytes())?;
    }
    io::stdout().flush()?;
    io::stderr().flush()?;

    let exit_code = result.exit_code.unwrap_or(0);
    if exit_code == 0 {
        Ok(())
    } else {
        std::process::exit(exit_code);
    }
}

fn run_phone_bridge_command(
    url: &str,
    kind: &str,
    args: &[String],
    file_path: Option<&str>,
) -> Result<BridgeCommandResult, Box<dyn std::error::Error>> {
    let mut payload = json!({
        "kind": kind,
        "args": args,
    });
    if let Some(path) = file_path {
        payload["filePath"] = Value::String(path.to_string());
    }
    let body = serde_json::to_string(&payload)?;
    let enqueue_url = bridge_url(url, "/bridge/commands");
    let (status, response) = http_request_json("POST", &enqueue_url, Some(&body))?;
    if status != 200 {
        return Err(format!("bridge enqueue failed with HTTP {status}: {response}").into());
    }
    let id = serde_json::from_str::<Value>(&response)?
        .get("id")
        .and_then(Value::as_str)
        .ok_or("bridge response did not include command id")?
        .to_string();

    let started_at = Instant::now();
    let result_url = bridge_url(url, &format!("/bridge/commands/result/{id}"));
    while started_at.elapsed() < PHONE_BRIDGE_TIMEOUT {
        let (status, response) = http_request_json("GET", &result_url, None)?;
        if status == 200 {
            return Ok(serde_json::from_str::<BridgeCommandResult>(&response)?);
        }
        if status != 202 {
            return Err(format!("bridge result failed with HTTP {status}: {response}").into());
        }
        thread::sleep(PHONE_BRIDGE_POLL_INTERVAL);
    }

    Err(format!(
        "timed out waiting for phone bridge command {id}; keep the browser tab open and connected"
    )
    .into())
}

fn phone_adb_file_attachment(
    args: &[String],
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(command) = args.first() else {
        return Ok(None);
    };
    if command != "install" {
        return Ok(None);
    }
    let Some(path) = args.last() else {
        return Err("adb install requires an APK path".into());
    };
    if path.starts_with('-') {
        return Err("adb install requires a final APK path argument".into());
    }
    if !Path::new(path).is_file() {
        return Err(format!("APK file not found: {path}").into());
    }
    Ok(Some(path.to_string()))
}

fn bridge_url(base_url: &str, path: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), path)
}

fn http_request_json(
    method: &str,
    url: &str,
    body: Option<&str>,
) -> Result<(u16, String), Box<dyn std::error::Error>> {
    let parsed = parse_http_url(url)?;
    let mut stream = TcpStream::connect((parsed.host.as_str(), parsed.port))?;
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        parsed.path,
        parsed.host,
        body.len(),
        body
    );
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    parse_http_response(&response)
}

fn parse_http_url(url: &str) -> Result<ParsedHttpUrl, Box<dyn std::error::Error>> {
    let without_scheme = url
        .strip_prefix("http://")
        .ok_or("phone bridge only supports http:// URLs")?;
    let (authority, path) = without_scheme
        .split_once('/')
        .map_or((without_scheme, "/"), |(authority, _)| {
            (authority, &without_scheme[authority.len()..])
        });
    let (host, port) = authority.rsplit_once(':').map_or_else(
        || (authority.to_string(), DEFAULT_WEB_PORT),
        |(host, port)| {
            let parsed_port = port.parse::<u16>().unwrap_or(DEFAULT_WEB_PORT);
            (host.to_string(), parsed_port)
        },
    );
    if host.is_empty() {
        return Err("phone bridge URL host must not be empty".into());
    }
    Ok(ParsedHttpUrl {
        host,
        port,
        path: path.to_string(),
    })
}

fn parse_http_response(response: &[u8]) -> Result<(u16, String), Box<dyn std::error::Error>> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("invalid HTTP response: missing header terminator")?
        + 4;
    let headers = String::from_utf8_lossy(&response[..header_end]);
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or("invalid HTTP response: missing status code")?
        .parse::<u16>()?;
    let body = String::from_utf8_lossy(&response[header_end..]).into_owned();
    Ok((status, body))
}

fn web(args: &WebArgs) -> Result<(), Box<dyn std::error::Error>> {
    let url = web_url(&args.bind_host, args.port);
    if args.dry_run {
        print_json(&json!({
            "bindHost": args.bind_host,
            "port": args.port,
            "url": url,
            "secureContext": "localhost-or-https",
            "requiresAdb": false,
            "requiresBrowser": "Chromium WebUSB",
            "phoneBridge": true
        }))?;
        return Ok(());
    }

    let listener = TcpListener::bind((args.bind_host.as_str(), args.port))?;
    let bridge = Arc::new(Mutex::new(BridgeState::default()));
    eprintln!("rust-android-connection web: {url}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let bridge = Arc::clone(&bridge);
                thread::spawn(move || {
                    if let Err(error) = handle_web_client(stream, &bridge) {
                        eprintln!("web request failed: {error}");
                    }
                });
            }
            Err(error) => eprintln!("web accept failed: {error}"),
        }
    }
    Ok(())
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

fn parse_web_port(value: &str) -> Result<u16, String> {
    parse_no_vnc_port(value).map_err(|_| "web port must be in 1..=65535".to_string())
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
    command_error(&format!("docker {}", args.join(" ")), output)
}

fn command_error(label: &str, output: &Output) -> String {
    format!(
        "{label} failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn docker_binary() -> String {
    env::var(DOCKER_BIN_ENV).unwrap_or_else(|_| "docker".to_string())
}

fn web_url(bind_host: &str, port: u16) -> String {
    let host = match bind_host {
        "0.0.0.0" | "::" => DEFAULT_WEB_BIND_HOST,
        host => host,
    };
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    format!("http://{host}:{port}/")
}

fn handle_web_client(mut stream: TcpStream, bridge: &Arc<Mutex<BridgeState>>) -> io::Result<()> {
    let mut buffer = [0_u8; 8192];
    let bytes_read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let mut parts = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or("/");
    let route_path = path.split('?').next().unwrap_or("/");

    if method == "POST" && route_path == "/client-log" {
        let body = read_http_body(&mut stream, &buffer[..bytes_read], MAX_WEB_CLIENT_LOG_BYTES)?;
        let is_raw_usb_trace =
            body.contains(r#""message":"USB OUT "#) || body.contains(r#""message":"USB IN "#);
        if !is_raw_usb_trace {
            eprintln!("web client log: {body}");
        }
        return write_http_response(
            &mut stream,
            "204 No Content",
            "text/plain; charset=utf-8",
            "",
            false,
        );
    }

    if route_path.starts_with("/bridge/") {
        return handle_bridge_request(
            &mut stream,
            bridge,
            method,
            route_path,
            &buffer[..bytes_read],
        );
    }

    if !matches!(method, "GET" | "HEAD") {
        return write_http_response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            "method not allowed",
            method == "HEAD",
        );
    }

    let (status, content_type, body) = match route_path {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", WEB_INDEX_HTML),
        "/app.js" => ("200 OK", "text/javascript; charset=utf-8", WEB_APP_JS),
        "/styles.css" => ("200 OK", "text/css; charset=utf-8", WEB_STYLES_CSS),
        "/healthz" => ("200 OK", "text/plain; charset=utf-8", "ok\n"),
        _ => ("404 Not Found", "text/plain; charset=utf-8", "not found\n"),
    };
    write_http_response(&mut stream, status, content_type, body, method == "HEAD")
}

fn handle_bridge_request(
    stream: &mut TcpStream,
    bridge: &Arc<Mutex<BridgeState>>,
    method: &str,
    route_path: &str,
    initial: &[u8],
) -> io::Result<()> {
    match (method, route_path) {
        ("POST", "/bridge/client/session") => {
            let client_id = {
                let mut bridge = bridge
                    .lock()
                    .map_err(|_| io::Error::new(io::ErrorKind::Other, "bridge state poisoned"))?;
                bridge.next_client_id += 1;
                let client_id = format!("client-{}", bridge.next_client_id);
                bridge.active_client_id = Some(client_id.clone());
                client_id
            };
            let body = serde_json::to_string(&json!({ "clientId": client_id }))?;
            write_http_response(
                stream,
                "200 OK",
                "application/json; charset=utf-8",
                &body,
                false,
            )
        }
        ("POST", "/bridge/commands") => {
            let body = read_http_body(stream, initial, MAX_WEB_BRIDGE_BODY_BYTES)?;
            let request = serde_json::from_str::<BridgeCommandRequest>(&body).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid bridge command: {error}"),
                )
            })?;
            if request.file_path.is_some()
                && !is_local_http_host(http_header_value(initial, "host").as_deref())
            {
                return write_http_response(
                    stream,
                    "403 Forbidden",
                    "text/plain; charset=utf-8",
                    "file attachments must be enqueued from localhost\n",
                    false,
                );
            }
            let command = {
                let mut bridge = bridge
                    .lock()
                    .map_err(|_| io::Error::new(io::ErrorKind::Other, "bridge state poisoned"))?;
                bridge.next_id += 1;
                let id = bridge.next_id.to_string();
                let file = request
                    .file_path
                    .as_deref()
                    .map(|path| bridge_command_file(&id, path))
                    .transpose()?;
                let command = BridgeCommand {
                    id: id.clone(),
                    kind: request.kind,
                    args: request.args,
                    file,
                };
                if let Some(path) = request.file_path {
                    bridge.files.insert(id, PathBuf::from(path));
                }
                bridge.queue.push_back(command.clone());
                command
            };
            let body = serde_json::to_string(&json!({
                "id": command.id,
                "status": "queued"
            }))?;
            write_http_response(
                stream,
                "200 OK",
                "application/json; charset=utf-8",
                &body,
                false,
            )
        }
        ("GET", "/bridge/commands/next") => {
            let client_id = http_header_value(initial, BRIDGE_CLIENT_ID_HEADER);
            let command = {
                let mut bridge = bridge
                    .lock()
                    .map_err(|_| io::Error::new(io::ErrorKind::Other, "bridge state poisoned"))?;
                if bridge.active_client_id.as_deref() != client_id.as_deref() {
                    return write_stale_bridge_client_response(stream);
                }
                bridge.queue.pop_front()
            };
            let body = serde_json::to_string(&json!({ "command": command }))?;
            write_http_response(
                stream,
                "200 OK",
                "application/json; charset=utf-8",
                &body,
                false,
            )
        }
        ("POST", "/bridge/commands/result") => {
            let body = read_http_body(stream, initial, MAX_WEB_BRIDGE_BODY_BYTES)?;
            let result = serde_json::from_str::<BridgeCommandResult>(&body).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid bridge result: {error}"),
                )
            })?;
            let client_id = http_header_value(initial, BRIDGE_CLIENT_ID_HEADER);
            {
                let mut bridge = bridge
                    .lock()
                    .map_err(|_| io::Error::new(io::ErrorKind::Other, "bridge state poisoned"))?;
                if bridge.active_client_id.as_deref() != client_id.as_deref() {
                    return write_stale_bridge_client_response(stream);
                }
                bridge.files.remove(&result.id);
                bridge.results.insert(result.id.clone(), result);
            }
            write_http_response(
                stream,
                "204 No Content",
                "text/plain; charset=utf-8",
                "",
                false,
            )
        }
        ("GET", path) if path.starts_with("/bridge/commands/result/") => {
            let id = path.trim_start_matches("/bridge/commands/result/");
            let result = {
                let bridge = bridge
                    .lock()
                    .map_err(|_| io::Error::new(io::ErrorKind::Other, "bridge state poisoned"))?;
                bridge.results.get(id).cloned()
            };
            if let Some(result) = result {
                let body = serde_json::to_string(&result)?;
                write_http_response(
                    stream,
                    "200 OK",
                    "application/json; charset=utf-8",
                    &body,
                    false,
                )
            } else {
                let body = serde_json::to_string(&json!({
                    "id": id,
                    "pending": true
                }))?;
                write_http_response(
                    stream,
                    "202 Accepted",
                    "application/json; charset=utf-8",
                    &body,
                    false,
                )
            }
        }
        ("GET", path) if path.starts_with("/bridge/commands/file/") => {
            let id = path.trim_start_matches("/bridge/commands/file/");
            let file_path = {
                let bridge = bridge
                    .lock()
                    .map_err(|_| io::Error::new(io::ErrorKind::Other, "bridge state poisoned"))?;
                bridge.files.get(id).cloned()
            };
            let Some(file_path) = file_path else {
                return write_http_response(
                    stream,
                    "404 Not Found",
                    "text/plain; charset=utf-8",
                    "file not found\n",
                    false,
                );
            };
            write_http_file_response(
                stream,
                "200 OK",
                "application/vnd.android.package-archive",
                &file_path,
            )
        }
        _ => write_http_response(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found\n",
            false,
        ),
    }
}

fn bridge_command_file(id: &str, path: &str) -> io::Result<BridgeCommandFile> {
    let path = Path::new(path);
    let metadata = path.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "bridge file attachment is not a regular file",
        ));
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("android-app.apk")
        .to_string();
    Ok(BridgeCommandFile {
        name,
        size: metadata.len(),
        url: format!("/bridge/commands/file/{id}"),
    })
}

fn write_stale_bridge_client_response(stream: &mut TcpStream) -> io::Result<()> {
    let body = serde_json::to_string(&json!({
        "error": "staleClient",
        "message": "another browser tab owns the active bridge session"
    }))?;
    write_http_response(
        stream,
        "409 Conflict",
        "application/json; charset=utf-8",
        &body,
        false,
    )
}

fn http_header_value(initial: &[u8], header_name: &str) -> Option<String> {
    let header_end = initial
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map_or(initial.len(), |position| position + 4);
    let headers = String::from_utf8_lossy(&initial[..header_end]);
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(header_name)
            .then(|| value.trim().to_string())
    })
}

fn is_local_http_host(host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|(host, _)| host))
        .unwrap_or_else(|| host.split(':').next().unwrap_or(host));
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn read_http_body(stream: &mut TcpStream, initial: &[u8], max_bytes: usize) -> io::Result<String> {
    let header_end = initial
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map_or(initial.len(), |position| position + 4);
    let headers = String::from_utf8_lossy(&initial[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    let target_body_len = content_length.min(max_bytes);
    let mut body = Vec::with_capacity(target_body_len);
    let initial_body = &initial[header_end..];
    let copied_len = initial_body.len().min(target_body_len);
    body.extend_from_slice(&initial_body[..copied_len]);

    while body.len() < target_body_len {
        let remaining = target_body_len - body.len();
        let mut chunk = vec![0_u8; remaining.min(4096)];
        let bytes_read = stream.read(&mut chunk)?;
        if bytes_read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..bytes_read]);
    }

    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
    headers_only: bool,
) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    if !headers_only {
        stream.write_all(body.as_bytes())?;
    }
    stream.flush()
}

fn write_http_file_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    path: &Path,
) -> io::Result<()> {
    let mut file = File::open(path)?;
    let content_length = file.metadata()?.len();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {content_length}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(response.as_bytes())?;
    io::copy(&mut file, stream)?;
    stream.flush()
}
