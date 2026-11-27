#!/usr/bin/env sh
# Build and install the Postlite GPUI desktop app for the current user.
#
#   sh scripts/install-gpui.sh              build, then install to ~/.local
#   sh scripts/install-gpui.sh --no-build   install the existing release build
#   sh scripts/install-gpui.sh --uninstall  remove the installed files
#
# Set PREFIX to install somewhere else, e.g. PREFIX=/usr/local (needs sudo).
# Your workspace data lives in the app-data directory and is never touched.
set -eu

APP_ID="app.postlite.gpui"
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PREFIX=${PREFIX:-"$HOME/.local"}
BIN="$PREFIX/bin/postlite-gpui"
DESKTOP="$PREFIX/share/applications/$APP_ID.desktop"
ICON="$PREFIX/share/icons/hicolor/128x128/apps/$APP_ID.png"

refresh_menus() {
    command -v update-desktop-database >/dev/null 2>&1 &&
        update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 &&
        gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
}

case "${1:-}" in
--uninstall)
    rm -f "$BIN" "$DESKTOP" "$ICON"
    refresh_menus
    echo "Removed Postlite GPUI from $PREFIX. Workspace data was kept."
    exit 0
    ;;
--no-build | "") ;;
*)
    echo "Usage: $0 [--no-build | --uninstall]" >&2
    exit 2
    ;;
esac

if [ "${1:-}" != "--no-build" ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "cargo was not found. On Arch: sudo pacman -S --needed rust base-devel" >&2
        exit 1
    fi
    echo "Building the release binary (the first build takes a few minutes)..."
    cargo build --release --locked --manifest-path "$ROOT/src-gpui/Cargo.toml"
fi

BUILT="$ROOT/src-gpui/target/release/postlite-gpui"
if [ ! -x "$BUILT" ]; then
    echo "No release build at $BUILT. Run without --no-build." >&2
    exit 1
fi

install -Dm755 "$BUILT" "$BIN"
install -Dm644 "$ROOT/src-tauri/icons/128x128.png" "$ICON"
mkdir -p "$(dirname -- "$DESKTOP")"
cat >"$DESKTOP" <<DESKTOP_ENTRY
[Desktop Entry]
Type=Application
Name=Postlite GPUI
GenericName=API Client
Comment=Send and inspect HTTP API requests
Exec=$BIN
Icon=$APP_ID
Terminal=false
Categories=Development;Network;
Keywords=api;http;rest;request;postman;
StartupWMClass=$APP_ID
DESKTOP_ENTRY
refresh_menus

echo "Installed Postlite GPUI:"
echo "  binary   $BIN"
echo "  launcher $DESKTOP"
case ":$PATH:" in
*":$PREFIX/bin:"*) echo "Run it from your app menu or with: postlite-gpui" ;;
*) echo "Run it from your app menu, or add $PREFIX/bin to PATH to use: postlite-gpui" ;;
esac
