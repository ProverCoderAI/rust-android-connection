use docker_git_android_connection::{
    android_resource_limits, android_spec, android_tools, default_android_resource_limits,
    docker_run_args, no_vnc_endpoint, normalize_project_id, parse_docker_no_vnc_port,
    validate_adb_endpoint, DEFAULT_ANDROID_CPUS, DEFAULT_ANDROID_IMAGE,
    DEFAULT_ANDROID_MEMORY_LIMIT, DEFAULT_ANDROID_MEMORY_SWAP_LIMIT, DEFAULT_NOVNC_HOST,
    DEFAULT_NOVNC_PORT, DEFAULT_PROJECT_ID, NOVNC_DOCKER_BRIDGE_COMMAND,
};

#[test]
fn normalizes_project_id_to_docker_safe_name() {
    assert_eq!(
        normalize_project_id("Org/Repo:Feature_X"),
        "org-repo-feature-x"
    );
    assert_eq!(normalize_project_id("///"), DEFAULT_PROJECT_ID);
}

#[test]
fn rejects_shell_fragments_in_adb_endpoint() {
    assert!(validate_adb_endpoint("dg-test-android:5555").is_ok());
    assert!(validate_adb_endpoint("dg-test-android:5555;touch /tmp/pwn").is_err());
    assert!(validate_adb_endpoint("$(whoami):5555").is_err());
}

#[test]
fn builds_deterministic_android_spec() {
    let spec = android_spec(
        "dg-test",
        "docker-git-shared",
        "dg-test-android:5555",
        DEFAULT_ANDROID_IMAGE,
    )
    .expect("valid spec");

    assert_eq!(spec.project_container_name, "dg-test");
    assert_eq!(spec.android_container_name, "dg-test-android");
    assert_eq!(spec.android_volume_name, "dg-test-home-android");
}

#[test]
fn advertises_android_mcp_tools() {
    let names: Vec<&str> = android_tools().into_iter().map(|tool| tool.name).collect();

    assert!(names.contains(&"android_status"));
    assert!(names.contains(&"android_tap"));
    assert!(names.contains(&"android_install_apk"));
}

#[test]
fn docker_run_args_expose_no_vnc_on_host_port() {
    let spec = android_spec(
        "dg-test",
        "docker-git-shared",
        "dg-test-android:5555",
        DEFAULT_ANDROID_IMAGE,
    )
    .expect("valid spec");
    let no_vnc = no_vnc_endpoint(DEFAULT_NOVNC_HOST, DEFAULT_NOVNC_HOST, DEFAULT_NOVNC_PORT);
    let resource_limits = default_android_resource_limits();

    let args = docker_run_args(&spec, Some(&no_vnc), &resource_limits);

    assert!(args
        .windows(2)
        .any(|window| window == ["--publish", "127.0.0.1:6080:6081"]));
    assert_eq!(args.last(), Some(&NOVNC_DOCKER_BRIDGE_COMMAND.to_string()));
    assert!(args
        .windows(2)
        .any(|window| window == ["--memory", DEFAULT_ANDROID_MEMORY_LIMIT]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--memory-swap", DEFAULT_ANDROID_MEMORY_SWAP_LIMIT]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--cpus", DEFAULT_ANDROID_CPUS]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "EMULATOR_HEADLESS=false"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "WEB_VNC=true"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "WEB_VNC_PORT=6080"]));
}

#[test]
fn docker_run_args_can_disable_no_vnc_publication() {
    let spec = android_spec(
        "dg-test",
        "docker-git-shared",
        "dg-test-android:5555",
        DEFAULT_ANDROID_IMAGE,
    )
    .expect("valid spec");
    let resource_limits = default_android_resource_limits();

    let args = docker_run_args(&spec, None, &resource_limits);

    assert!(!args.iter().any(|argument| argument == "--publish"));
    assert_eq!(args.last(), Some(&DEFAULT_ANDROID_IMAGE.to_string()));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "WEB_VNC=true"]));
}

#[test]
fn docker_run_args_allow_custom_resource_limits() {
    let spec = android_spec(
        "dg-test",
        "docker-git-shared",
        "dg-test-android:5555",
        DEFAULT_ANDROID_IMAGE,
    )
    .expect("valid spec");
    let resource_limits = android_resource_limits("4g", "4g", "2.0");

    let args = docker_run_args(&spec, None, &resource_limits);

    assert!(args.windows(2).any(|window| window == ["--memory", "4g"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--memory-swap", "4g"]));
    assert!(args.windows(2).any(|window| window == ["--cpus", "2.0"]));
}

#[test]
fn parses_docker_port_for_actual_no_vnc_url() {
    let endpoint =
        parse_docker_no_vnc_port("127.0.0.1:16080\n", DEFAULT_NOVNC_HOST).expect("port binding");

    assert_eq!(endpoint.bind_host, "127.0.0.1");
    assert_eq!(endpoint.url_host, "127.0.0.1");
    assert_eq!(
        endpoint.url,
        "http://127.0.0.1:16080/?autoconnect=true&resize=remote"
    );
}

#[test]
fn wildcard_docker_port_does_not_leak_into_no_vnc_url() {
    let endpoint =
        parse_docker_no_vnc_port("0.0.0.0:16080\n", DEFAULT_NOVNC_HOST).expect("port binding");

    assert_eq!(endpoint.bind_host, "0.0.0.0");
    assert_eq!(endpoint.url_host, DEFAULT_NOVNC_HOST);
    assert_eq!(
        endpoint.url,
        "http://127.0.0.1:16080/?autoconnect=true&resize=remote"
    );
}
