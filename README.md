# postlite

A lightweight desktop HTTP client built with [Tauri v2](https://tauri.app) + Rust. It covers the day-to-day request workflow — send HTTP calls, organize them into collections, reuse variables and auth, and move data in and out with Postman's JSON formats — in a small native window instead of a browser tab.

> **Not affiliated with Postman.** postlite is an independent project. "Postman" is a trademark of Postman, Inc.; the only relationship is that postlite can import and export Postman's open JSON formats.

## Features

- **Workspaces** — multiple workspaces, each with its own collections, environments, and open tabs. Create, rename, and switch from the top bar; deleting a workspace (with confirmation) removes everything in it. Older single-workspace files are migrated automatically.
- **HTTP core** — every standard verb plus `QUERY`, URL with query params, headers, and request bodies (none / raw / form-data). Responses show status, latency, size, headers, and a pretty-printed JSON view with syntax highlighting; the body can be copied in one click.
- **Collections** — nested folders, create/rename/delete, save the active request into any folder, a filterable tree, expand/collapse, and a draggable sidebar.
- **Postman interop** — import and export Collection v2.1, and import/export Environment JSON, so you can move work between postlite and Postman.
- **cURL & code** — the **Code** link under Send opens a right-side drawer that copies the active request as a runnable `curl` command, generates it as JavaScript (fetch / axios), PHP (cURL / Guzzle), Python (requests / urllib), or Go (`net/http`), and turns a pasted `curl` command back into a request tab. Output carries resolved variables and inherited auth, so the same secret warning applies; flags postlite cannot express (`-x`, `--compressed`) are named on import rather than guessed.
- **Cookies** — a persistent cookie jar per workspace, on by default. Persistent cookies are stored encrypted (AES-256-GCM file whose key lives in the OS secret store) until they expire or you delete them; session cookies die when the app closes. Server expiration rules are respected, and a manual `Cookie` header always beats the jar. The **Cookies** link under Send opens a Cookie Manager drawer: rows grouped by domain with Secure / HttpOnly / SameSite chips, per-row reveal/copy/delete, domain filtering and domain-wide delete. Settings holds the global **Persist cookies** switch and **Clear all cookies**; turning persistence off deletes the saved jar, and deleting a workspace deletes its jar and key. Cookies are never written to `workspace.json` or backups.
- **Proxy** — direct, manual (URL + optional username/password), or environment variables (`HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY`), with a bypass list for hosts that connect directly. Proxy credentials live in the OS secret store, never in settings or backups.
- **Auth** — none / basic / bearer / API-key, set at request, folder, or collection level with inheritance.
- **Variables** — `{{token}}` interpolation with collection, folder, and environment scopes. Precedence is `collection < environment < folder`. A peek overlay under the URL bar resolves each token and names the scope it came from. Values can be flagged **secret** and are masked in the UI (raw values are still sent).
- **Environments** — managed in their own sidebar tab, scoped per workspace, bound to a collection through the selector in the top bar (next to **Import**). Bindings persist per collection.
- **Scripts** — pre-request and test scripts run against a minimal `pm` sandbox (`pm.request`, `pm.response`, and variables); results appear in the response Tests tab.
- **Settings** — a global preferences dialog (open with `Ctrl+,`) covers theme (light / dark / system), auto-save and session restore, request defaults (timeout, redirect policy, TLS verification, cache, response size cap), the Cookies and Proxy groups, response view options (default tab, syntax highlighting, JSON indent, word wrap), privacy masking, the fixed keyboard shortcuts, and reset. Values live in `workspace.json` under `settings`, and the whole block can be exported or imported as JSON.
- **Persistence** — collections, tabs, environments, and settings are saved to a single JSON file in the OS app-data directory. **Import / Export** in settings can also write that whole state to one `postlite.backup` file and read it back, either merged beside the workspaces you already have or replacing them (replace confirms twice and restores the backup's preferences). Cookie jars and proxy credentials are deliberately excluded from backups.
- **Privacy masking** — sensitive headers (`Authorization`, `Cookie`, anything named token/secret/key) are masked in the request header grid, the response header table, and the Auth tab, with a per-row reveal; secret variables are masked in editors and the peek overlay. Masking is display-only: everything that leaves the app (send, copy, export, generated code) carries the real values. Both masks can be switched off in Settings → Privacy.

## Tech stack

- **Tauri v2** for the native shell and IPC.
- **Rust** backend: `reqwest` for HTTP, `cookie_store` for the jar, `aes-gcm` + `keyring` for encrypted cookie/credential storage, `serde`/`serde_json` for the workspace file. Commands: `http_send`, `load_workspace`, `save_workspace`, `cookie_list` / `cookie_delete` / `cookie_purge` / `cookie_set_persist`, and `proxy_credentials_set` / `proxy_credentials_get`.
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

The file is plain JSON (`version`, `settings`, `activeWorkspaceId`, `workspaces` — each with `collections`, `environments`, `tabs`, `activeTabId`, `activeTabIndex`), so it is easy to back up or move between machines.

Secrets are kept out of that file, in the same directory:

- `cookies/<workspaceId>.bin` — the workspace's persistent cookie jar, AES-256-GCM encrypted. The random key lives in the OS secret store (Windows Credential Manager / macOS Keychain / Linux Secret Service) under `app.postlite.desktop.postlite.store`. No secret store available ⇒ cookies still work for the session but nothing is saved, and the Cookies drawer says why.
- `secrets/global.bin` — the manual proxy credentials, encrypted the same way.

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

The settings and interop harnesses use a separate debug build so they can never touch a real workspace: set `identifier` to `app.postlite.tier1verify20261110` and add `build.devUrl` `http://127.0.0.1:8100` in `src-tauri/tauri.conf.json` while building it, then revert both. `node .verify/tier1-server.mjs` serves the UI (plus a self-signed TLS endpoint) on `127.0.0.1:8100`/`8101` with a `window.__verify` hook, and the app is launched with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9235`. Run:

```sh
node .verify/tier1-verify.mjs     # preferences, masking, export warnings (60 checks)
node .verify/tier1-edge.mjs       # restart, restore-session, legacy migration (25)
node .verify/tier1-close.mjs      # autosave on window close (closes the app)
node .verify/tier2a-codegen.mjs   # cURL/code logic, no browser needed (20)
node .verify/tier2a-execute.mjs   # generated snippets run against a live fixture
node .verify/tier2a-verify.mjs    # Code drawer, cURL import, backup/restore in the app (48)
node .verify/tier2b-verify.mjs    # cookies, masking, proxy on the wire (36)
# then restart the isolated app and run:
node .verify/tier2b-verify.mjs --after-restart   # session gone, persistent back (5)
# or relaunch it with POSTLITE_FORCE_NO_SECRET_STORE=1 and run:
node .verify/tier2b-verify.mjs --no-secret       # nothing written, drawer explains (20)
```

`tier2b-verify` runs its own forward proxy on `127.0.0.1:8890` to prove manual-proxy routing and bypass on the wire, and uses the harness server's `/set` and `/echo` routes to inspect exactly which cookies were sent. `POSTLITE_FORCE_NO_SECRET_STORE` is only honored in debug builds. After a verification session, delete the isolated app-data directory and any `*.tier1verify20261110.postlite.store` credentials so nothing of it survives.

## License

Distributed under the MIT License. See [LICENSE](LICENSE) for details.
