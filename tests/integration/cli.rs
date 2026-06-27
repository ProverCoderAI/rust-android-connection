use serde_json::Value;
use std::process::Command;

#[test]
fn lifecycle_cli_renders_status_json() {
    let output = lifecycle_output(&[
        "dg-test",
        "status",
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
    assert_eq!(json["runtime"]["profile"], "interactive");
    assert_eq!(json["runtime"]["emulatorHeadless"], false);
}

#[test]
fn lifecycle_cli_can_disable_no_vnc_publication() {
    let output = lifecycle_output(&[
        "dg-test",
        "status",
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
        "dg-test",
        "start",
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
    assert_eq!(json["runtime"]["profile"], "interactive");
}

#[test]
fn start_dry_run_accepts_custom_resource_limits() {
    let output = lifecycle_output(&[
        "dg-test",
        "start",
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
fn start_dry_run_supports_app_test_runtime_profile() {
    let output = lifecycle_output(&[
        "dg-test",
        "start",
        "--endpoint",
        "dg-test-android:5555",
        "--runtime-profile",
        "app-test",
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

    assert_eq!(json["runtime"]["profile"], "app-test");
    assert_eq!(json["runtime"]["emulatorHeadless"], true);
    assert_eq!(json["noVncPublished"], false);
    assert!(json["noVncUrl"].is_null());
    assert!(!docker_args.contains(&"--publish"));
    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--env", "EMULATOR_HEADLESS=true"]));
    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--env", "APPIUM=false"]));
    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--env", "WEB_LOG=false"]));
    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--env", "WEB_VNC=false"]));
    assert!(docker_args
        .last()
        .is_some_and(|argument| argument.contains("hw.gsmModem = no")));
}

#[test]
fn start_dry_run_supports_app_test_vnc_runtime_profile() {
    let output = lifecycle_output(&[
        "dg-test",
        "start",
        "--endpoint",
        "dg-test-android:5555",
        "--runtime-profile",
        "app-test-vnc",
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

    assert_eq!(json["runtime"]["profile"], "app-test-vnc");
    assert_eq!(json["runtime"]["emulatorHeadless"], false);
    assert_eq!(json["runtime"]["appiumEnabled"], false);
    assert_eq!(json["runtime"]["webLogEnabled"], false);
    assert_eq!(json["runtime"]["webVncEnabled"], true);
    assert_eq!(json["noVncPublished"], true);
    assert_eq!(
        json["noVncUrl"],
        "http://127.0.0.1:16080/?autoconnect=true&resize=remote"
    );
    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--publish", "127.0.0.1:16080:6081"]));
    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--env", "EMULATOR_HEADLESS=false"]));
    assert!(docker_args
        .windows(2)
        .any(|window| window == ["--env", "WEB_VNC=true"]));
    assert!(docker_args
        .last()
        .is_some_and(|argument| argument.contains("socat TCP-LISTEN:6081")));
    assert!(docker_args
        .last()
        .is_some_and(|argument| argument.contains("hw.gsmModem = no")));
}

#[test]
fn lifecycle_cli_rejects_invalid_no_vnc_port() {
    let output = lifecycle_output(&["dg-test", "status", "--novnc-port", "0"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("noVNC port must be in 1..=65535"));
}

#[test]
fn lifecycle_cli_rejects_invalid_cpu_limit() {
    let output = lifecycle_output(&["dg-test", "status", "--cpus", "0"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("CPU limit must be a positive finite number"));
}

#[test]
fn lifecycle_cli_rejects_invalid_runtime_profile() {
    let output = lifecycle_output(&["dg-test", "status", "--runtime-profile", "unknown"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("runtime profile must be one of"));
}

#[test]
fn web_dry_run_renders_browser_connector_json() {
    let output = cli_output(&["web", "--bind", "0.0.0.0", "--port", "18080", "--dry-run"]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);

    assert_eq!(json["bindHost"], "0.0.0.0");
    assert_eq!(json["port"], 18080);
    assert_eq!(json["url"], "http://127.0.0.1:18080/");
    assert_eq!(json["requiresAdb"], false);
    assert_eq!(json["requiresBrowser"], "Chromium WebUSB");
    assert_eq!(json["secureContext"], "localhost-or-https");
    assert_eq!(json["phoneBridge"], true);
}

#[test]
fn web_cli_rejects_invalid_port() {
    let output = cli_output(&["web", "--port", "0", "--dry-run"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("web port must be in 1..=65535"));
}

#[test]
fn phone_adb_dry_run_renders_browser_bridge_json() {
    let output = cli_output(&[
        "phone",
        "--url",
        "http://127.0.0.1:18080",
        "adb",
        "--dry-run",
        "shell",
        "getprop",
        "ro.product.model",
    ]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);
    let args = json_array_as_strings(&json["args"]);

    assert_eq!(json["url"], "http://127.0.0.1:18080");
    assert_eq!(json["kind"], "adb");
    assert_eq!(json["transport"], "browser-webusb-bridge");
    assert_eq!(args, ["shell", "getprop", "ro.product.model"]);
}

#[test]
fn lifecycle_cli_rejects_invalid_endpoint() {
    let output = lifecycle_output(&["dg-test", "status", "--endpoint", "$(whoami):5555"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid ADB endpoint"));
}

#[test]
fn adb_dry_run_proxies_to_container_by_default() {
    let output = lifecycle_output(&[
        "dg-test",
        "adb",
        "--endpoint",
        "dg-test-android:5555",
        "--dry-run",
        "shell",
        "getprop",
        "sys.boot_completed",
    ]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);
    let host = json_array_as_strings(&json["host"]);
    let container = json_array_as_strings(&json["container"]);

    assert_eq!(json["adbMode"], "container");
    assert_eq!(
        host,
        [
            "sh",
            "-c",
            "adb connect 'dg-test-android:5555' && adb 'shell' 'getprop' 'sys.boot_completed'",
        ]
    );
    assert_eq!(
        container,
        [
            "docker",
            "exec",
            "dg-test-android",
            "adb",
            "shell",
            "getprop",
            "sys.boot_completed",
        ]
    );
}

#[test]
fn adb_dry_run_accepts_empty_adb_invocation() {
    let output = lifecycle_output(&["dg-test", "adb", "--dry-run"]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);
    let host = json_array_as_strings(&json["host"]);
    let container = json_array_as_strings(&json["container"]);

    assert_eq!(host, ["sh", "-c", "adb connect 'android:5555' && adb"]);
    assert_eq!(container, ["docker", "exec", "dg-test-android", "adb"]);
}

#[test]
fn adb_dry_run_passes_help_to_container_adb() {
    let output = lifecycle_output(&["dg-test", "adb", "--dry-run", "--help"]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);
    let container = json_array_as_strings(&json["container"]);

    assert_eq!(
        container,
        ["docker", "exec", "dg-test-android", "adb", "--help"]
    );
}

#[test]
fn adb_dry_run_passes_version_to_container_adb() {
    let output = lifecycle_output(&["dg-test", "adb", "--dry-run", "--version"]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);
    let container = json_array_as_strings(&json["container"]);

    assert_eq!(
        container,
        ["docker", "exec", "dg-test-android", "adb", "--version"]
    );
}

#[test]
fn install_apk_dry_run_copies_apk_for_container_mode() {
    let output = lifecycle_output(&["dg-test", "install-apk", "--dry-run", "app/build/app.apk"]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);
    let container_copy = json_array_as_strings(&json["containerCopy"]);
    let container = json_array_as_strings(&json["container"]);

    assert_eq!(json["adbMode"], "container");
    assert_eq!(
        container_copy,
        [
            "cp",
            "app/build/app.apk",
            "dg-test-android:/tmp/docker-git-install.apk",
        ]
    );
    assert_eq!(
        container,
        [
            "docker",
            "exec",
            "dg-test-android",
            "adb",
            "-s",
            "emulator-5554",
            "install",
            "/tmp/docker-git-install.apk",
        ]
    );
}

#[test]
fn launch_app_dry_run_renders_monkey_command() {
    let output = lifecycle_output(&[
        "dg-test",
        "launch-app",
        "--package",
        "com.example.app",
        "--dry-run",
    ]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);
    let container = json_array_as_strings(&json["container"]);

    assert_eq!(
        container,
        [
            "docker",
            "exec",
            "dg-test-android",
            "adb",
            "-s",
            "emulator-5554",
            "shell",
            "monkey",
            "-p",
            "com.example.app",
            "-c",
            "android.intent.category.LAUNCHER",
            "1",
        ]
    );
}

#[test]
fn adb_dry_run_preserves_explicit_serial() {
    let output = lifecycle_output(&[
        "dg-test",
        "adb",
        "--dry-run",
        "--",
        "-s",
        "custom-device",
        "shell",
        "getprop",
        "ro.product.model",
    ]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);
    let container = json_array_as_strings(&json["container"]);

    assert_eq!(
        container,
        [
            "docker",
            "exec",
            "dg-test-android",
            "adb",
            "-s",
            "custom-device",
            "shell",
            "getprop",
            "ro.product.model",
        ]
    );
}

#[test]
fn adb_dry_run_preserves_hyphenated_adb_arguments() {
    let output = lifecycle_output(&[
        "dg-test",
        "adb",
        "--dry-run",
        "shell",
        "monkey",
        "-p",
        "com.example.app",
        "-c",
        "android.intent.category.LAUNCHER",
        "1",
    ]);

    assert!(output.status.success());
    let json = parse_stdout_json(&output);
    let container = json_array_as_strings(&json["container"]);

    assert_eq!(
        container,
        [
            "docker",
            "exec",
            "dg-test-android",
            "adb",
            "shell",
            "monkey",
            "-p",
            "com.example.app",
            "-c",
            "android.intent.category.LAUNCHER",
            "1",
        ]
    );
}

#[test]
fn adb_cli_rejects_invalid_adb_mode() {
    let output = lifecycle_output(&["dg-test", "adb", "--adb-mode", "remote", "--", "devices"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ADB mode must be one of: auto, host, container"));
}

fn lifecycle_output(args: &[&str]) -> std::process::Output {
    cli_output(args)
}

fn cli_output(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rust-android-connection"))
        .args(args)
        .output()
        .expect("failed to execute rust-android-connection binary")
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

fn json_array_as_strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .expect("JSON value is an array")
        .iter()
        .map(|entry| entry.as_str().expect("JSON array entry is a string"))
        .collect()
}
