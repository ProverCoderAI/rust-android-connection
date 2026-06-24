pub mod mcp;

use serde::Serialize;

pub const SERVER_NAME: &str = "android-connection";
pub const DEFAULT_ANDROID_IMAGE: &str = "budtmo/docker-android:emulator_14.0";
pub const DEFAULT_ADB_ENDPOINT: &str = "android:5555";
pub const DEFAULT_PROJECT_ID: &str = "docker-git";
pub const DEFAULT_NOVNC_HOST: &str = "127.0.0.1";
pub const DEFAULT_NOVNC_PORT: u16 = 6080;
pub const DEFAULT_NOVNC_CONTAINER_PORT: u16 = 6081;
pub const DEFAULT_NOVNC_WEB_PORT: u16 = 6080;
pub const DEFAULT_ANDROID_MEMORY_LIMIT: &str = "3g";
pub const DEFAULT_ANDROID_MEMORY_SWAP_LIMIT: &str = "3g";
pub const DEFAULT_ANDROID_CPUS: &str = "1.0";
pub const NOVNC_DOCKER_BRIDGE_COMMAND: &str = "while true; do container_ip=$(hostname -i | awk '{print $1}'); /usr/bin/socat TCP-LISTEN:6081,bind=${container_ip},fork,reuseaddr TCP:127.0.0.1:6080; sleep 1; done & exec ${APP_PATH}/mixins/scripts/run.sh";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointError {
    pub value: String,
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid ADB endpoint {:?}; allowed characters are ASCII letters, digits, '.', '-', '_' and ':'",
            self.value
        )
    }
}

impl std::error::Error for EndpointError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AndroidSpec {
    pub project_id: String,
    pub project_container_name: String,
    pub android_container_name: String,
    pub android_volume_name: String,
    pub docker_network: String,
    pub adb_endpoint: String,
    pub image: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoVncEndpoint {
    pub bind_host: String,
    pub url_host: String,
    pub host_port: u16,
    pub container_port: u16,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidResourceLimits {
    pub memory: String,
    pub memory_swap: String,
    pub cpus: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct McpToolSpec {
    pub name: &'static str,
    pub description: &'static str,
}

// CHANGE: normalize externally supplied project ids into Docker-safe names
// WHY: Android sidecar names are pure functions of the project id, so MCP clients and lifecycle CLI agree
// QUOTE(TZ): "Подключить mcp-android так же как работает MCP PLAYRIGHT"
// REF: issue-436
// SOURCE: n/a
// FORMAT THEOREM: forall s: normalize(s) in [a-z0-9-]+ and normalize(s) != ""
// PURITY: CORE
// INVARIANT: output is non-empty, lowercase, and contains only Docker-name-safe characters
// COMPLEXITY: O(n)/O(n)
#[must_use]
pub fn normalize_project_id(raw: &str) -> String {
    let mut normalized = String::new();
    let mut previous_dash = false;

    for byte in raw.bytes() {
        let next = match byte {
            b'a'..=b'z' | b'0'..=b'9' => Some(byte as char),
            b'A'..=b'Z' => Some(byte.to_ascii_lowercase() as char),
            _ => {
                if normalized.is_empty() || previous_dash {
                    None
                } else {
                    Some('-')
                }
            }
        };

        if let Some(character) = next {
            previous_dash = character == '-';
            normalized.push(character);
        }
    }

    while normalized.ends_with('-') {
        normalized.pop();
    }

    if normalized.is_empty() {
        DEFAULT_PROJECT_ID.to_string()
    } else {
        normalized
    }
}

#[must_use]
pub fn android_container_name(project_id: &str) -> String {
    format!("{}-android", normalize_project_id(project_id))
}

#[must_use]
pub fn android_volume_name(project_id: &str) -> String {
    format!("{}-home-android", normalize_project_id(project_id))
}

#[must_use]
pub fn is_safe_adb_endpoint(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.contains(':')
        && value.bytes().all(|byte| {
            matches!(
                byte,
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b'_' | b':'
            )
        })
}

pub fn validate_adb_endpoint(value: &str) -> Result<String, EndpointError> {
    if is_safe_adb_endpoint(value) {
        Ok(value.to_string())
    } else {
        Err(EndpointError {
            value: value.to_string(),
        })
    }
}

pub fn android_spec(
    project_id: &str,
    docker_network: &str,
    adb_endpoint: &str,
    image: &str,
) -> Result<AndroidSpec, EndpointError> {
    let normalized = normalize_project_id(project_id);
    Ok(AndroidSpec {
        project_id: normalized.clone(),
        project_container_name: normalized.clone(),
        android_container_name: android_container_name(&normalized),
        android_volume_name: android_volume_name(&normalized),
        docker_network: docker_network.to_string(),
        adb_endpoint: validate_adb_endpoint(adb_endpoint)?,
        image: image.to_string(),
    })
}

#[must_use]
pub fn android_tools() -> Vec<McpToolSpec> {
    vec![
        McpToolSpec {
            name: "android_status",
            description: "Return the configured Android runtime and optional ADB status.",
        },
        McpToolSpec {
            name: "android_devices",
            description: "List Android devices visible to adb.",
        },
        McpToolSpec {
            name: "android_screenshot",
            description: "Capture a PNG screenshot into the workspace.",
        },
        McpToolSpec {
            name: "android_tap",
            description: "Tap screen coordinates.",
        },
        McpToolSpec {
            name: "android_swipe",
            description: "Swipe between screen coordinates.",
        },
        McpToolSpec {
            name: "android_type_text",
            description: "Type text into the active Android input field.",
        },
        McpToolSpec {
            name: "android_press_key",
            description: "Send an Android keycode.",
        },
        McpToolSpec {
            name: "android_launch_app",
            description: "Launch an installed Android package.",
        },
        McpToolSpec {
            name: "android_open_url",
            description: "Open a URL through Android intent handling.",
        },
        McpToolSpec {
            name: "android_logcat",
            description: "Read recent logcat output.",
        },
        McpToolSpec {
            name: "android_install_apk",
            description: "Install an APK from the workspace when explicitly enabled.",
        },
    ]
}

#[must_use]
pub fn no_vnc_url_host_for_bind_host(bind_host: &str) -> String {
    let trimmed = bind_host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let visible_host = match trimmed {
        "" | "0.0.0.0" | "::" => DEFAULT_NOVNC_HOST,
        host => host,
    };

    if visible_host.contains(':') && !visible_host.starts_with('[') {
        format!("[{visible_host}]")
    } else {
        visible_host.to_string()
    }
}

#[must_use]
pub fn no_vnc_endpoint(bind_host: &str, url_host: &str, host_port: u16) -> NoVncEndpoint {
    let url_host = no_vnc_url_host_for_bind_host(url_host);
    NoVncEndpoint {
        bind_host: bind_host.to_string(),
        url_host: url_host.clone(),
        host_port,
        container_port: DEFAULT_NOVNC_CONTAINER_PORT,
        url: format!("http://{url_host}:{host_port}/?autoconnect=true&resize=remote"),
    }
}

#[must_use]
pub fn parse_docker_no_vnc_port(output: &str, default_url_host: &str) -> Option<NoVncEndpoint> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find_map(|line| {
            let (raw_host, raw_port) = line.rsplit_once(':')?;
            let host_port = raw_port.parse::<u16>().ok()?;
            if host_port == 0 {
                return None;
            }

            let bind_host = raw_host.trim_start_matches('[').trim_end_matches(']');
            let url_host = match bind_host {
                "" | "0.0.0.0" | "::" => default_url_host,
                host => host,
            };
            Some(no_vnc_endpoint(bind_host, url_host, host_port))
        })
}

#[must_use]
pub fn android_resource_limits(
    memory: &str,
    memory_swap: &str,
    cpus: &str,
) -> AndroidResourceLimits {
    AndroidResourceLimits {
        memory: memory.to_string(),
        memory_swap: memory_swap.to_string(),
        cpus: cpus.to_string(),
    }
}

#[must_use]
pub fn default_android_resource_limits() -> AndroidResourceLimits {
    android_resource_limits(
        DEFAULT_ANDROID_MEMORY_LIMIT,
        DEFAULT_ANDROID_MEMORY_SWAP_LIMIT,
        DEFAULT_ANDROID_CPUS,
    )
}

#[must_use]
pub fn docker_run_args(
    spec: &AndroidSpec,
    no_vnc: Option<&NoVncEndpoint>,
    resource_limits: &AndroidResourceLimits,
) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--detach".to_string(),
        "--name".to_string(),
        spec.android_container_name.clone(),
        "--memory".to_string(),
        resource_limits.memory.clone(),
        "--memory-swap".to_string(),
        resource_limits.memory_swap.clone(),
        "--cpus".to_string(),
        resource_limits.cpus.clone(),
        "--privileged".to_string(),
        "--network".to_string(),
        spec.docker_network.clone(),
        "--env".to_string(),
        "EMULATOR_HEADLESS=false".to_string(),
        "--env".to_string(),
        "WEB_VNC=true".to_string(),
        "--env".to_string(),
        format!("WEB_VNC_PORT={DEFAULT_NOVNC_WEB_PORT}"),
        "--volume".to_string(),
        format!("{}:/root/.android", spec.android_volume_name),
    ];

    if let Some(no_vnc) = no_vnc {
        args.extend([
            "--publish".to_string(),
            format!(
                "{}:{}:{}",
                no_vnc.bind_host, no_vnc.host_port, no_vnc.container_port
            ),
        ]);
    }

    args.push(spec.image.clone());
    if no_vnc.is_some() {
        args.push(NOVNC_DOCKER_BRIDGE_COMMAND.to_string());
    }
    args
}

#[must_use]
pub fn docker_stop_args(spec: &AndroidSpec) -> Vec<String> {
    vec![
        "rm".to_string(),
        "--force".to_string(),
        spec.android_container_name.clone(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn builds_no_vnc_endpoint_for_host_browser_access() {
        let endpoint = no_vnc_endpoint("127.0.0.1", "127.0.0.1", 16_080);

        assert_eq!(endpoint.bind_host, "127.0.0.1");
        assert_eq!(endpoint.url_host, "127.0.0.1");
        assert_eq!(endpoint.host_port, 16_080);
        assert_eq!(endpoint.container_port, DEFAULT_NOVNC_CONTAINER_PORT);
        assert_eq!(
            endpoint.url,
            "http://127.0.0.1:16080/?autoconnect=true&resize=remote"
        );
    }

    #[test]
    fn renders_wildcard_no_vnc_binding_as_loopback_url() {
        let endpoint = no_vnc_endpoint("0.0.0.0", "0.0.0.0", DEFAULT_NOVNC_PORT);

        assert_eq!(endpoint.bind_host, "0.0.0.0");
        assert_eq!(endpoint.url_host, DEFAULT_NOVNC_HOST);
        assert_eq!(
            endpoint.url,
            "http://127.0.0.1:6080/?autoconnect=true&resize=remote"
        );
    }

    #[test]
    fn parses_docker_port_output_into_no_vnc_endpoint() {
        let endpoint = parse_docker_no_vnc_port("0.0.0.0:16080\n[::]:16080\n", DEFAULT_NOVNC_HOST)
            .expect("published noVNC port");

        assert_eq!(endpoint.bind_host, "0.0.0.0");
        assert_eq!(endpoint.url_host, DEFAULT_NOVNC_HOST);
        assert_eq!(endpoint.host_port, 16_080);
    }

    #[test]
    fn docker_run_args_publish_no_vnc_before_image() {
        let spec = android_spec(
            "dg-test",
            "docker-git-shared",
            "dg-test-android:5555",
            DEFAULT_ANDROID_IMAGE,
        )
        .expect("valid spec");
        let endpoint = no_vnc_endpoint("127.0.0.1", "127.0.0.1", DEFAULT_NOVNC_PORT);

        let resource_limits = default_android_resource_limits();
        let args = docker_run_args(&spec, Some(&endpoint), &resource_limits);
        let publish_position = args
            .iter()
            .position(|value| value == "--publish")
            .expect("publish flag");
        let image_position = args
            .iter()
            .position(|value| value == DEFAULT_ANDROID_IMAGE)
            .expect("image argument");

        assert!(publish_position < image_position);
        assert_eq!(
            args.get(image_position + 1),
            Some(&NOVNC_DOCKER_BRIDGE_COMMAND.to_string())
        );
        assert_eq!(
            args.get(publish_position + 1),
            Some(&"127.0.0.1:6080:6081".to_string())
        );
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
}
