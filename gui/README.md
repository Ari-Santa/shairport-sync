# AirPlay Receiver (GUI)

A small Kirigami/PyQt6 window that shows what Shairport Sync is playing (cover art,
title, artist, album, progress, the sending device and audio formats) and lets you
play/pause, skip, change volume, disconnect the sender, and start or stop the receiver.

It talks to shairport-sync over D-Bus and controls the receiver through
`systemd --user`, so it is meant for a per-user install with
`shairport-sync.service` and `nqptp.service` as user units.

## Install the .deb (Debian 13, amd64)

The `shairport-sync-desktop` package from this fork's
[releases](https://github.com/Ari-Santa/shairport-sync/releases) installs the
whole receiver: Shairport Sync built for AirPlay 2, NQPTP, this app, and the
systemd user services.

```
sudo apt install ./shairport-sync-desktop_5.6.1-1_amd64.deb
```

The receiver starts right away in your session and at every login. Your
settings are in `~/.config/shairport-sync/shairport-sync.conf` (copied from
`/etc/shairport-sync.conf` on first start); open them from the app's menu with
"Edit Settings…". The package replaces Debian's own `shairport-sync`.

To build the package yourself, from a clone with submodules
(`git clone --recurse-submodules …`):

```
sudo apt build-dep ./
dpkg-buildpackage -b -us -uc
```

## Install from source

### Requirements

* shairport-sync configured with `--with-dbus-interface --with-mpris-interface`.
* Both interfaces on the session bus, plus cover art, in `shairport-sync.conf`:
  ```
  general = {
    dbus_service_bus = "session";
    mpris_service_bus = "session";
  };
  metadata = {
    enabled = "yes";
    include_cover_art = "yes";
  };
  ```
* PyQt6 with QML and Kirigami (Debian/Ubuntu):
  ```
  sudo apt install python3-pyqt6 python3-pyqt6.qtquick qml6-module-org-kde-kirigami
  ```

### Install

```
sh gui/install.sh
```

This copies the app to `~/.local/share/shairport-sync-gui`, links
`~/.local/bin/shairport-sync-gui`, and adds "AirPlay Receiver" to the application
launcher. The desktop file is named `shairport-sync.desktop` to match the
`DesktopEntry` that shairport-sync reports over MPRIS, so Plasma's media controls
show the same icon.

## Notes

* Seeking isn't offered: AirPlay senders don't accept it from the receiver.
* The window can be run straight from the source tree: `gui/shairport-sync-gui`.
