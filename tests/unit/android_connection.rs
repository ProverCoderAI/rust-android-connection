use docker_git_android_connection::{
    android_resource_limits, android_spec, app_test_runtime_options, app_test_vnc_runtime_options,
    default_android_resource_limits, docker_run_args, interactive_runtime_options, no_vnc_endpoint,
    normalize_project_id, parse_docker_no_vnc_port, validate_adb_endpoint, RuntimeSwitch,
    APP_TEST_ANDROID_RUNTIME_PROFILE, APP_TEST_VNC_ANDROID_RUNTIME_PROFILE, DEFAULT_ANDROID_CPUS,
    DEFAULT_ANDROID_IMAGE, DEFAULT_ANDROID_MEMORY_LIMIT, DEFAULT_ANDROID_MEMORY_SWAP_LIMIT,
    DEFAULT_ANDROID_RUNTIME_PROFILE, DEFAULT_NOVNC_HOST, DEFAULT_NOVNC_PORT, DEFAULT_PROJECT_ID,
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
    let runtime_options = interactive_runtime_options();

    let args = docker_run_args(&spec, Some(&no_vnc), &resource_limits, &runtime_options);

    assert!(args
        .windows(2)
        .any(|window| window == ["--publish", "127.0.0.1:6080:6081"]));
    assert!(args
        .last()
        .is_some_and(|argument| argument.contains("socat TCP-LISTEN:6081")));
    assert!(args
        .last()
        .is_some_and(|argument| argument.contains("hw.gsmModem = no")));
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
        .any(|window| window == ["--env", "APPIUM=false"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "WEB_LOG=false"]));
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
    let runtime_options = interactive_runtime_options();

    let args = docker_run_args(&spec, None, &resource_limits, &runtime_options);

    assert!(!args.iter().any(|argument| argument == "--publish"));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "WEB_VNC=false"]));
    assert!(args
        .last()
        .is_some_and(|argument| argument.contains("exec ${APP_PATH}/mixins/scripts/run.sh")));
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
    let runtime_options = interactive_runtime_options();

    let args = docker_run_args(&spec, None, &resource_limits, &runtime_options);

    assert!(args.windows(2).any(|window| window == ["--memory", "4g"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--memory-swap", "4g"]));
    assert!(args.windows(2).any(|window| window == ["--cpus", "2.0"]));
}

#[test]
fn docker_run_args_support_app_test_runtime_profile() {
    let spec = android_spec(
        "dg-test",
        "docker-git-shared",
        "dg-test-android:5555",
        DEFAULT_ANDROID_IMAGE,
    )
    .expect("valid spec");
    let resource_limits = default_android_resource_limits();
    let runtime_options = app_test_runtime_options();

    let args = docker_run_args(&spec, None, &resource_limits, &runtime_options);

    assert_eq!(runtime_options.profile, APP_TEST_ANDROID_RUNTIME_PROFILE);
    assert!(!args.iter().any(|argument| argument == "--publish"));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "EMULATOR_HEADLESS=true"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "APPIUM=false"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "WEB_LOG=false"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "WEB_VNC=false"]));
    assert!(args.windows(2).any(|window| {
        window == [
            "--env",
            "EMULATOR_ADDITIONAL_ARGS=-no-window -no-audio -no-boot-anim -no-snapshot -lowram -memory 1536 -camera-back none -camera-front none",
        ]
    }));
    assert!(args
        .last()
        .is_some_and(|argument| argument.contains("hw.gsmModem = no")));
}

#[test]
fn docker_run_args_support_app_test_vnc_runtime_profile() {
    let spec = android_spec(
        "dg-test",
        "docker-git-shared",
        "dg-test-android:5555",
        DEFAULT_ANDROID_IMAGE,
    )
    .expect("valid spec");
    let no_vnc = no_vnc_endpoint(DEFAULT_NOVNC_HOST, DEFAULT_NOVNC_HOST, DEFAULT_NOVNC_PORT);
    let resource_limits = default_android_resource_limits();
    let runtime_options = app_test_vnc_runtime_options();

    let args = docker_run_args(&spec, Some(&no_vnc), &resource_limits, &runtime_options);

    assert_eq!(
        runtime_options.profile,
        APP_TEST_VNC_ANDROID_RUNTIME_PROFILE
    );
    assert!(args
        .windows(2)
        .any(|window| window == ["--publish", "127.0.0.1:6080:6081"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "EMULATOR_HEADLESS=false"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "APPIUM=false"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "WEB_LOG=false"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--env", "WEB_VNC=true"]));
    assert!(args.windows(2).any(|window| {
        window == [
            "--env",
            "EMULATOR_ADDITIONAL_ARGS=-no-audio -no-boot-anim -no-snapshot -lowram -memory 1536 -camera-back none -camera-front none",
        ]
    }));
    assert!(args
        .last()
        .is_some_and(|argument| argument.contains("socat TCP-LISTEN:6081")));
    assert!(args
        .last()
        .is_some_and(|argument| argument.contains("hw.gsmModem = no")));
}

#[test]
fn interactive_runtime_profile_is_default() {
    let runtime_options = interactive_runtime_options();

    assert_eq!(runtime_options.profile, DEFAULT_ANDROID_RUNTIME_PROFILE);
    assert_eq!(runtime_options.emulator_headless, RuntimeSwitch::Disabled);
    assert_eq!(runtime_options.web_vnc_enabled, RuntimeSwitch::Enabled);
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
