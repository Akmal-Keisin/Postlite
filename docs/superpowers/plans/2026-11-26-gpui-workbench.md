# GPUI Workbench and Saved Requests Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development for independent implementation units. Verify the integrated workflow on the running app.

**Goal:** Open an existing workspace in the GPUI desktop app, navigate its nested collections and request tabs, bind an environment, and send through the existing Rust transport.

**Architecture:** Share the HTTP engine with Tauri through a platform-independent Rust crate. Load the portable workspace model independently of GPUI and prepare each request from its saved values and selection. GPUI renders and controls the workbench.

**Tech stack:** GPUI, Serde, Tokio, reqwest, Linux native input verification.

**Spec:** `docs/superpowers/specs/2026-11-26-gpui-parity-design.md`.

## Global constraints

- Keep v1 and v2 workspace compatibility. Use an explicit fixture or a read-only source for this milestone. Keep the experimental app from writing to Tauri's live data.
- Preserve saved method, body, inherited static authentication, variable scopes and disabled rows, path substitution, and query encoding.
- Verify against the current Tauri release and synthetic fixtures. Keep HTTP delivery and outcome handling off the UI thread.
- Keep ordinary startup free of webview initialization. Use the existing palette and workbench geometry as the visual reference.

## Review focus

- Legacy nested folders and open request tabs must load without unchecked shape assumptions.
- Workspace changes and request switching cannot apply an in-flight result to a different tab.
- Static inherited authentication follows the nearest explicit scope and preserves duplicate header values as the existing transport does.
- Path variables and query strings must use the same encodings as the current workbench on the wire.
- HTTP core extraction must preserve existing Tauri behavior and all backend tests.

## Tasks

- [x] Extract the existing HTTP backend to `postlite-core`. Keep a Tauri adapter and provide a direct Rust invocation that returns its existing response. Run all existing Tauri backend tests.
- [x] Implement workspace loading in `src-gpui/src/workspace.rs`. Test schema v1, v2, malformed data, active workspace, and tab restoration. Preserve complete saved request objects for Send.
- [x] Implement request preparation in `src-gpui/src/prepare.rs`. Record the current behavior for conflicting scopes, inherited auth, multipart/URL-encoded bodies, encoded paths, and query; replay it against the Rust implementation.
- [ ] Implement GPUI navigation and the request/response workbench in `src-gpui/src/main.rs` and `src-gpui/src/workbench.rs`. Test a populated fixture with two workspaces, nested folders, env switching, tab restoration, selection and request Send.
- [ ] Build and test both apps. Capture and compare the default GPUI workbench against the Tauri fixture and record visual differences honestly. Re-run startup benchmarks with the populated fixture. Update the branch README with completed and pending scope.

The remaining editing/session, interoperability, and security/release work stays in the overall design and is delivered in subsequent testable units.

## Verification on Linux

The release builds passed 81 Tauri tests and 35 GPUI tests. The native smoke test opens a nested saved request through the collection tree, verifies path and query encoding, folder and environment scope resolution, inherited bearer auth and request body on the wire, and checks isolated session and history persistence. The matching-workspace startup comparison is recorded in src-gpui/README.md.

The workbench is runnable on the branch. The Linux reference still shows visual differences in several controls and secondary panels. Full cross-platform screenshot coverage, the remaining features, and installer verification are open.
