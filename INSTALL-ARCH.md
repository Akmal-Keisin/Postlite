# Installing Postlite on Arch Linux

Postlite is a Tauri v2 desktop app: a Rust backend plus a static HTML/CSS/JS frontend that is embedded into the binary at build time. On Arch there is no official package, so you have two options:

- **[A. Run the prebuilt AppImage](#a-run-the-prebuilt-appimage)** — fastest, nothing to compile, no system integration.
- **[B. Build from source](#b-build-from-source)** — recommended, gives you a native binary you can install system-wide.

Both need a graphical session with GTK 3 / WebKitGTK 4.1 available at runtime.

> Always do a full upgrade before installing build dependencies on Arch. Partial upgrades (`pacman -Sy` without `-u`) are unsupported and are the most common cause of "library not found" errors:
>
> ```sh
> sudo pacman -Syu
> ```

---

## A. Run the prebuilt AppImage

Releases are published on the [GitHub releases page](https://github.com/WildanR19/postlite/releases) (built on Ubuntu 22.04, unsigned). The Linux asset is an AppImage whose name includes the version, e.g. `postlite_0.2.5_amd64.AppImage`.

```sh
# 1. Download the AppImage for the latest release, using the exact
#    asset name shown on the releases page (or with the gh CLI):
gh release download --repo WildanR19/postlite --pattern '*amd64.AppImage'

# 2. Make it executable and run it
chmod +x postlite_*_amd64.AppImage
./postlite_*_amd64.AppImage
```

AppImages need FUSE 2 to mount themselves. If you get a `fuse: failed to open /dev/fuse` or "dlopen(): error loading libfuse.so.2" error:

```sh
# Either install FUSE 2 (AUR) ...
paru -S fuse2

# ... or skip FUSE entirely and extract-and-run
./postlite_*.AppImage --appimage-extract-and-run
```

An AppImage is not integrated into your application menu and does not update itself. If you want a normal `postlite` command and a launcher entry, use option B or follow [Install the binary and a desktop entry](#install-the-binary-and-a-desktop-entry) after extracting the AppImage.

---

## B. Build from source

### 1. Build dependencies

The only dependencies Postlite needs are the standard Tauri v2 Linux stack, plus OpenSSL for `reqwest`'s `native-tls` backend and `patchelf` if you also want to produce an AppImage.

```sh
sudo pacman -S --needed \
  base-devel \
  git \
  rust \
  webkit2gtk-4.1 \
  gtk3 \
  libsoup3 \
  openssl \
  librsvg \
  patchelf \
  curl wget file
```

Notes on the packages:

| Package | Why |
| --- | --- |
| `rust` | Stable toolchain (`cargo`, `rustc`). `rustup` works too — `rustup default stable`. |
| `base-devel` | `cc`, `make`, `pkg-config` and friends for crates with native build steps. |
| `webkit2gtk-4.1` | The webview Tauri v2 uses. **4.0 will not work** — this crate needs 4.1. |
| `gtk3` | Window/widget toolkit, and what `rfd` uses for the native Save-As dialog. |
| `libsoup3` | Pulled in by `webkit2gtk-4.1`; listed explicitly so it is never missing. |
| `openssl` | `native-tls` (HTTPS) at build and link time. |
| `librsvg` | Renders the app icons when building bundles. |
| `patchelf` | Only needed for `cargo tauri build` AppImage bundling. |

Optional:

- `libayatana-appindicator` — only if you later enable a system-tray feature; not used by the current config.
- `nodejs` + `npm` — only for the verification harnesses in `.verify/` (`node .verify/server.mjs`, etc.). The app itself has **no** npm dependencies and no bundler.

### 2. Get the source and build

```sh
git clone https://github.com/WildanR19/postlite.git
cd postlite/src-tauri

# Production build. The static UI in ../ui is embedded by build.rs,
# so any later edit to ui/ requires re-running this.
cargo build --release
```

The result is `src-tauri/target/release/postlite` — a self-contained binary.

Cargo downloads and compiles the whole tree on the first build (~5–15 min depending on your machine); later builds are incremental.

**Development loop (optional).** Hot reload needs the Tauri CLI:

```sh
cargo install tauri-cli   # once
cd src-tauri
cargo tauri dev
```

### 3. Install the binary and a desktop entry

Installing to `/usr/local` keeps the app outside pacman's database but available to every user. Use `~/.local` variants if you prefer a per-user install (no `sudo`).

```sh
# Binary
sudo install -Dm755 src-tauri/target/release/postlite /usr/local/bin/postlite

# Icon (256x256 PNG is used for the hicolor theme)
sudo install -Dm644 src-tauri/icons/icon.png \
  /usr/share/icons/hicolor/256x256/apps/postlite.png

# Desktop entry
sudo tee /usr/share/applications/postlite.desktop >/dev/null <<'EOF'
[Desktop Entry]
Type=Application
Name=Postlite
Comment=Lightweight desktop HTTP client
Exec=postlite
Icon=postlite
Terminal=false
Categories=Development;Network;
Keywords=http;rest;api;postman;curl;
StartupWMClass=postlite
EOF

# Refresh the menu/icon caches
sudo gtk-update-icon-cache -q /usr/share/icons/hicolor || true
sudo update-desktop-database -q /usr/share/applications || true
```

Then launch it from your app launcher, or:

```sh
postlite
```

Per-user install instead:

```sh
install -Dm755 src-tauri/target/release/postlite ~/.local/bin/postlite
install -Dm644 src-tauri/icons/icon.png ~/.local/share/icons/hicolor/256x256/apps/postlite.png
install -d ~/.local/share/applications
tee ~/.local/share/applications/postlite.desktop >/dev/null <<'EOF'
[Desktop Entry]
Type=Application
Name=Postlite
Comment=Lightweight desktop HTTP client
Exec=postlite
Icon=postlite
Terminal=false
Categories=Development;Network;
EOF
```

Make sure `~/.local/bin` is in your `PATH` (it is by default in Arch's `/etc/profile`).

### 4. Bundling packages instead (optional)

`src-tauri/tauri.conf.json` configures `nsis`, `deb`, `appimage` and `rpm` targets. On Arch you can build the Linux ones locally (this needs the Tauri CLI: `cargo install tauri-cli`):

```sh
cd src-tauri
cargo tauri build --bundles appimage   # portable AppImage in target/release/bundle/appimage/
```

- The AppImage bundler downloads `linuxdeploy` on first run and needs `patchelf` plus FUSE 2 (or `APPIMAGE_EXTRACT_AND_RUN=1`). Arch does not ship `fuse2` in the official repos; `paru -S fuse2` or the extract-and-run variable avoids it.
- `.deb` and `.rpm` bundles build too, but they are for Debian/Fedora. On Arch, prefer the plain binary above, or convert with `debtap` if you really want a pacman-tracked package (`debtap` is in the AUR).
- Packages produced this way are unsigned, same as the released artifacts.

---

## Runtime: the secret store (cookies & proxy credentials)

Persistent cookies and manual proxy credentials are encrypted with a per-workspace AES-256-GCM key that lives in the **OS secret store**. On Linux that means a **Secret Service** provider over D-Bus. Without one, Postlite still works for the session but writes nothing to disk for those features and says so in the Cookies drawer.

Install/enable one of:

```sh
# GNOME / most GTK sessions — usually already installed and started
sudo pacman -S --needed gnome-keyring

# KDE Plasma: KWallet, with its Secret Service integration enabled
# (kwallet is part of plasma; no extra package needed)

# KeePassXC also provides Secret Service — enable it in
# Tools > Settings > Browser Integration > Secret Service
```

Checks:

```sh
# Is a Secret Service answering on the session bus?
busctl --user list | grep -i org.freedesktop.secrets

# No D-Bus session at all (e.g. a bare window manager started without one)?
dbus-run-session postlite
```

Unlock your keyring at login (GNOME and Plasma do this automatically). If it is locked, Postlite treats the store as unavailable rather than leaking plaintext.

---

## Where your data lives

Everything lives under `~/.local/share/app.postlite.desktop/`:

```
~/.local/share/app.postlite.desktop/
  workspace.json          # workspaces, collections, environments, tabs, settings (plain JSON)
  cookies/<workspaceId>.bin   # AES-256-GCM encrypted cookie jars
  secrets/global.bin          # AES-256-GCM encrypted proxy credentials
```

- `workspace.json` is human-readable and safe to back up by hand, or use **Settings → Import / Export** to write a single `postlite.backup` file.
- Cookie jars and proxy credentials are deliberately excluded from backups.
- To reset the app completely: quit it and `rm -rf ~/.local/share/app.postlite.desktop`, then remove the `app.postlite.desktop.postlite.store` entry from your keyring (`seahorse`, `kwalletmanager`, or `secret-tool`).

Environment variables worth knowing:

```sh
# Proxy mode "environment variables" reads these
HTTP_PROXY / HTTPS_PROXY / ALL_PROXY / NO_PROXY

# Debug builds only: simulate a machine with no secret store
POSTLITE_FORCE_NO_SECRET_STORE=1
```

---

## Troubleshooting

**`error: failed to run custom build command for 'webkit2gtk-sys'` / `Package webkit2gtk-4.0 was not found`**
You are missing the 4.1 development package. Install `webkit2gtk-4.1` (not `webkit2gtk` from older guides); Tauri v2 does not support 4.0.

**Linker errors mentioning `ssl`, `crypto` or `-lssl`**
Install `openssl` and make sure `pkg-config --modversion openssl` prints a version. `reqwest` is built with `native-tls`, so OpenSSL is linked, not vendored.

**Blank/white window, or the window flickers, on NVIDIA or Wayland**
WebKitGTK's DMA-BUF renderer is the usual culprit. Launch with one of:

```sh
WEBKIT_DISABLE_DMABUF_RENDERER=1 postlite
WEBKIT_DISABLE_COMPOSITING_MODE=1 postlite
```

To make it permanent, drop those into the desktop entry's `Exec=` line, or set them in your session environment.

**"cookies are not saved: no secret store found on this system"**
No Secret Service is reachable — see [Runtime: the secret store](#runtime-the-secret-store-cookies--proxy-credentials). Session cookies still work; nothing persistent is written.

**AppImage will not start: `libfuse.so.2` / `/dev/fuse`**
Install `fuse2` from the AUR, or run it with `--appimage-extract-and-run`.

**`Failed to initialize gtk backend!` / `Failed to initialize GTK`**
The app was started without a graphical session — over SSH, from a TTY, or under a cron/systemd unit. Launch it from inside your desktop session. Note that `sudo postlite` also loses your session's `DISPLAY`/`WAYLAND_DISPLAY` and D-Bus address.

**The window is too small / clipped**
The app enforces a minimum window size of 1080×600. On smaller displays, scale the UI (`GDK_SCALE=1`, or a lower display scale) or run at a higher resolution.

**My UI edits do not show up**
`ui/` is embedded at compile time by `src-tauri/build.rs`. Rebuild (`cargo build --release`, or `cargo tauri dev` for hot reload) — editing files in a built app does nothing.

**Build fails after a system upgrade**
Run `sudo pacman -Syu` and rebuild from a clean state if needed (`cargo clean` in `src-tauri`). Rebuilds after a `webkit2gtk-4.1` soname bump require a fresh link.

---

## Uninstall

```sh
# System-wide install
sudo rm /usr/local/bin/postlite \
        /usr/share/applications/postlite.desktop \
        /usr/share/icons/hicolor/256x256/apps/postlite.png
sudo gtk-update-icon-cache -q /usr/share/icons/hicolor || true
sudo update-desktop-database -q /usr/share/applications || true

# Your data (irreversible)
rm -rf ~/.local/share/app.postlite.desktop
```

Then delete the `app.postlite.desktop.postlite.store` entry from your keyring.
