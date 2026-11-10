# postlite

A lightweight desktop HTTP client built with [Tauri v2](https://tauri.app) + Rust. It covers the day-to-day request workflow — send HTTP calls, organize them into collections, reuse variables and auth, and move data in and out with Postman's JSON formats — in a small native window instead of a browser tab.

> **Not affiliated with Postman.** postlite is an independent project. "Postman" is a trademark of Postman, Inc.; the only relationship is that postlite can import and export Postman's open JSON formats.

## Features

- **HTTP core** — every standard verb plus `QUERY`, URL with query params, headers, and request bodies (none / raw / form-data). Responses show status, latency, size, headers, and a pretty-printed JSON view with syntax highlighting; the body can be copied in one click.
- **Collections** — nested folders, create/rename/delete, save the active request into any folder, a filterable tree, expand/collapse, and a draggable sidebar.
- **Postman interop** — import and export Collection v2.1, and import/export Environment JSON, so you can move work between postlite and Postman.
- **Auth** — none / basic / bearer / API-key, set at request, folder, or collection level with inheritance.
- **Variables** — `{{token}}` interpolation with collection, folder, and environment scopes. Precedence is `collection < environment < folder`. A peek overlay under the URL bar resolves each token and names the scope it came from. Values can be flagged **secret** and are masked in the UI (raw values are still sent).
- **Environments** — a global list managed in its own sidebar tab, bound to a collection through the selector in the top bar (next to **Import**). Bindings persist per collection.
- **Scripts** — pre-request and test scripts run against a minimal `pm` sandbox (`pm.request`, `pm.response`, and variables); results appear in the response Tests tab.
- **Persistence** — collections, tabs, environments, and settings are saved to a single JSON file in the OS app-data directory.

## Tech stack

- **Tauri v2** for the native shell and IPC.
- **Rust** backend: `reqwest` for HTTP, `serde`/`serde_json` for storage. Three commands only: `http_send`, `load_workspace`, `save_workspace`.
- **Static frontend** in plain HTML/CSS/JS (`ui/`) — no bundler, no npm dependencies.

## Requirements

- Rust (stable) toolchain.
- Tauri v2 system prerequisites: on Windows, the Microsoft Visual Studio C++ Build Tools and the WebView2 runtime; on macOS, Xcode command line tools; on Linux, the usual WebKit/GTK dev packages. See the [Tauri prerequisites guide](https://tauri.app/start/prerequisites/).
- Node.js is only needed to run the local verification harness and helper scripts.

## Getting started

```sh
# Hot reload during development (needs the tauri-cli: cargo install tauri-cli)
cd src-tauri
cargo tauri dev

# Production build (embeds the static UI from ../ui)
cd src-tauri
cargo build --release
# -> src-tauri/target/release/postlite(.exe)

# Installer/bundle (Windows NSIS target is preconfigured)
cd src-tauri
cargo tauri build
```

Because the UI is a static folder embedded at build time, edits to files in `ui/` need a rebuild to show up in the packaged app.

## Where your data lives

Everything is stored in one file under the OS app-data directory for identifier `app.postlite.desktop`:

- **Windows:** `%APPDATA%\app.postlite.desktop\workspace.json`
- **macOS:** `~/Library/Application Support/app.postlite.desktop/workspace.json`
- **Linux:** `~/.local/share/app.postlite.desktop/workspace.json`

The file is plain JSON (`collections`, `tabs`, `activeTabId`, `environments`), so it is easy to back up or move between machines.

## Project layout

```
postlite/
  ui/                static frontend (index.html, app.js, styles.css)
  src-tauri/         Rust + Tauri shell (main.rs, tauri.conf.json, Cargo.toml, icons)
  scripts/         helper scripts (icon generation, Windows shortcut)
  docs/              design specs
  .verify/         CDP verification harnesses and JSON fixtures (see below)
```

## Development and verification

postlite is verified by driving the real app over Chrome DevTools Protocol rather than unit tests:

1. Build the release binary.
2. Launch it with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9229`.
3. Start the local echo server: `node .verify/server.mjs` (listens on `127.0.0.1:8099`).
4. Run the harnesses against the debugging port:
   ```sh
   node .verify/verify.mjs 9229    # functional checks (not idempotent — seed a fresh fixture first)
   node .verify/visual.mjs 9229    # screenshots + runtime-exception capture
   node .verify/resize.mjs 9229    # sidebar resize behaviour
   ```

`.verify/workspace.*.json` files are local test/backup workspaces and, together with the generated screenshots, are ignored by git so real request data is never committed.

## License

Distributed under the MIT License. See [LICENSE](LICENSE) for details.
