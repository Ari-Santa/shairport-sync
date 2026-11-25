#!/bin/sh

# install.sh - Builds the AirPlay Receiver GUI and installs it for the current user
#
# Usage: sh gui/install.sh [--uninstall]
#
# Building needs cargo (Rust 1.85 or newer), a C++ compiler, lld and the Qt 6
# development files; cargo downloads the app's Rust crates from crates.io.
#
# The app needs shairport-sync built with --with-dbus-interface and
# --with-mpris-interface, running as the systemd user unit
# shairport-sync.service with its settings in
# ~/.config/shairport-sync/shairport-sync.conf, and NQPTP as the user unit
# nqptp.service: see "Install from source" in README.md.

set -e

SRC_DIR=$(cd "$(dirname "$0")" && pwd)
DATA_HOME=${XDG_DATA_HOME:-$HOME/.local/share}
BIN_DIR="$HOME/.local/bin"
BIN="$BIN_DIR/shairport-sync-gui"
DESKTOP_DIR="$DATA_HOME/applications"
ICON_DIR="$DATA_HOME/icons/hicolor/scalable/apps"
# Where this script put the app up to 5.6.2-3, when it was a Python script,
# with $BIN a link to it.
OLD_APP_DIR="$DATA_HOME/shairport-sync-gui"

if command -v dnf >/dev/null 2>&1; then
    BUILD_ADVICE="sudo dnf install cargo gcc-c++ lld qt6-qtbase-devel qt6-qtdeclarative-devel"
    ADVICE="sudo dnf install kf6-kirigami kf6-qqc2-desktop-style qt6-qtdeclarative qt6-qtsvg"
else
    BUILD_ADVICE="sudo apt install cargo g++ lld qt6-base-dev qt6-declarative-dev"
    ADVICE="sudo apt install qml6-module-qtquick qml6-module-qtquick-controls qml6-module-qtquick-layouts qml6-module-qtquick-effects qml6-module-org-kde-kirigami qml6-module-org-kde-desktop qt6-svg-plugins"
fi

# Qt 6's qmake, under the names the build looks for it.
find_qmake() {
    for qmake in ${QMAKE:+"$QMAKE"} qmake6 qmake-qt6 qmake; do
        command -v "$qmake" >/dev/null 2>&1 || continue
        case "$("$qmake" -query QT_VERSION 2>/dev/null)" in
            6.*) echo "$qmake"; return 0 ;;
        esac
    done
    return 1
}

# Whether a Qt Quick module, given as its directory (org/kde/kirigami), is
# installed where the app looks for it.
have_qml_module() {
    saved_ifs=$IFS
    IFS=:
    for dir in $QML_DIRS; do
        if [ -n "$dir" ] && [ -f "$dir/$1/qmldir" ]; then
            IFS=$saved_ifs
            return 0
        fi
    done
    IFS=$saved_ifs
    return 1
}

remove_python_install() {
    if [ "$(readlink "$BIN" 2>/dev/null)" = "$OLD_APP_DIR/shairport-sync-gui" ]; then
        rm -f "$BIN"
    fi
    [ ! -d "$OLD_APP_DIR" ] || rm -r -- "$OLD_APP_DIR"
}

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
    # Only what this script installed, now or as the Python app: a launcher
    # entry or an icon of the same name that something else put there stays,
    # and so does a link of the program's name that points somewhere else.
    if grep -q -x -F -e "Exec=$BIN" -e "Exec=$OLD_APP_DIR/shairport-sync-gui" \
            "$DESKTOP_DIR/shairport-sync.desktop" 2>/dev/null; then
        rm -f "$DESKTOP_DIR/shairport-sync.desktop"
    fi
    if cmp -s "$SRC_DIR/shairport-sync.svg" "$ICON_DIR/shairport-sync.svg" ||
            cmp -s "$OLD_APP_DIR/shairport-sync.svg" "$ICON_DIR/shairport-sync.svg"; then
        rm -f "$ICON_DIR/shairport-sync.svg"
    fi
    remove_python_install
    if [ -f "$BIN" ] && [ ! -L "$BIN" ]; then
        rm -f "$BIN"
    fi
    refresh_launcher
    echo "[OK] Removed \"AirPlay Receiver\" from $BIN_DIR and the application launcher."
    exit 0
fi

# What the build needs. cargo names a Rust that is too old itself.
MISSING=
command -v cargo >/dev/null 2>&1 || MISSING="$MISSING, cargo"
command -v c++ >/dev/null 2>&1 || command -v g++ >/dev/null 2>&1 ||
    command -v clang++ >/dev/null 2>&1 || MISSING="$MISSING, a C++ compiler"
# With GNU ld the app doesn't link; the build uses lld when it finds it.
command -v ld.lld >/dev/null 2>&1 || MISSING="$MISSING, lld"
if QMAKE_BIN=$(find_qmake); then
    QT_HEADERS=$("$QMAKE_BIN" -query QT_INSTALL_HEADERS 2>/dev/null)
    [ -d "$QT_HEADERS/QtWidgets" ] || MISSING="$MISSING, the Qt 6 base development files"
    [ -d "$QT_HEADERS/QtQuick" ] || MISSING="$MISSING, the Qt 6 declarative (Qt Quick) development files"
else
    MISSING="$MISSING, the Qt 6 development files (qmake6)"
fi
if [ -n "$MISSING" ]; then
    echo "[FAIL] Missing for the build: ${MISSING#, }." >&2
    echo "  [ADVICE] $BUILD_ADVICE" >&2
    exit 1
fi

# The imports of Main.qml, which is compiled into the program: the window
# doesn't open without them, and the program then only says so in a dialog.
QML_DIRS="$("$QMAKE_BIN" -query QT_INSTALL_QML 2>/dev/null)${QML_IMPORT_PATH:+:$QML_IMPORT_PATH}"
MISSING=
for module in QtQuick QtQuick/Controls QtQuick/Effects QtQuick/Layouts org/kde/kirigami; do
    have_qml_module "$module" || MISSING="$MISSING, $(echo "$module" | tr / .)"
done
if [ -n "$MISSING" ]; then
    echo "[FAIL] Qt Quick modules the app needs are missing: ${MISSING#, }." >&2
    echo "  [ADVICE] $ADVICE" >&2
    exit 1
fi
if ! have_qml_module org/kde/desktop; then
    echo "[NOTE] The Plasma Qt Quick style (org.kde.desktop) is missing, so the app uses the default Qt style."
fi
if [ ! -e "$("$QMAKE_BIN" -query QT_INSTALL_PLUGINS 2>/dev/null)/imageformats/libqsvg.so" ]; then
    echo "[NOTE] Qt's SVG plugin is missing, so the app's icons may not show."
    echo "  [ADVICE] $ADVICE"
fi

echo "[..] Building the app with cargo. The first build downloads its Rust crates and takes a few minutes..."
if ! cargo build --release --locked --manifest-path "$SRC_DIR/Cargo.toml"; then
    echo "[FAIL] The build failed." >&2
    echo "  [ADVICE] $BUILD_ADVICE" >&2
    exit 1
fi
BUILT="${CARGO_TARGET_DIR:-$SRC_DIR/target}/release/shairport-sync-gui"

remove_python_install
install -d "$BIN_DIR" "$DESKTOP_DIR" "$ICON_DIR"
# Not through a link that something else left there.
rm -f "$BIN"
install -m 0755 "$BUILT" "$BIN"
install -m 0644 "$SRC_DIR/shairport-sync.svg" "$ICON_DIR/shairport-sync.svg"
sed "s|^Exec=.*|Exec=$BIN|" "$SRC_DIR/shairport-sync.desktop" \
    > "$DESKTOP_DIR/shairport-sync.desktop"
echo "[OK] Installed $BIN"

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
