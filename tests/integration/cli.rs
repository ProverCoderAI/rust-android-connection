use std::process::Command;

#[test]
fn lifecycle_cli_renders_status_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_docker-git-android-connection"))
        .args([
            "status",
            "--project",
            "dg-test",
            "--endpoint",
            "dg-test-android:5555",
        ])
        .output()
        .expect("failed to execute lifecycle binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"project_id\": \"dg-test\""));
    assert!(stdout.contains("\"android_container_name\": \"dg-test-android\""));
}

#[test]
fn lifecycle_cli_rejects_invalid_endpoint() {
    let output = Command::new(env!("CARGO_BIN_EXE_docker-git-android-connection"))
        .args(["status", "--endpoint", "$(whoami):5555"])
        .output()
        .expect("failed to execute lifecycle binary");

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
