# rust-android-connection

Rust Android MCP and lifecycle module for docker-git.

## Install

```bash
cargo install --git https://github.com/ProverCoderAI/rust-android-connection --branch main --locked --bins
```

Installs two binaries:

```text
docker-git-android-connection  # start/status/stop Android runtime container
android-connection             # MCP stdio server for Codex, Claude, Gemini, and Grok
```

## Lifecycle CLI

```bash
docker-git-android-connection status --project dg-my-project
docker-git-android-connection start --project dg-my-project --dry-run
docker-git-android-connection stop --project dg-my-project --dry-run
```

The lifecycle CLI computes deterministic Docker names from the project id and validates the configured ADB endpoint before constructing Docker arguments.

## MCP Server

```bash
android-connection --project dg-my-project --network docker-git-shared --endpoint dg-my-project-android:5555 --workspace .
```

For handshake tests without ADB access:

```bash
android-connection --project dg-my-project --no-adb-probe
```

## MCP Tools

```text
android_status()
android_devices()
android_screenshot(path?)
android_tap(x, y)
android_swipe(startX, startY, endX, endY, durationMs?)
android_type_text(text)
android_press_key(keycode)
android_launch_app(package, activity?)
android_open_url(url)
android_logcat(lines?)
android_install_apk(path)
```

`android_install_apk` is disabled unless the server is started with `--allow-install`.

## Smoke Test

```bash
python3 - <<'PY' | android-connection --project dg-my-project --no-adb-probe | python3 - <<'PY'
import json
import sys

messages = [
    {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
    {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}},
    {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
]

for message in messages:
    body = json.dumps(message, separators=(",", ":")).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode())
    sys.stdout.buffer.write(body)
PY
import json
import sys

stream = sys.stdin.buffer
while True:
    header = {}
    while True:
        line = stream.readline()
        if not line:
            raise SystemExit(0)
        stripped = line.strip()
        if not stripped:
            break
        name, value = line.decode().split(":", 1)
        header[name.lower()] = value.strip()

    length = int(header["content-length"])
    body = stream.read(length)
    print(json.dumps(json.loads(body), indent=2))
PY
```

Expected: server `android-connection` and tools such as `android_status`, `android_tap`, and `android_screenshot`.

## Development

```bash
cargo fmt --check
cargo test --locked
cargo build --locked --bins
cargo clippy --locked --all-targets --all-features -- -D warnings
```
