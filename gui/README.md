# AirPlay Receiver (GUI)

A small Kirigami/PyQt6 window that shows what Shairport Sync is playing (cover art,
title, artist, album, progress, the sending device and audio formats) and lets you
play/pause, skip, change volume, disconnect the sender, and start or stop the receiver.

With the PipeWire backend, the **Output** picker at the bottom chooses where AirPlay
audio plays: **Follow System** (the default output, moving with it when you change it
in Plasma) or a specific device. The choice is saved as `pipewire.sink_target` in your
settings. A stream that is playing moves immediately; the receiver itself restarts to
pick up the new setting only once nothing is connected, so senders aren't dropped.

**Use Device Volume**, in the menu, decides what the sender's volume control does.
Off (the default), Shairport Sync turns the audio down digitally before PipeWire
gets it, which costs resolution on a 16-bit device: about 5 bits at half volume.
On, the sender's volume control sets the output device's own volume instead (its
hardware mixer, where it has one), so the audio stays at full resolution at any
volume. Other apps playing on that device get louder or quieter with it, and the
first session after you turn it on sets the device to whatever the sender's volume
is, so turn the sender down first. The choice is saved as `pipewire.mixer_type =
"sink"` and, like an output change, takes effect once nothing is connected.

It talks to shairport-sync over D-Bus and controls the receiver through
`systemd --user`, so it is meant for a per-user install with
`shairport-sync.service` and `nqptp.service` as user units.

## Install the packages

The `shairport-sync-desktop` package from this fork's
[releases](https://github.com/Ari-Santa/shairport-sync/releases) installs the
whole receiver: Shairport Sync built for AirPlay 2, NQPTP, this app, and the
systemd user services.

Debian 13 (amd64):

```
sudo apt install ./shairport-sync-desktop_5.6.2-1_amd64.deb
```

Fedora 45 (x86_64), no extra repositories needed:

```
sudo dnf install ./shairport-sync-desktop-5.6.2-1.fc45.x86_64.rpm
```

The receiver starts right away in your session and at every login. Your
settings are in `~/.config/shairport-sync/shairport-sync.conf` (copied from
`/etc/shairport-sync.conf` on first start); open them from the app's menu with
"Edit Settings…". The package replaces the distribution's own `shairport-sync`.

To build the packages yourself, from a clone with submodules
(`git clone --recurse-submodules …`):

```
sudo apt build-dep ./              # Debian: once
dpkg-buildpackage -b -us -uc
rpm/build-rpm.sh 45                # Fedora 45, in a podman container
```

Both put the packages in the directory above the source tree. The Debian and
RPM packaging share the user services and default settings in `packaging/`.

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
