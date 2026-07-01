# rust-android-connection

Rust Android lifecycle and ADB command module for docker-git.

## Install

```bash
cargo install --git https://github.com/ProverCoderAI/rust-android-connection --branch main --locked --bins
```

Installs one binary:

```text
rust-android-connection  # start/status/stop Android runtime container and proxy ADB commands
```

## Lifecycle CLI

```bash
rust-android-connection dg-my-project status
rust-android-connection dg-my-project start --dry-run
rust-android-connection dg-my-project stop --dry-run
```

The project id is the first positional argument. The lifecycle CLI computes deterministic Docker names from it and validates the configured ADB endpoint before constructing Docker arguments. By default it publishes a Docker-Android noVNC bridge to `127.0.0.1:6080` and returns `noVncUrl` in lifecycle JSON:

```json
{
  "androidContainerName": "dg-my-project-android",
  "resourceLimits": {
    "memory": "3g",
    "memorySwap": "3g",
    "cpus": "1.0"
  },
  "runtime": {
    "profile": "interactive",
    "emulatorHeadless": false
  },
  "noVncPublished": true,
  "noVncUrl": "http://127.0.0.1:6080/?autoconnect=true&resize=remote"
}
```

Use `--novnc-port <port>` to request a different host port, `--novnc-bind-host <host>` to bind Docker publishing somewhere other than loopback, `--novnc-host <host>` to control the browser-facing URL host, or `--no-novnc-publish` to disable host publication.

Android containers are resource-limited by default with `--memory 3g --memory-swap 3g --cpus 1.0`. Use `--memory <docker-size>`, `--memory-swap <docker-size>`, and `--cpus <positive-number>` to override those limits for a specific run.

### Runtime Profiles

Use `app-test` when the goal is to build, install, and launch an APK through ADB rather than manually control the device through noVNC:

```bash
rust-android-connection dg-my-project start \
  --endpoint dg-my-project-android:5555 \
  --runtime-profile app-test
```

The `app-test` profile keeps the Docker limit at `3g/1 CPU` by default but reduces emulator pressure by running headless and disabling noVNC, Appium, web logs, skin, audio, cameras, boot animation, snapshots, and the emulated GSM modem. This is a tight minimum for APK install/launch smoke checks; use `--memory 4g --memory-swap 4g --cpus 2.0` if the app or UI tests are heavy.

It is intended for:

```text
build APK -> install-apk <path> -> launch-app --package <package> [--activity <activity>]
```

Use `app-test-vnc` when the same lightweight app-test setup needs noVNC for manual debugging:

```bash
rust-android-connection dg-my-project start \
  --endpoint dg-my-project-android:5555 \
  --runtime-profile app-test-vnc \
  --novnc-port 16080
```

`app-test-vnc` keeps Appium and web logs disabled, disables audio, cameras, snapshots, skin, and GSM modem, but runs a visible emulator window through noVNC. It is more expensive than `app-test`; use `--memory 4g --memory-swap 4g --cpus 2.0` if Android 14 or the tested app becomes unstable.

Use `interactive` when a human needs the full visual Docker-Android session through noVNC. For Android 14 with UI, expect to raise limits to roughly `--memory 5g --memory-swap 5g --cpus 2.0`.

## ADB Commands

```bash
rust-android-connection dg-my-project adb \
  shell getprop sys.boot_completed

rust-android-connection dg-my-project install-apk \
  app/build/outputs/apk/debug/app-debug.apk

rust-android-connection dg-my-project launch-app \
  --package com.example.app
```

By default, `adb` is a container proxy:

```bash
rust-android-connection dg-my-project adb devices
rust-android-connection dg-my-project adb shell getprop sys.boot_completed
```

The command above runs ADB inside the Android container:

```bash
docker exec dg-my-project-android adb shell getprop sys.boot_completed
```

ADB execution is still selectable with `--adb-mode container|auto|host` when an explicit override is needed. `auto` first tries host `adb connect <endpoint>` and then runs host `adb ...`; if host ADB is unavailable or cannot connect, it falls back to `docker exec <android-container> adb ...`.

For APK installation in `container` mode, the CLI copies the APK into the Android container and installs that internal path:

```text
docker cp app.apk dg-my-project-android:/tmp/docker-git-install.apk
docker exec dg-my-project-android adb -s emulator-5554 install /tmp/docker-git-install.apk
```

## Browser WebUSB Phone UI

Use the built-in browser connector when you want to attach a real Android phone from the user's computer without installing host ADB:

```bash
rust-android-connection web --port 8080
```

Then open:

```text
http://127.0.0.1:8080/
```

The page uses WebUSB/WebADB in Chromium-compatible browsers, serves its HTML/CSS/JS shell from the Rust binary, and imports a pinned Tango WebADB stack from `esm.sh`. It does not start Docker, does not require `adb` on the host, and does not silently enumerate new USB devices. The browser can list only already-authorized WebUSB devices until the user clicks `Connect phone` and grants USB access. If Android authorization stalls, use `Copy diagnostics` from the session log and reset browser-side USB/ADB state with `Forget USB/ADB keys`.

Requirements for a physical phone:

- Chromium-compatible browser with WebUSB support.
- Localhost or HTTPS secure context.
- USB debugging enabled on the Android phone.
- User approval for the browser USB picker and the Android RSA debugging prompt.

## Smoke Test

```bash
rust-android-connection dg-my-project start --runtime-profile app-test --dry-run
rust-android-connection dg-my-project adb --dry-run shell getprop sys.boot_completed
rust-android-connection dg-my-project install-apk --dry-run app.apk
rust-android-connection dg-my-project launch-app --dry-run --package com.example.app
rust-android-connection web --port 8080 --dry-run
```

Expected: every command returns deterministic JSON with the Docker/ADB command or browser endpoint it would use. Remove `--dry-run` from lifecycle/ADB commands to run against a started Android container, or from `web` to serve the local browser connector.

## Development

```bash
cargo fmt --check
cargo test --locked
cargo build --locked --bins
cargo clippy --locked --all-targets --all-features -- -D warnings
```
