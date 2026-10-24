# AirPlay Receiver (GUI)

A small Kirigami/PyQt6 window that shows what Shairport Sync is playing (cover art,
title, artist, album, progress, the sending device and audio formats) and lets you
play/pause, skip, change volume, disconnect the sender, and start or stop the receiver.

It talks to shairport-sync over D-Bus and controls the receiver through
`systemd --user`, so it is meant for a per-user install with
`shairport-sync.service` and `nqptp.service` as user units.

## Requirements

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

## Install

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
