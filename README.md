# Postlite

A lightweight desktop HTTP client built with [Tauri v2](https://tauri.app) and Rust. It covers the day-to-day request workflow: send HTTP calls, organize them into collections, reuse variables and auth, and move data in and out with Postman's JSON formats. It runs in a small native window instead of a browser tab.

> **Not affiliated with Postman.** postlite is an independent project. "Postman" is a trademark of Postman, Inc.; the only relationship is that postlite can import and export Postman's open JSON formats.

---

## Features

### Requests

- **HTTP core**: every standard verb plus `QUERY`, URL query params, headers, and request bodies (none / raw / form-data).
  - **Form-data**: each row has a **Text / File** type. File rows open a native picker and upload the file's bytes as a multipart part (filename and Content-Type guessed from the extension). Text rows send plain fields. Empty File rows are skipped on send.
  - **Description column**: query and form-data rows carry an optional description (Postman's third column). It survives URL-bar edits and round-trips through Postman JSON.
  - **Path variables**: `:product_slug`-style segments get their own **Path Variables** table. Values are substituted URL-encoded on send and in generated code / cURL, and round-trip through Postman's `url.variable`.
  - **File fields** round-trip through Postman's `formdata` `type: "file"` + `src`. Generated cURL uses `--form key=@path`.
  - **Responses**: status, latency, size, headers, and a pretty-printed JSON view with syntax highlighting. The body copies in one click.
  - **Collapsible JSON**: click any object or array line (or its caret) to fold it into a one-line summary with a key/item count. **Collapse all** / **Expand all** sit above the body. Huge arrays start folded, children render lazily, fold state survives tab switches, and the indent follows the JSON indent preference.

- **Examples**: save any successful response as a named example (bookmark button in the response head).
  - The **Examples** picker lists them with status and date. Opening one swaps the response stage to the saved snapshot under a banner, with **Back to live response** to return. Sending again always returns to the live response.
  - Examples persist with the request, survive edits and re-saves, and map to Postman's saved `response` entries. Deleting one asks for confirmation.

- **cURL & code**: the **Code** link under Send opens a right-side drawer that:
  - copies the active request as a runnable `curl` command,
  - generates JavaScript (fetch / axios), PHP (cURL / Guzzle), Python (requests / urllib), or Go (`net/http`),
  - turns a pasted `curl` command back into a request tab.
  - Output carries resolved variables and inherited auth, so the same secret warning applies. Flags postlite cannot express (`-x`, `--compressed`) are named on import rather than guessed.

### Collections, workspaces & variables

- **Workspaces**: multiple workspaces, each with its own collections, environments, and open tabs. Create, rename, and switch from the top bar. Deleting a workspace (with confirmation) removes everything in it. Older single-workspace files migrate automatically.

- **Collections**: nested folders, create / rename / delete, save the active request into any folder, a filterable tree, expand / collapse, and a draggable sidebar.
  - **Reordering**: drag any collection, folder, or request by its row (a grip appears on hover, and a line marks the insertion point). Or use Alt+Up/Down on a focused row. Order persists at every level and stays correct while the tree is filtered.
  - **Rename**: right-click a request row (or its tab). The new name lands in the tree and every open tab at once.
  - **Tabs**: the tab strip scrolls horizontally when tabs overflow, with a thin scrollbar and wheel support, and keeps the active tab in view.

- **History**: every send — manual or through the runner — lands in a per-workspace **History** panel (the third rail view). Rows carry method, name/URL, status, latency, and a relative time; consecutive identical sends collapse with a run count, and the list is capped at 100 entries. Clicking a row re-opens what was sent: saved requests select their existing tab, unsaved scratch sends restore a snapshot clone, deleted requests say so. The sidebar filter narrows rows, **Clear all** wipes the workspace's history after confirmation, and history survives restarts independent of the "restore last session" setting. History is excluded from backups.

- **Auth**: none / basic / bearer / API-key, set at request, folder, or collection level, with inheritance.

- **Variables**: `{{token}}` interpolation with collection, folder, and environment scopes.
  - Precedence: `collection < environment < folder`.
  - A peek overlay under the URL bar resolves each token and names the scope it came from.
  - Values can be flagged **secret** and are masked in the UI. Raw values are still sent.
  - Duplicate path slashes fold in joined URLs, so `{{api_url}}/api/...` works whether or not the variable ends in `/`. The scheme and query string are untouched.

- **Environments**: managed in their own sidebar tab, scoped per workspace, and bound to a collection through the selector in the top bar (next to **Import**). Bindings persist per collection.

### Automation

- **Scripts**: pre-request and test scripts run in a minimal `pm` sandbox: `pm.request`, `pm.response`, `pm.variables`, `pm.environment`, and `pm.collectionVariables`. Results appear in the response **Tests** tab.

  ```js
  // Writes into the bound environment and persists it, so the next request can use {{token}}
  pm.environment.set("token", pm.response.json().data.token);

  // Writes into the collection's own Variables table, so it stays with that collection
  pm.collectionVariables.set("token", pm.response.json().data.token);

  // Session-only; not saved
  pm.variables.set("tmp", "1");
  ```

- **Collection runner**: right-click a collection or folder, pick requests in a checkbox tree, and run them in order through the exact Send pipeline (scripts, auth, environments, cookies).
  - The run stops at the first failed test or transport error. A passing 404 negative test keeps going.
  - Live progress, a Stop button, and per-request results with test details and a body preview.
  - Results are view-only. Nothing is saved.

### Interop

- **Postman**: import and export Collection v2.1, and import / export Environment JSON.
- **Drag and drop**: drop files anywhere in the window. postlite detects the content and routes it:
  - Postman collections and environments go to the sidebar,
  - postlite backups go to the restore dialog,
  - postlite settings go to preferences,
  - cURL scripts open in a new tab. A script that fails to parse opens in the cURL dialog instead.

### Security & privacy

- **Cookies**: a persistent cookie jar per workspace, on by default.
  - Persistent cookies are stored encrypted (AES-256-GCM). The key lives in the OS secret store. Session cookies die when the app closes.
  - Server expiration rules are respected. A manual `Cookie` header always beats the jar.
  - The **Cookies** link under Send opens a Cookie Manager drawer: rows grouped by domain, Secure / HttpOnly / SameSite chips, per-row reveal / copy / delete, domain filtering, and domain-wide delete.
  - Settings holds the **Persist cookies** switch and **Clear all cookies**. Turning persistence off deletes the saved jar. Deleting a workspace deletes its jar and key.
  - Cookies are never written to `workspace.json` or backups.

- **Proxy**: direct, manual (URL plus optional username / password), or environment variables (`HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY`). A bypass list sends selected hosts direct. Proxy credentials live in the OS secret store, never in settings or backups.

- **Privacy masking**: sensitive headers (`Authorization`, `Cookie`, and anything named token / secret / key) are masked in the request header grid, the response header table, and the Auth tab, with per-row reveal. Secret variables are masked in editors and the peek overlay.
  - Masking is display-only. Everything that leaves the app (send, copy, export, generated code) carries the real values.
  - Both masks can be switched off in **Settings → Privacy**.

### Settings & data

- **Settings**: a preferences dialog (`Ctrl+,`) covers:
  - appearance: light / dark / system theme, plus eight accent presets, each with a tuned dark and light variant,
  - auto-save and session restore,
  - request defaults: timeout, redirect policy, TLS verification, cache, response size cap,
  - the Cookies and Proxy groups,
  - response view: default tab, syntax highlighting, JSON indent, word wrap,
  - privacy masking, the fixed keyboard shortcuts, and reset.
  - A search bar (`Ctrl+F`, or `/` while the dialog is open) filters settings live, badges each category with its match count, and jumps to the first hit. `Escape` clears the query before it closes the dialog.
  - Values live in `workspace.json` under `settings`. The whole block can be exported or imported as JSON.

- **Persistence**: collections, tabs, environments, and settings save to a single JSON file in the OS app-data directory.
  - **Import / Export** in Settings can write the whole state to one `postlite.backup` file and read it back. Restore either merges beside your existing workspaces or replaces them. Replace asks twice and restores the backup's preferences.
  - Every export (collection, environment, settings, backup, code snippet) opens the native Save As dialog first. Cancelling writes nothing. The status line reports the path written.
  - Cookie jars and proxy credentials are deliberately excluded from backups.

---

## Tech stack

| Layer | Choice |
| --- | --- |
| Shell & IPC | Tauri v2 |
| Backend | Rust: `reqwest` (HTTP), `cookie_store` (jar), `aes-gcm` + `keyring` (encrypted cookies / credentials), `serde` / `serde_json` (workspace file), `rfd` (native Save As) |
| Frontend | Plain HTML / CSS / JS in `ui/`. No bundler, no npm dependencies. |

Commands exposed to the frontend: `http_send`, `load_workspace`, `save_workspace`, `cookie_list` / `cookie_delete` / `cookie_purge` / `cookie_set_persist`, `proxy_credentials_set` / `proxy_credentials_get`, and `save_export_file`.

---

## Requirements

- Rust (stable) toolchain.
- Tauri v2 system prerequisites. See the [Tauri prerequisites guide](https://tauri.app/start/prerequisites/).
  - **Windows**: Microsoft Visual Studio C++ Build Tools and the WebView2 runtime.
  - **macOS**: Xcode command line tools.
  - **Linux**: the usual WebKit / GTK dev packages.
- Node.js is only needed to run the local verification harness and helper scripts.

---

## Getting started

```sh
# Hot reload during development (needs tauri-cli: cargo install tauri-cli)
cd src-tauri
cargo tauri dev

# Production build (embeds the static UI from ../ui)
cd src-tauri
cargo build --release
# -> src-tauri/target/release/postlite(.exe)

# Installer / bundle (Windows NSIS target is preconfigured)
cd src-tauri
cargo tauri build
```

The UI is a static folder embedded at build time. Edits to `ui/` need a rebuild before they show up in the packaged app.

---

## Where your data lives

Everything is stored in one file under the OS app-data directory for identifier `app.postlite.desktop`:

| OS | Path |
| --- | --- |
| Windows | `%APPDATA%\app.postlite.desktop\workspace.json` |
| macOS | `~/Library/Application Support/app.postlite.desktop/workspace.json` |
| Linux | `~/.local/share/app.postlite.desktop/workspace.json` |

The file is plain JSON: `version`, `settings`, `activeWorkspaceId`, and `workspaces`. Each workspace holds `collections`, `environments`, `tabs`, `activeTabId`, and `activeTabIndex`. It is easy to back up or move between machines.

Secrets stay out of that file, in the same directory:

- `cookies/<workspaceId>.bin`: the workspace's persistent cookie jar, AES-256-GCM encrypted. The random key lives in the OS secret store (Windows Credential Manager / macOS Keychain / Linux Secret Service) under `app.postlite.desktop.postlite.store`. If no secret store is available, cookies still work for the session but nothing is saved, and the Cookies drawer says why.
- `secrets/global.bin`: the manual proxy credentials, encrypted the same way.

---

## Project layout

```
postlite/
├── ui/              static frontend (index.html, app.js, styles.css)
├── src-tauri/       Rust + Tauri shell (main.rs, tauri.conf.json, Cargo.toml, icons)
├── scripts/         helper scripts (icon generation, Windows shortcut)
├── docs/            design specs
└── .verify/         CDP verification harnesses and JSON fixtures
```

---

## Development & verification

### CI and releases

- Every pull request and push to `main` runs CI (`.github/workflows/ci.yml`): `cargo test` and a release build on Windows and Ubuntu, with the binaries attached as workflow artifacts.
- Releases are tag-driven. Push a `v*` tag, or run the Release workflow manually, to build the installers and attach them to a GitHub release.

### Verification harnesses

postlite is verified by driving the real app over the Chrome DevTools Protocol rather than unit tests.

**Manual flow**

1. Build the release binary.
2. Launch it with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9229`.
3. Start the local echo server: `node .verify/server.mjs` (listens on `127.0.0.1:8099`).
4. Run the harnesses against the debugging port:

   ```sh
   node .verify/verify.mjs 9229    # functional checks (not idempotent: seed a fresh fixture first)
   node .verify/visual.mjs 9229    # screenshots + runtime-exception capture
   node .verify/resize.mjs 9229    # sidebar resize behaviour
   ```

`.verify/workspace.*.json` files are local test and backup workspaces. Together with the generated screenshots, they are git-ignored, so real request data is never committed.

**Isolated debug build (settings, interop, runner)**

These harnesses use a separate debug build, so they never touch a real workspace:

1. In `src-tauri/tauri.conf.json`, set `identifier` to `app.postlite.tier1verify20261110` and add `build.devUrl` `http://127.0.0.1:8100`. Build, then revert both.
2. Serve the UI with `node .verify/tier1-server.mjs`. It listens on `127.0.0.1:8100` (UI) and `8101` (self-signed TLS endpoint) and exposes a `window.__verify` hook.
3. Launch the app with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9235`.
4. Run the suites:

| Harness | Covers | Checks |
| --- | --- | --- |
| `tier1-verify.mjs` | preferences, settings search, native export, masking | 75 |
| `tier1-edge.mjs` | restart, restore-session, legacy migration | 25 |
| `tier1-close.mjs` | autosave on window close (closes the app) | — |
| `tier2a-codegen.mjs` | cURL / code logic (no browser needed) | 20 |
| `tier2a-execute.mjs` | generated snippets run against a live fixture | — |
| `tier2a-verify.mjs` | Code drawer, cURL import, backup / restore | 49 |
| `tier2b-verify.mjs` | cookies, masking, proxy on the wire | 36 |
| `tier2b-verify.mjs --after-restart` | session gone, persistent cookies back | 5 |
| `tier2b-verify.mjs --no-secret` | nothing written; drawer explains (relaunch with `POSTLITE_FORCE_NO_SECRET_STORE=1`) | 20 |
| `tier2c-dnd-verify.mjs` | drag & drop import, tree reordering | 52 |
| `runner-verify.mjs` | collection runner: selection, ordering, env chaining, stop semantics, `pm.collectionVariables` | 43 |
| `history-verify.mjs` | request history: model, recording, panel, click-through, backup exclusion | 24 |

`tier2b-verify` runs its own forward proxy on `127.0.0.1:8890` to prove manual-proxy routing and bypass on the wire. It uses the harness server's `/set` and `/echo` routes to inspect exactly which cookies were sent.

> **Cleanup:** `POSTLITE_FORCE_NO_SECRET_STORE` is only honored in debug builds. After a verification session, delete the isolated app-data directory and any `*.tier1verify20261110.postlite.store` credentials so nothing survives.

---

## License

Distributed under the MIT License. See [LICENSE](LICENSE) for details.
