# Postlite GPUI desktop port

The GPUI workbench opens legacy and current Postlite workspaces. It supports nested collections, request and environment tabs, environment binding, collection and folder variables, request editing, history, saved examples, Postman import and export, cURL import and copy, and workspace backup and restore. Both desktop apps send through the same Rust HTTP engine and share the cookie jar, proxy credential store, and OAuth token exchange. The four appearance presets use the original palettes, and the native workbench supports dark, light, and system themes, the eight accents, and stacked and side-by-side panels.

## Install

The GPUI app is built from source on every platform; there is no prebuilt download or packaged installer yet. Each setup takes the same three steps: install the build tools, install Rust from [rustup.rs](https://rustup.rs), then build from the repository root.

```sh
cargo build --release --locked --manifest-path src-gpui/Cargo.toml
```

The first build downloads and compiles every dependency and takes several minutes. Later builds are incremental.

### Linux

Install the build tools, the libraries GPUI links against, and a Vulkan driver for your GPU. On Arch Linux:

```sh
sudo pacman -S --needed base-devel rust openssl libxkbcommon-x11 wayland fontconfig vulkan-icd-loader
# plus the Vulkan driver for your GPU: vulkan-radeon, vulkan-intel, or nvidia-utils
```

On Debian or Ubuntu, the equivalent packages are `build-essential pkg-config libssl-dev libxkbcommon-x11-dev libx11-xcb-dev libwayland-dev libfontconfig-dev libvulkan1 mesa-vulkan-drivers`. Install Rust with rustup if your distribution's `cargo` is older than the current stable release.

Then build and install for your user in one step:

```sh
sh scripts/install-gpui.sh
```

The script installs the binary to `~/.local/bin/postlite-gpui` and adds a **Postlite GPUI** launcher with its icon to your application menu. Run the same command again after pulling changes to update. Other options:

```sh
sh scripts/install-gpui.sh --no-build             # install the existing release build
sh scripts/install-gpui.sh --uninstall            # remove the binary and launcher
sudo PREFIX=/usr/local sh scripts/install-gpui.sh # install for every user
```

Uninstalling keeps your workspace data.

### macOS

Install Apple's command line tools, which include the compiler and the Metal toolchain GPUI renders with:

```sh
xcode-select --install
```

Build with the command above, then start the app from Terminal:

```sh
./src-gpui/target/release/postlite-gpui
```

The build is a plain executable, not an `.app` bundle, so it does not appear in Launchpad or the Dock until it runs. To keep it on your `PATH`:

```sh
mkdir -p ~/.local/bin
cp src-gpui/target/release/postlite-gpui ~/.local/bin/
```

### Windows

Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the **Desktop development with C++** workload, which includes the MSVC compiler and the Windows SDK. Then build in PowerShell from the repository root:

```powershell
cargo build --release --locked --manifest-path src-gpui/Cargo.toml
.\src-gpui\target\release\postlite-gpui.exe
```

Release builds open without a console window. To add a Start menu entry, copy the executable somewhere permanent and create a shortcut to it:

```powershell
$dir = "$env:LOCALAPPDATA\Programs\Postlite GPUI"
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item src-gpui\target\release\postlite-gpui.exe $dir
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut(
  "$([Environment]::GetFolderPath('Programs'))\Postlite GPUI.lnk")
$shortcut.TargetPath = "$dir\postlite-gpui.exe"
$shortcut.Save()
```

Repeat the build and `Copy-Item` steps to update. The executable has no embedded icon yet, so the shortcut and taskbar show the default Windows icon.

## Run from the repository

To try a build without installing it:

```sh
./src-gpui/target/release/postlite-gpui
```

The GPUI app looks for its own workspace in the OS app-data directory under `app.postlite.gpui`. On first launch it can load the original Tauri workspace. Edits save to the separate GPUI profile. To open a specific fixture or use a writable copy at a chosen location, pass one of these flags.

```sh
./src-gpui/target/release/postlite-gpui --workspace /path/to/workspace.json
./src-gpui/target/release/postlite-gpui --write-workspace /path/to/isolated-copy.json
```

`--workspace` keeps edits in memory. The save code rejects paths inside the live Tauri app-data directory. The lockfile pins compatible `libc` and `mio` versions because GPUI 0.2.2's archive dependency references the former Linux `libc::ENOATTR` constant.

## Cookies, proxies, and OAuth

Open **Cookies** in the request toolbar to inspect, copy, or delete cookies for the current workspace. **Application settings → Request** selects Direct, Manual, or Environment variables for proxies, and contains the cookie persistence and clear controls.

Select **OAuth 2.0** in a request's Auth tab, or configure it by right-clicking a collection or folder. An inheriting request uses the nearest parent with explicit auth. Choose **Get token** for client credentials or **Authorize** for authorization code with PKCE. You can paste a redirect URL or code if the browser callback cannot reach the loopback listener.

A writable GPUI workspace stores eligible cookies and credentials in an encrypted side directory with keys in the OS credential store. Each workspace copy has a separate namespace. Re-enter proxy passwords and OAuth client secrets when switching from Tauri. Read-only runs keep their session data in memory.

## Verification and performance

Use the Linux X11 smoke test with `xdotool` and `xclip` to open a nested collection request and send it to a local echo server. It checks variable resolution, request and response bodies, auth, history, environment tab selection, and workspace persistence.

```sh
cargo test --release --locked --manifest-path src-gpui/Cargo.toml
cargo test --release --locked --manifest-path src-tauri/Cargo.toml
cargo test --release --locked --manifest-path postlite-core/Cargo.toml
python3 .verify/gpui-workbench-smoke.py
python3 .verify/gpui-workbench-smoke.py --sessions
python3 .verify/gpui-workbench-smoke.py --preview
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

The response viewer renders up to 512 KiB of text. The toolbar can copy or save the full captured body, subject to the request's response-size limit. Preview displays formatted static HTML up to 256 KiB. Open Raw for complex documents or use Save to open a PDF in a PDF viewer.

This comparison covers startup with a saved collection, environment, and open request. Script execution, the collection runner, inline PDF previews, alternate import formats, code targets beyond cURL, installer verification, and visual matching for controls and secondary screens remain in the [parity design](../docs/superpowers/specs/2026-11-26-gpui-parity-design.md).
