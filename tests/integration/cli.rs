use serde_json::Value;
use std::process::Command;

#[test]
fn lifecycle_cli_renders_status_json() {
    let output = lifecycle_output(&[
        "status",
        "--project",
        "dg-test",
        "--endpoint",
        "dg-test-android:5555",
        "--novnc-port",
        "16080",
    ]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);

    assert_eq!(json["projectId"], "dg-test");
    assert_eq!(json["androidContainerName"], "dg-test-android");
    assert_eq!(
        json["noVncUrl"],
        "http://127.0.0.1:16080/?autoconnect=true&resize=remote"
    );
    assert_eq!(json["noVncPublished"], true);
    assert_eq!(json["resourceLimits"]["memory"], "3g");
    assert_eq!(json["resourceLimits"]["memorySwap"], "3g");
    assert_eq!(json["resourceLimits"]["cpus"], "1.0");
}

#[test]
fn lifecycle_cli_can_disable_no_vnc_publication() {
    let output = lifecycle_output(&[
        "status",
        "--project",
        "dg-test",
        "--endpoint",
        "dg-test-android:5555",
        "--no-novnc-publish",
    ]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);

    assert_eq!(json["projectId"], "dg-test");
    assert_eq!(json["noVncPublished"], false);
    assert!(json["noVncUrl"].is_null());
}

#[test]
fn start_dry_run_includes_publish_and_no_vnc_url_json() {
    let output = lifecycle_output(&[
        "start",
        "--project",
        "dg-test",
        "--endpoint",
        "dg-test-android:5555",
        "--novnc-port",
        "16080",
        "--dry-run",
    ]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);
    let docker_args = json["docker"]
        .as_array()
        .expect("docker args array")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let publish_position = docker_args
        .iter()
        .position(|argument| *argument == "--publish")
        .expect("publish flag");
    let image_position = docker_args
        .iter()
        .position(|argument| *argument == "budtmo/docker-android:emulator_14.0")
        .expect("image argument");

    assert!(publish_position < image_position);
    assert!(docker_args
        .get(image_position + 1)
        .is_some_and(|argument| argument.contains("socat TCP-LISTEN:6081")));
    assert_eq!(
        docker_args.get(publish_position + 1),
        Some(&"127.0.0.1:16080:6081")
    );
    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--memory", "3g"]));
    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--memory-swap", "3g"]));
    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--cpus", "1.0"]));
    assert_eq!(
        json["noVncUrl"],
        "http://127.0.0.1:16080/?autoconnect=true&resize=remote"
    );
    assert_eq!(json["resourceLimits"]["memory"], "3g");
    assert_eq!(json["resourceLimits"]["memorySwap"], "3g");
    assert_eq!(json["resourceLimits"]["cpus"], "1.0");
}

#[test]
fn start_dry_run_accepts_custom_resource_limits() {
    let output = lifecycle_output(&[
        "start",
        "--project",
        "dg-test",
        "--endpoint",
        "dg-test-android:5555",
        "--memory",
        "4g",
        "--memory-swap",
        "4g",
        "--cpus",
        "2.0",
        "--dry-run",
    ]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);
    let docker_args = json["docker"]
        .as_array()
        .expect("docker args array")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();

    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--memory", "4g"]));
    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--memory-swap", "4g"]));
    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--cpus", "2.0"]));
    assert_eq!(json["resourceLimits"]["memory"], "4g");
    assert_eq!(json["resourceLimits"]["memorySwap"], "4g");
    assert_eq!(json["resourceLimits"]["cpus"], "2.0");
}

#[test]
fn lifecycle_cli_rejects_invalid_no_vnc_port() {
    let output = lifecycle_output(&["status", "--novnc-port", "0"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("noVNC port must be in 1..=65535"));
}

#[test]
fn lifecycle_cli_rejects_invalid_cpu_limit() {
    let output = lifecycle_output(&["status", "--cpus", "0"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("CPU limit must be a positive finite number"));
}

#[test]
fn lifecycle_cli_rejects_invalid_endpoint() {
    let output = lifecycle_output(&["status", "--endpoint", "$(whoami):5555"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid ADB endpoint"));
}

#[test]
fn mcp_cli_exposes_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_android-connection"))
        .arg("--help")
        .output()
        .expect("failed to execute MCP binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Android MCP stdio server"));
    assert!(stdout.contains("--no-adb-probe"));
}

fn lifecycle_output(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_docker-git-android-connection"))
        .args(args)
        .output()
        .expect("failed to execute lifecycle binary")
}

fn parse_stdout_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not valid JSON: {error}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
