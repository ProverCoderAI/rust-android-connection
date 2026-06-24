use docker_git_android_connection::{
    android_spec, docker_run_args, DEFAULT_ADB_ENDPOINT, DEFAULT_ANDROID_IMAGE,
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
    println!("docker args: {}", docker_run_args(&spec).join(" "));
}
