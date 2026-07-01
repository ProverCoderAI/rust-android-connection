use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const WEB_COMMAND_NAME: &str = "web";
pub const PHONE_COMMAND_NAME: &str = "phone";

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
    name = "rust-android-connection web",
    about = "Serve the browser WebUSB/WebADB phone connector"
)]
pub struct WebArgs {
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
pub struct PhoneArgs {
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

pub fn phone(args: &PhoneArgs) -> Result<(), Box<dyn std::error::Error>> {
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
    Ok(Some(path.clone()))
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

pub fn web(args: &WebArgs) -> Result<(), Box<dyn std::error::Error>> {
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

fn parse_web_port(value: &str) -> Result<u16, String> {
    let port = value
        .parse::<u16>()
        .map_err(|error| format!("invalid web port {value:?}: {error}"))?;
    if port == 0 {
        Err("web port must be in 1..=65535".to_string())
    } else {
        Ok(port)
    }
}

fn print_json(value: &Value) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
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
