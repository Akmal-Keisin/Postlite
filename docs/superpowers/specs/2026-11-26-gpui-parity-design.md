# Postlite GPUI migration

## Goal

Deliver a GPUI desktop app with every user-facing feature and the same appearance as Postlite at `72c5ddc`. Keep the current workspace format and release targets. Measure startup and idle memory again once the two apps do the same work. The desktop app is the parity target. The website and the separate MCP project are outside this migration.

## Approach

Keep the Tauri and GPUI apps runnable side by side during the port. Use the current Tauri app as the reference for each workflow and use the GPUI branch for the replacement. Reuse the existing Rust backend for HTTP, cookies, proxy, OAuth, credentials, and persistence by moving desktop-independent logic behind ordinary Rust interfaces. Leave platform calls such as locating app data and opening dialogs in each app's entry point. Replace the prototype transport when the GPUI Send action is connected to the shared backend.

A direct one-pass replacement would make it harder to isolate differences and risk making the branch unusable between features. Keeping Tauri and porting only the frontend language would preserve browser rendering but retain the webview on startup. Native GPUI views are the selected approach.

## Visual acceptance

- Treat `ui/index.html`, `ui/styles.css`, and `design.md` as the reference. Compare screenshots with the same fixture, operating system, system fonts, scale, window size, and UI state. Compare positions, dimensions, colors, typography, icon shapes, and interaction states. Allow renderer-specific antialiasing at glyph and shape edges.
- Cover Workbench, Terminal, Studio, and Ledger in light and dark mode, system theme changes, and all eight accent presets. Check default, minimum, and wide windows. Include the rail, workspace and environment selectors, collection tree, tabs, request and response panes, settings, menus, drawers, empty states, dialogs, focus, and disabled controls.
- Preserve the 840 px workbench-width split threshold, divider drag and keyboard behavior, independent stacked and split sizes, and layout restoration. Preserve accessibility names, focus order, shortcuts, high-contrast state, and reduced-motion behavior on the supported platforms.

## Data and runtime acceptance

- Read legacy and v2 workspaces, preserve IDs and saved structure, and keep imports, exports, backups, and secrets compatible. Verify restart, autosave, failed writes, and switching workspaces. Verify at cutover that an existing user with nondefault sidebar and response sizes keeps their stacked and split sizes and layout choice. The original webview stores some of those preferences outside `workspace.json`.
- Use temporary profiles and synthetic workspaces for development and comparisons. Keep prototype writes separate from the user's existing workspace while both apps are available. Preserve the production app identifier and OS secret-store keys at cutover, with the original app closed during migration.
- Treat the existing Rust tests and `.verify/` workflows as behavior contracts. Use matching data and local echo, proxy, and OAuth fixtures for wire-level comparisons. Exercise GPUI flows with native input and screenshots. Verify response errors, byte-for-byte export where appropriate, and legacy data before marking a workflow complete.
- Initialize the request client, scripting runtime, and document previews on demand. Preserve the documented JavaScript `pm` API and script order in an isolated engine so existing scripts continue to work. The proposed HTML/PDF renderer opens a system webview only inside Preview, with the current HTML sandbox and PDF byte handling. Verify in-panel rendering on Windows, XWayland, and native Wayland before accepting that workflow. Wry can use an X11 child window, while native Wayland needs a separate integration proof.

## Delivery order

| Milestone | Deliverable | Acceptance |
| --- | --- | --- |
| Workbench and saved requests | Native top bar, rail, sidebar, tab strip, request/response shell, workspace selection, nested collection tree, request tabs, collection environment binding, and Send with the saved method, body, and static inherited auth | Load legacy and current fixtures. Switch workspaces, open a nested request, bind an environment, and verify variable precedence, path encoding, headers, and query on the local echo server. Include raw, URL-encoded, and multipart bodies, and basic, bearer, and API-key authentication. Confirm layout against the reference at 1080×600, 1240×820, and a wide window. |
| Editing and session | Method and request editors, auth configuration, variable editors, workspace/collection/environment CRUD, reordering, history, examples, and restart persistence | Re-run request, drag and drop, history, examples, settings, and restart scenarios. Confirm saved requests and environments remain readable in Tauri. |
| Interoperability and inspection | cURL, code snippets, Postman/Insomnia/Hoppscotch import and export, backups, response views, search, copy/save, HTML and PDF previews | Compare exports and response bytes, exercise import/merge and preview fixtures, and check remaining drawers and dialogs against screenshots. |
| Security, automation, and release | Encrypted cookies, proxy, OAuth including PKCE, scripts, collection runner, settings, complete appearance coverage, installers | Run the security and automation scenarios. Test Windows NSIS and portable builds and Linux deb, AppImage, and RPM packages against the current Ubuntu 22.04 build baseline. Compare all reference states. Re-run startup and process-tree memory on equivalent profiles, including a populated workspace, and record any regressions before cutover. |

The initial GET-only timings are a baseline for the experiment. The release comparison uses equivalent workflows and includes time to first visible content, idle process-tree memory, and behavior on a large response.
