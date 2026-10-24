#!/bin/sh

# install.sh - Installs the AirPlay Receiver GUI for the current user
#
# The app needs shairport-sync built with --with-dbus-interface and
# --with-mpris-interface, with both interfaces on the session bus:
#   general = { dbus_service_bus = "session"; mpris_service_bus = "session"; };

set -e

SRC_DIR=$(cd "$(dirname "$0")" && pwd)
DATA_HOME=${XDG_DATA_HOME:-$HOME/.local/share}
APP_DIR="$DATA_HOME/shairport-sync-gui"
BIN_DIR="$HOME/.local/bin"
DESKTOP_DIR="$DATA_HOME/applications"
ICON_DIR="$DATA_HOME/icons/hicolor/scalable/apps"

if [ "$(id -u)" -eq 0 ]; then
    echo "[FAIL] Run this as the user who will use the app, not as root." >&2
    exit 1
fi

if ! python3 -c "import PyQt6.QtQml, PyQt6.QtDBus" 2>/dev/null; then
    echo "[FAIL] PyQt6 with QML support is missing." >&2
    echo "  [ADVICE] sudo apt install python3-pyqt6 python3-pyqt6.qtquick" >&2
    exit 1
fi

install -d "$APP_DIR/qml" "$BIN_DIR" "$DESKTOP_DIR" "$ICON_DIR"
install -m 0755 "$SRC_DIR/shairport-sync-gui" "$APP_DIR/shairport-sync-gui"
install -m 0644 "$SRC_DIR/qml/Main.qml" "$APP_DIR/qml/Main.qml"
install -m 0644 "$SRC_DIR/shairport-sync.svg" "$APP_DIR/shairport-sync.svg"
install -m 0644 "$SRC_DIR/shairport-sync.svg" "$ICON_DIR/shairport-sync.svg"
ln -sf "$APP_DIR/shairport-sync-gui" "$BIN_DIR/shairport-sync-gui"
sed "s|^Exec=.*|Exec=$APP_DIR/shairport-sync-gui|" "$SRC_DIR/shairport-sync.desktop" \
    > "$DESKTOP_DIR/shairport-sync.desktop"
echo "[OK] Installed to $APP_DIR"

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DESKTOP_DIR" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$DATA_HOME/icons/hicolor" || true
command -v kbuildsycoca6 >/dev/null 2>&1 && kbuildsycoca6 >/dev/null 2>&1 || true
# Running KDE apps (e.g. the Plasma launcher) only scan icon folders that existed
# when they started; this is the signal System Settings sends to make them rescan.
command -v dbus-send >/dev/null 2>&1 && \
    dbus-send --session --type=signal /KIconLoader org.kde.KIconLoader.iconChanged int32:0 2>/dev/null || true

echo "[OK] \"AirPlay Receiver\" is now in your application launcher."
