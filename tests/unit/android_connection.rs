use docker_git_android_connection::{
    android_spec, android_tools, normalize_project_id, validate_adb_endpoint,
    DEFAULT_ANDROID_IMAGE, DEFAULT_PROJECT_ID,
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
