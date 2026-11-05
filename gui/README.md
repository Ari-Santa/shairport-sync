# AirPlay Receiver (GUI)

A small Kirigami/PyQt6 window that shows what Shairport Sync is playing (cover art,
title, artist, album, progress, the sending device and audio formats) and lets you
play/pause, skip, change volume, disconnect the sender, and start or stop the receiver.

With the PipeWire backend, the **Output** picker at the bottom chooses where AirPlay
audio plays: **Follow System** (the default output, moving with it when you change it
in Plasma) or a specific device. The choice is saved as `pipewire.sink_target` in your
settings. A stream that is playing moves immediately; the receiver itself restarts to
pick up the new setting only once nothing is connected (a paused sender still counts),
so senders aren't dropped. If you close the app before then, it does so once you open
it again.

**Use Device Volume**, in the menu, decides what the sender's volume control does.
Off (the default), Shairport Sync turns the audio down digitally before PipeWire
gets it, which costs resolution on a 16-bit device: about 5 bits at half volume.
On, the sender's volume control sets the output device's own volume instead (its
hardware mixer, where it has one), so the audio stays at full resolution at any
volume. Other apps playing on that device get louder or quieter with it, and the
first session after you turn it on sets the device to whatever the sender's volume
is, so turn the sender down first. The choice is saved as `pipewire.mixer_type =
"sink"` and, like an output change, takes effect once nothing is connected.

The receiver sets the volume of the device it started with until it restarts. So
during a session, AirPlay stays on that device when you pick another output or, with
Follow System, when the default output changes; the app says so and switches once the
session ends. If the device wasn't connected when the receiver started, the sender's
volume control turns the audio down digitally after all: the app shows a warning and
restarts the receiver once the device is connected and nothing is playing. It can
only do this while it is open.

Changes you make to these two settings in the file yourself ("Edit Settings…" in the
menu) are picked up and applied the same way. The app changes nothing else in the
file, and won't save into it while it has a syntax error: the error names the line.

It talks to shairport-sync over D-Bus and controls the receiver through
`systemd --user`, so it is meant for a per-user install with
`shairport-sync.service` and `nqptp.service` as user units.

## Install the packages

The `shairport-sync-desktop` package from this fork's
[releases](https://github.com/Ari-Santa/shairport-sync/releases) installs the
whole receiver: Shairport Sync built for AirPlay 2, NQPTP, this app, and the
systemd user services. A .deb installs only on the release it was built for.

Debian 13 (amd64):

```
sudo apt install ./shairport-sync-desktop_5.6.2-3_amd64.deb
```

Ubuntu 26.10 (amd64):

```
sudo apt install ./shairport-sync-desktop_5.6.2-3.ubuntu26.10_amd64.deb
```

Fedora 45 (x86_64), no extra repositories needed:

```
sudo dnf install ./shairport-sync-desktop-5.6.2-3.fc45.x86_64.rpm
```

On Fedora, also open the [firewall](#firewall) for it.

The receiver starts right away in your session and at every login. Your settings
are in `~/.config/shairport-sync/shairport-sync.conf` (copied from
`/etc/shairport-sync-desktop.conf` on first start); open them from the app's menu
with "Edit Settings…".

The package replaces the distribution's own `shairport-sync` but doesn't use its
`/etc/shairport-sync.conf`: if you had changed settings there, such as the
receiver's name, copy them into your settings (Fedora keeps an edited copy as
`/etc/shairport-sync.conf.rpmsave`). On Ubuntu, if you later purge the replaced
package (`sudo apt purge shairport-sync`), run
`sudo systemctl --global enable shairport-sync.service` afterwards: purging it also
turns off this package's receiver, which has the same name, for future logins.

Up to 5.6.2-2, the package installed its defaults as `/etc/shairport-sync.conf`.
Upgrading removes that file, or keeps it as `/etc/shairport-sync.conf.dpkg-bak`
(Fedora: `.rpmsave`) if you had edited it; your own settings aren't touched.

To build the packages yourself, from a clone with submodules
(`git clone --recurse-submodules …`), with podman:

```
debian/build-deb.sh                # Debian 13
debian/build-deb.sh ubuntu:26.10   # Ubuntu 26.10
rpm/build-rpm.sh 45                # Fedora 45
```

Each builds in a container of that release and puts the packages in the directory
above the source tree. A .deb for a release other than the one in
`debian/changelog` (Debian 13) gets a version suffix such as `~ubuntu26.10`; in
GitHub's release downloads the `~` becomes a `.`. The Debian and RPM packaging
share the user services, default settings and firewall service in `packaging/`.

## Install from source

### Requirements

* Shairport Sync and NQPTP built and installed as in [BUILD.md](../BUILD.md), with
  `--with-pipewire --with-dbus-interface --with-mpris-interface` added to Shairport
  Sync's `./configure`. Those options also need `libpipewire-0.3-dev` and
  `libglib2.0-dev` on Debian/Ubuntu, or `pipewire-devel` and `glib2-devel` on
  Fedora, which BUILD.md's lists leave out. Don't enable Shairport Sync or NQPTP as
  a system service (or disable them:
  `sudo systemctl disable --now nqptp shairport-sync`) or run
  `user-service-install.sh`: the app needs them as the user units below.
* These settings in `/etc/shairport-sync.conf`, which the stock file has commented
  out in their groups:
  ```
  general = {
    output_backend = "pipewire";
  };
  metadata = {
    enabled = "yes";
    include_cover_art = "yes";
  };
  ```
  The Output picker and Use Device Volume need `output_backend` set in the file;
  `-o pipewire` on the command line isn't enough.
* PyQt6 with QML and Kirigami, on Debian/Ubuntu:
  ```
  sudo apt install python3-pyqt6 python3-pyqt6.qtquick qml6-module-org-kde-kirigami \
      qml6-module-qtquick-effects qml6-module-org-kde-desktop qt6-svg-plugins
  ```
  or on Fedora:
  ```
  sudo dnf install python3-pyqt6 kf6-kirigami kf6-qqc2-desktop-style \
      qt6-qtdeclarative qt6-qtsvg
  ```
  Without Plasma's Qt Quick style (`qml6-module-org-kde-desktop`,
  `kf6-qqc2-desktop-style`) the window has Qt's default look.

### Install

From the top of the source tree, set up the packages' user units for programs in
`/usr/local/bin` and settings copied from `/etc/shairport-sync.conf`, then the app:

```
mkdir -p ~/.config/systemd/user
for unit in shairport-sync nqptp; do
    sed -e 's|^ExecStart=/usr/bin/|ExecStart=/usr/local/bin/|' \
        -e 's|/etc/shairport-sync-desktop.conf|/etc/shairport-sync.conf|' \
        packaging/$unit.service > ~/.config/systemd/user/$unit.service
done
sudo setcap cap_net_bind_service=+ep /usr/local/bin/nqptp
systemctl --user daemon-reload
systemctl --user enable --now shairport-sync.service
sh gui/install.sh
```

The receiver's first start copies `/etc/shairport-sync.conf` to
`~/.config/shairport-sync/shairport-sync.conf`, which is what it reads and the app
changes; if you already have that file, it needs the settings above. The unit puts
the D-Bus and MPRIS interfaces on the session bus, where the app looks for them,
unless the settings name another bus. `setcap` lets NQPTP use the PTP ports; repeat
it after each `make install` of NQPTP.

`gui/install.sh` checks that the Qt Quick modules the window needs are installed,
copies the app to `~/.local/share/shairport-sync-gui`, links
`~/.local/bin/shairport-sync-gui`, and adds "AirPlay Receiver" to the application
launcher. The desktop file is named `shairport-sync.desktop` to match the
`DesktopEntry` that shairport-sync reports over MPRIS, so Plasma's media controls
show the same icon. `sh gui/install.sh --uninstall` removes the app again; do that
before you install the package, as this copy would be started instead of the
package's.

## Firewall

Fedora's firewall blocks the PTP ports (UDP 319 and 320) that AirPlay 2 needs for
timing: senders can see the receiver, but nothing plays. The packages include a
firewalld service for all of the receiver's ports; open them once with:

```
sudo firewall-cmd --permanent --add-service=airplay-receiver
sudo firewall-cmd --reload
```

With another firewall, open UDP 319 and 320, TCP 7000, TCP and UDP 32768–60999 (for
AirPlay 2's audio streams) and mDNS (UDP 5353); classic AirPlay also needs TCP 5000
and UDP 6001–6010.

## One user at a time

The receiver runs as a systemd user service of whoever is logged in, but its ports
belong to the whole computer, so only one user at a time can run it. If a second
user logs in while the first one's receiver is running, theirs gives up after a few
tries and their app says it has stopped working. Once the first user has logged
out, Start Receiver in the app starts it, as do these commands:

```
systemctl --user reset-failed nqptp.service shairport-sync.service
systemctl --user start shairport-sync.service
```

System accounts, such as the login screen's, never run the receiver.

## Notes

* Seeking isn't offered: AirPlay senders don't accept it from the receiver.
* If the receiver or NQPTP keeps failing, for example over a mistake in the
  settings, the window says "The receiver stopped unexpectedly" and shows the
  `journalctl` command for its log; Stop Receiver ends the retries.
* Cover art is kept in `$XDG_RUNTIME_DIR/shairport-sync/coverart`, which only you
  can read. The receiver points `cover_art_cache_directory` there when it starts,
  as long as that line is still commented out as shipped.
* The window can be run straight from the source tree: `gui/shairport-sync-gui`.
