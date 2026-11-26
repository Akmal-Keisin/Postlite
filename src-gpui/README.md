# Postlite GPUI desktop port

The GPUI workbench opens legacy and current Postlite workspaces. It supports nested collections, request and environment tabs, environment binding, collection and folder variables, request editing, history, saved examples, Postman import and export, cURL import and copy, and workspace backup and restore. Both desktop apps send through the same Rust HTTP engine and share the cookie jar, proxy credential store, and OAuth token exchange. The four appearance presets use the original palettes, and the native workbench supports dark, light, and system themes, the eight accents, and stacked and side-by-side panels.

## Build and run

From the repository root:

```sh
cargo build --release --locked --manifest-path src-gpui/Cargo.toml
./src-gpui/target/release/postlite-gpui
```

The GPUI app looks for its own workspace in the OS app-data directory under `app.postlite.gpui`. On first launch it can load the original Tauri workspace. Edits save to the separate GPUI profile. To open a specific fixture or use a writable copy at a chosen location, pass one of these flags.

```sh
./src-gpui/target/release/postlite-gpui --workspace /path/to/workspace.json
./src-gpui/target/release/postlite-gpui --write-workspace /path/to/isolated-copy.json
```

`--workspace` keeps edits in memory. The save code rejects paths inside the live Tauri app-data directory. The lockfile pins compatible `libc` and `mio` versions because GPUI 0.2.2's archive dependency references the former Linux `libc::ENOATTR` constant.

## Cookies, proxies, and OAuth

Open **Cookies** in the request toolbar to inspect, copy, or delete cookies for the current workspace. **Application settings → Request** selects Direct, Manual, or From environment for proxies. **Session and privacy** contains the cookie persistence and clear controls.

Select **OAuth 2.0** in a request's Auth tab, or configure it by right-clicking a collection or folder. An inheriting request uses the nearest parent with explicit auth. Choose **Get token** for client credentials or **Authorize** for authorization code with PKCE. You can paste a redirect URL or code if the browser callback cannot reach the loopback listener.

A writable GPUI workspace stores eligible cookies and credentials in an encrypted side directory with keys in the OS credential store. Each workspace copy has a separate namespace. Re-enter proxy passwords and OAuth client secrets when switching from Tauri. Read-only runs keep their session data in memory.

## Verification and performance

Use the Linux X11 smoke test to open a nested collection request and send it to a local echo server. It checks the authored request's variable resolution, wire body and auth header, history, environment tab selection, and workspace persistence.

```sh
cargo test --release --locked --manifest-path src-gpui/Cargo.toml
cargo test --release --locked --manifest-path src-tauri/Cargo.toml
cargo test --release --locked --manifest-path postlite-core/Cargo.toml
python3 .verify/gpui-workbench-smoke.py
python3 .verify/gpui-workbench-smoke.py --sessions
```

The benchmark runs the release binaries on an X11 or XWayland desktop with `xdotool`. It uses an isolated profile for each app, discards a warmup, alternates run order, and measures window mapping and first visible content separately. Proportional set size (PSS) includes each app's process tree and is sampled two seconds after first visible content.

```sh
cargo build --release --locked --manifest-path src-tauri/Cargo.toml
cargo build --release --locked --manifest-path src-gpui/Cargo.toml
python3 .verify/gpui-benchmark.py --runs 5 --workspace /path/to/fixture.json
```

Five warm launches on Linux/XWayland with an AMD Ryzen 5 6600H and the same populated workbench fixture gave these medians.

| Metric | Tauri | GPUI |
| --- | ---: | ---: |
| Window mapped | 82.7 ms | 125.3 ms |
| First visible content | 498.6 ms | 168.8 ms |
| Idle process-tree PSS | 282.3 MiB | 60.7 MiB |

In a separate five-run comparison of the GPUI builds before and after adding sessions, median first visible content went from 210.1 ms to 208.8 ms and idle PSS went from 57.5 MiB to 59.9 MiB.

This comparison covers startup with a saved collection, environment, and open request. Script execution, the collection runner, HTML/PDF previews, alternate import formats, code targets beyond cURL, installer verification, and visual matching for controls and secondary screens remain in the [parity design](../docs/superpowers/specs/2026-11-26-gpui-parity-design.md).
