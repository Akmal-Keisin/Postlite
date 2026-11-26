# Postlite GPUI desktop port

The GPUI workbench opens legacy and current Postlite workspaces. It supports nested collections, request and environment tabs, environment binding, collection and folder variables, request editing, static authentication, history, saved examples, Postman import and export, cURL import and copy, and workspace backup and restore. Both desktop apps send through the same Rust HTTP engine. The four appearance presets use the original palettes, and the native workbench supports dark, light, and system themes, the eight accents, and stacked and side-by-side panels.

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

## Verification and performance

Use the Linux X11 smoke test to open a nested collection request and send it to a local echo server. It checks the authored request's variable resolution, wire body and auth header, history, environment tab selection, and workspace persistence.

```sh
cargo test --release --locked --manifest-path src-gpui/Cargo.toml
cargo test --release --locked --manifest-path src-tauri/Cargo.toml
python3 .verify/gpui-workbench-smoke.py
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
| Window mapped | 82.9 ms | 125.6 ms |
| First visible content | 574.0 ms | 209.9 ms |
| Idle process-tree PSS | 269.6 MiB | 50.9 MiB |

This comparison covers startup with a saved collection, environment, and open request. The GPUI branch is still working toward full feature and visual parity. OAuth, cookie storage, script execution, the collection runner, HTML/PDF previews, alternate import formats, code targets beyond cURL, and installer verification remain part of the [parity design](../docs/superpowers/specs/2026-11-26-gpui-parity-design.md). Some controls and secondary screens still differ from the reference.
