use docker_git_android_connection::{
    android_spec, default_android_resource_limits, docker_run_args, interactive_runtime_options,
    no_vnc_endpoint, DEFAULT_ADB_ENDPOINT, DEFAULT_ANDROID_IMAGE, DEFAULT_NOVNC_HOST,
    DEFAULT_NOVNC_PORT,
};

fn main() {
    let spec = android_spec(
        "dg-my-project",
        "docker-git-shared",
        DEFAULT_ADB_ENDPOINT,
        DEFAULT_ANDROID_IMAGE,
    )
    .expect("default Android spec is valid");

    println!("container: {}", spec.android_container_name);
    let no_vnc = no_vnc_endpoint(DEFAULT_NOVNC_HOST, DEFAULT_NOVNC_HOST, DEFAULT_NOVNC_PORT);
    let resource_limits = default_android_resource_limits();
    let runtime_options = interactive_runtime_options();
    println!("noVNC: {}", no_vnc.url);
    println!(
        "docker args: {}",
        docker_run_args(&spec, Some(&no_vnc), &resource_limits, &runtime_options).join(" ")
    );
}
