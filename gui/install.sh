#!/bin/sh

# install.sh - Installs the AirPlay Receiver GUI for the current user
#
# Usage: sh gui/install.sh [--uninstall]
#
# The app needs shairport-sync built with --with-dbus-interface and
# --with-mpris-interface, running as the systemd user unit
# shairport-sync.service with its settings in
# ~/.config/shairport-sync/shairport-sync.conf, and NQPTP as the user unit
# nqptp.service: see "Install from source" in README.md.

set -e

SRC_DIR=$(cd "$(dirname "$0")" && pwd)
DATA_HOME=${XDG_DATA_HOME:-$HOME/.local/share}
APP_DIR="$DATA_HOME/shairport-sync-gui"
BIN_DIR="$HOME/.local/bin"
DESKTOP_DIR="$DATA_HOME/applications"
ICON_DIR="$DATA_HOME/icons/hicolor/scalable/apps"

if command -v dnf >/dev/null 2>&1; then
    ADVICE="sudo dnf install python3-pyqt6 kf6-kirigami kf6-qqc2-desktop-style qt6-qtdeclarative qt6-qtsvg"
else
    ADVICE="sudo apt install python3-pyqt6 python3-pyqt6.qtquick qml6-module-org-kde-kirigami qml6-module-qtquick-effects qml6-module-org-kde-desktop qt6-svg-plugins"
fi

# Loads the imports of Main.qml the way the app does: the window doesn't open
# without them. (A bare QQmlEngine hangs on Kirigami 6.29's import.) Plasma's
# Qt Quick style is used where it is installed.
QML_CHECK='
import sys
from pathlib import Path
from PyQt6.QtCore import QUrl, qInstallMessageHandler
from PyQt6.QtQml import QQmlApplicationEngine
from PyQt6.QtWidgets import QApplication

qInstallMessageHandler(lambda *message: None)  # the errors are printed below
app = QApplication(sys.argv[:1])
engine = QQmlApplicationEngine()
engine.setOutputWarningsToStandardError(False)
errors = []
engine.warnings.connect(lambda warnings: errors.extend(w.description() for w in warnings))
main_qml = Path(sys.argv[1])
imports = [line for line in main_qml.read_text().splitlines(True) if line.startswith("import ")]
engine.loadData("".join(imports + ["QtObject {}\n"]).encode(), QUrl.fromLocalFile(str(main_qml)))
if not engine.rootObjects():
    print("[FAIL] Qt Quick modules the app needs are missing:", file=sys.stderr)
    for error in errors:
        print("  " + error, file=sys.stderr)
    sys.exit(1)
if not any(Path(path, "org/kde/desktop/qmldir").is_file() for path in engine.importPathList()):
    print("[NOTE] The Plasma Qt Quick style (org.kde.desktop) is missing, so the app uses the default Qt style.")
'

refresh_launcher() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DESKTOP_DIR" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$DATA_HOME/icons/hicolor" || true
    command -v kbuildsycoca6 >/dev/null 2>&1 && kbuildsycoca6 >/dev/null 2>&1 || true
    # Running KDE apps (e.g. the Plasma launcher) only scan icon folders that existed
    # when they started; this is the signal System Settings sends to make them rescan.
    command -v dbus-send >/dev/null 2>&1 && \
        dbus-send --session --type=signal /KIconLoader org.kde.KIconLoader.iconChanged int32:0 2>/dev/null || true
}

case "$1" in
    "" | --uninstall) ;;
    *) echo "usage: $0 [--uninstall]" >&2; exit 1 ;;
esac

if [ "$(id -u)" -eq 0 ]; then
    echo "[FAIL] Run this as the user who will use the app, not as root." >&2
    exit 1
fi

if [ "$1" = "--uninstall" ]; then
    # Only what this script installed: a launcher entry or an icon of the same
    # name that something else put there stays.
    if [ "$(readlink "$BIN_DIR/shairport-sync-gui" 2>/dev/null)" = "$APP_DIR/shairport-sync-gui" ]; then
        rm -f "$BIN_DIR/shairport-sync-gui"
    fi
    if grep -q -x -F "Exec=$APP_DIR/shairport-sync-gui" "$DESKTOP_DIR/shairport-sync.desktop" 2>/dev/null; then
        rm -f "$DESKTOP_DIR/shairport-sync.desktop"
    fi
    if cmp -s "$APP_DIR/shairport-sync.svg" "$ICON_DIR/shairport-sync.svg"; then
        rm -f "$ICON_DIR/shairport-sync.svg"
    fi
    [ ! -d "$APP_DIR" ] || rm -r -- "$APP_DIR"
    refresh_launcher
    echo "[OK] Removed \"AirPlay Receiver\" from $APP_DIR and the application launcher."
    exit 0
fi

if ! python3 -c "import PyQt6.QtDBus, PyQt6.QtQml, PyQt6.QtWidgets" 2>/dev/null; then
    echo "[FAIL] PyQt6 with QML support is missing." >&2
    echo "  [ADVICE] $ADVICE" >&2
    exit 1
fi
if ! QT_QPA_PLATFORM=offscreen python3 -c "$QML_CHECK" "$SRC_DIR/qml/Main.qml"; then
    echo "  [ADVICE] $ADVICE" >&2
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

refresh_launcher

# The app changes the receiver's settings in this file and controls it through
# these units; without them its changes never reach the receiver.
if ! systemctl --user show -p ExecStart --value shairport-sync.service 2>/dev/null |
        grep -q -F -- "-c $HOME/.config/shairport-sync/shairport-sync.conf" ||
        [ "$(systemctl --user show -p LoadState --value nqptp.service 2>/dev/null)" != loaded ]; then
    echo "[NOTE] The app controls the receiver through the user units shairport-sync.service and nqptp.service, and changes its settings in ~/.config/shairport-sync/shairport-sync.conf, but the receiver isn't set up that way here."
    echo "  [ADVICE] See \"Install from source\" in $SRC_DIR/README.md."
fi
# The same names as the package's, which these per-user copies hide.
if [ -e /usr/share/applications/shairport-sync.desktop ]; then
    echo "[NOTE] The shairport-sync-desktop package is installed too. This copy is started instead of the package's until you remove it with \"sh $SRC_DIR/install.sh --uninstall\"."
fi

echo "[OK] \"AirPlay Receiver\" is now in your application launcher."
