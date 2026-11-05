Name:           shairport-sync-desktop
Version:        5.6.2
Release:        3%{?dist}
Summary:        AirPlay 2 audio receiver for desktop users

# Shairport Sync is MIT, with tinyhttp (linked for the DACP client that the
# D-Bus and MPRIS interfaces need) under BSD-2-Clause; NQPTP and the NQPTP
# interface header Shairport Sync includes are GPL-2.0-only; the AirPlay
# Receiver app and packaging are MIT.
License:        MIT AND BSD-2-Clause AND GPL-2.0-only
URL:            https://github.com/Ari-Santa/shairport-sync
# Made by rpm/build-rpm.sh from the git tree, including the nqptp submodule.
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  autoconf
BuildRequires:  automake
BuildRequires:  desktop-file-utils
BuildRequires:  firewalld-filesystem
BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  libtool
BuildRequires:  make
BuildRequires:  pkgconfig(alsa)
BuildRequires:  pkgconfig(avahi-client)
BuildRequires:  pkgconfig(gio-unix-2.0)
BuildRequires:  pkgconfig(libavcodec)
BuildRequires:  pkgconfig(libavformat)
BuildRequires:  pkgconfig(libavutil)
BuildRequires:  pkgconfig(libconfig)
BuildRequires:  pkgconfig(libgcrypt)
BuildRequires:  pkgconfig(libpipewire-0.3)
BuildRequires:  pkgconfig(libplist-2.0)
BuildRequires:  pkgconfig(libpulse)
BuildRequires:  pkgconfig(libsodium)
BuildRequires:  pkgconfig(libswresample)
BuildRequires:  pkgconfig(openssl)
BuildRequires:  pkgconfig(popt)
BuildRequires:  pkgconfig(soxr)
BuildRequires:  pkgconfig(uuid)
BuildRequires:  /usr/bin/plistutil
BuildRequires:  systemd-rpm-macros
BuildRequires:  xxd

Requires:       avahi
Requires:       firewalld-filesystem
Requires:       kf6-kirigami
Requires:       kf6-qqc2-desktop-style
Requires:       python3
Requires:       python3-pyqt6
Requires:       qt6-qtdeclarative
Requires:       qt6-qtsvg
Recommends:     pipewire
Recommends:     pipewire-utils
Recommends:     wireplumber
%{?systemd_requires}

# Replaces Fedora's own shairport-sync (4.x, system service, no AirPlay 2).
Provides:       shairport-sync = %{version}-%{release}
Obsoletes:      shairport-sync < 5
Conflicts:      nqptp

%description
Shairport Sync built as an AirPlay 2 receiver that runs as a systemd user
service in the desktop session and plays through PipeWire, together with
NQPTP, the timing companion AirPlay 2 needs, and AirPlay Receiver, a
Kirigami app that shows what is playing and controls the receiver.

The receiver starts automatically when a user logs in. Each user's settings
live in ~/.config/shairport-sync/shairport-sync.conf, which is created from
/etc/shairport-sync-desktop.conf the first time the receiver runs. The
AirPlay ports belong to the whole machine, so only one user at a time can run
the receiver.

The firewall that Fedora turns on by default blocks the PTP ports (UDP 319
and 320) that AirPlay 2 needs for timing. To open them and the receiver's
other ports, run "firewall-cmd --permanent --add-service=airplay-receiver"
and then "firewall-cmd --reload" as root.

%prep
%autosetup
test -f nqptp/configure.ac
cp -p gui/README.md README.gui.md
# %%license installs by base name, and nqptp/LICENSE is LICENSE too.
cp -p tinyhttp/LICENSE LICENSE.tinyhttp

%build
autoreconf -fi
%configure \
    --with-airplay-2 \
    --with-alsa \
    --with-avahi \
    --with-dbus-interface \
    --with-mpris-interface \
    --with-pipewire \
    --with-pulseaudio \
    --with-soxr \
    --with-ssl=openssl
%make_build

pushd nqptp
autoreconf -fi
%configure
%make_build
popd

%install
# The D-Bus policy files are only for the system bus; this package puts both
# interfaces on the session bus, so stage them aside and drop them.
%make_install DBUS_POLICY_DIR=/dbus-policy-unused
rm -r %{buildroot}/dbus-policy-unused
pushd nqptp
%make_install
popd

# The desktop defaults, from which each user's copy is made, get a name of
# their own: /etc/shairport-sync.conf is the path of Fedora's shairport-sync
# config, and taking it over can keep that package's edited copy instead. The
# stock sample is left out, as the defaults file has every setting and its
# explanation too.
mv %{buildroot}%{_sysconfdir}/shairport-sync.conf %{buildroot}%{_sysconfdir}/shairport-sync-desktop.conf
rm %{buildroot}%{_sysconfdir}/shairport-sync.conf.sample
sh packaging/apply-desktop-defaults.sh %{buildroot}%{_sysconfdir}/shairport-sync-desktop.conf

install -D -m 0644 packaging/shairport-sync.service %{buildroot}%{_userunitdir}/shairport-sync.service
install -D -m 0644 packaging/nqptp.service %{buildroot}%{_userunitdir}/nqptp.service
install -D -m 0644 packaging/airplay-receiver.xml %{buildroot}%{_prefix}/lib/firewalld/services/airplay-receiver.xml
install -d %{buildroot}%{_userpresetdir}
echo "enable shairport-sync.service" > %{buildroot}%{_userpresetdir}/80-shairport-sync-desktop.preset

install -D -m 0755 gui/shairport-sync-gui %{buildroot}%{_datadir}/shairport-sync-gui/shairport-sync-gui
install -D -m 0644 gui/qml/Main.qml %{buildroot}%{_datadir}/shairport-sync-gui/qml/Main.qml
install -D -m 0644 gui/shairport-sync.svg %{buildroot}%{_datadir}/shairport-sync-gui/shairport-sync.svg
install -D -m 0644 gui/shairport-sync.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/shairport-sync.svg
install -D -m 0644 gui/shairport-sync.desktop %{buildroot}%{_datadir}/applications/shairport-sync.desktop
ln -s ../share/shairport-sync-gui/shairport-sync-gui %{buildroot}%{_bindir}/shairport-sync-gui
install -D -m 0644 packaging/shairport-sync-gui.1 %{buildroot}%{_mandir}/man1/shairport-sync-gui.1

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/shairport-sync.desktop
%{buildroot}%{_bindir}/shairport-sync -V | grep -q AirPlay2

%post
# Enables the receiver for every user's future logins (via the preset).
%systemd_user_post shairport-sync.service
# Makes the airplay-receiver firewalld service known.
%{?firewalld_reload}
if [ "$1" -eq 1 ]; then
    # Start it in the sessions of users logged in right now, so it works
    # without logging out. The units never run for system accounts, such as
    # the login screen's, so accounts below UID 1000 are skipped.
    if command -v loginctl >/dev/null 2>&1; then
        loginctl list-users --no-legend 2>/dev/null | while read -r uid user _; do
            [ "$uid" -ge 1000 ] 2>/dev/null || continue
            # Only running user managers: going through --machine to a user
            # whose manager has stopped (logged out, still listed) starts one.
            systemctl is-active --quiet "user@$uid.service" || continue
            # Only where it is enabled: an admin's preset that disables it
            # keeps it off.
            if systemctl --user --machine="$user@" is-enabled --quiet shairport-sync.service >/dev/null 2>&1; then
                systemctl --user --machine="$user@" start shairport-sync.service >/dev/null 2>&1 || :
            fi
        done
    fi
    # Fedora's guidelines leave opening ports to the admin, so only say how.
    if firewall-cmd --state >/dev/null 2>&1 &&
            ! firewall-cmd --query-service=airplay-receiver >/dev/null 2>&1 &&
            ! firewall-cmd --query-service=ptp >/dev/null 2>&1; then
        echo "shairport-sync-desktop: the firewall blocks the PTP ports AirPlay 2 needs." \
             "To open the receiver's ports, run" \
             "\"sudo firewall-cmd --permanent --add-service=airplay-receiver\"" \
             "and then \"sudo firewall-cmd --reload\"."
    fi
elif [ -x /usr/lib/systemd/systemd-update-helper ]; then
    # On upgrade, mark the running units for a restart. systemd's file trigger
    # restarts them after the transaction, once the user managers have loaded
    # the new unit files; restarting them here would keep the old ones.
    /usr/lib/systemd/systemd-update-helper mark-restart-user-units nqptp.service shairport-sync.service || :
fi
:

%preun
# Stop the receiver in running sessions so nobody is left with a deleted
# binary still holding the AirPlay ports.
if [ "$1" -eq 0 ] && command -v loginctl >/dev/null 2>&1; then
    loginctl list-users --no-legend 2>/dev/null | while read -r uid user _; do
        [ "$uid" -ge 1000 ] 2>/dev/null || continue
        # Only running user managers: going through --machine to a user whose
        # manager has stopped (logged out, still listed) starts one.
        systemctl is-active --quiet "user@$uid.service" || continue
        systemctl --user --machine="$user@" stop shairport-sync.service nqptp.service >/dev/null 2>&1 || :
    done
fi
%systemd_user_preun shairport-sync.service

%files
%license COPYING LICENSES LICENSE.tinyhttp nqptp/LICENSE
%doc README.md README.gui.md
%config(noreplace) %{_sysconfdir}/shairport-sync-desktop.conf
%{_bindir}/shairport-sync
# NQPTP runs in each user's session but must bind the PTP ports 319 and 320.
%caps(cap_net_bind_service=ep) %{_bindir}/nqptp
%{_bindir}/shairport-sync-gui
%{_datadir}/shairport-sync-gui/
%{_datadir}/applications/shairport-sync.desktop
%{_datadir}/icons/hicolor/scalable/apps/shairport-sync.svg
%{_userunitdir}/shairport-sync.service
%{_userunitdir}/nqptp.service
%{_userpresetdir}/80-shairport-sync-desktop.preset
%{_prefix}/lib/firewalld/services/airplay-receiver.xml
%{_mandir}/man1/shairport-sync.1*
%{_mandir}/man1/shairport-sync-gui.1*
%{_mandir}/man8/nqptp.8*

%changelog
* Thu Nov 05 2026 Ari-Santa <apeach@live.no> - 5.6.2-3
- Install the desktop defaults as /etc/shairport-sync-desktop.conf and make
  each user's settings from that. /etc/shairport-sync.conf is the path of
  Fedora's shairport-sync config: replacing that package after its config
  was edited kept the old file, which put the receiver's D-Bus and MPRIS
  interfaces on the system bus with ALSA output. On upgrade, the old file
  and the stock sample are removed, or saved as .rpmsave if they were
  edited
- Start shairport-sync with --dbus-default-message-bus=session, so the D-Bus
  and MPRIS interfaces are on the session bus even when a user's settings
  name no bus
- Never run the receiver or NQPTP for system accounts, such as the login
  screen's. Only one user at a time can run the receiver, as its ports
  belong to the whole machine; a second user's copy now stops retrying
  after a few tries. shairport-sync no longer starts when NQPTP cannot be
  started, instead of quietly falling back to classic AirPlay. Restarting
  NQPTP, which systemd also does after a crash, now restarts shairport-sync
  with it, ending a session in progress
- Keep each user's cover art in a private directory under $XDG_RUNTIME_DIR
  instead of /tmp/shairport-sync, which all users shared and another local
  user could point elsewhere. Settings copied by earlier versions are moved
  over too, unless cover_art_cache_directory was set in them by hand
- Add a firewalld service, airplay-receiver, for the ports AirPlay 2 needs,
  which Fedora's firewall blocks; the description and a note on first
  install say how to enable it
- Add a manual page for shairport-sync-gui, and
  rpm/shairport-sync-desktop.rpmlintrc for rpmlint's false positives
- Restart the receiver on upgrade after systemd has loaded the new unit
  files, through systemd-update-helper, instead of with the old ones. On
  install and removal, users whose systemd manager has stopped are left
  alone instead of getting a new one started, and on install the receiver
  is started only where it is enabled, so a preset that disables it keeps
  it off
- AirPlay Receiver: never restart the receiver while a sender is connected,
  paused senders included, or start one that was stopped. A restart still
  due to apply a setting is remembered when the app is closed
- AirPlay Receiver: read and change the output and volume settings with a
  parser for libconfig's syntax, so settings edited by hand no longer end up
  with duplicate settings that stop the receiver, or with changes the
  receiver ignores. Saving is atomic, keeps the file's permissions, owner
  and symlinks, and refuses a read-only file or one with a syntax error,
  naming the line. Bytes that aren't UTF-8 no longer stop the app from
  starting, and output names containing "${" are saved literally
- AirPlay Receiver: pick up output and volume settings changed in the file
  by hand, and a receiver that was still starting when the app first
  looked. Stepping through outputs saves and restarts once, and the picker
  always shows the real choice
- AirPlay Receiver: with Use Device Volume, keep AirPlay on the device whose
  volume the sender controls until the session ends, and warn, then restart
  the receiver, when that device wasn't there when it started. Only the
  receiver's own PipeWire stream is moved, and only with the PipeWire
  backend
- AirPlay Receiver: show a receiver or NQPTP that keeps failing, with a Stop
  action, and let Start and Restart clear systemd's start limit
- AirPlay Receiver: one window per user. A window that can't load is
  reported in a dialog, Plasma's Qt Quick style is used only where it is
  installed, and the footer shows whether AirPlay 2 or classic AirPlay is in
  use. The volume slider's arrow keys step from the current volume, and
  Space no longer blocks focused buttons
- AirPlay Receiver: copy a new user's settings from
  /etc/shairport-sync-desktop.conf, or from /etc/shairport-sync.conf where
  that is missing
- README.gui.md: document the 5.6.2-3 packages and the container builds,
  the new defaults file, the firewall and one receiver per computer, and a
  from-source setup with the user units the app needs. gui/install.sh checks
  for the Qt Quick modules the window needs and removes the app again with
  --uninstall
- License: BSD-2-Clause for the linked tinyhttp instead of BSD-3-Clause for
  code this build does not compile; ship tinyhttp's license text
- build-rpm.sh: Ctrl-C now stops the build and removes the container and
  the temporary work directory

* Wed Nov 04 2026 Ari-Santa <apeach@live.no> - 5.6.2-2
- AirPlay Receiver: add "Use Device Volume" to the menu, so the sender's
  volume control can set the output device's own volume
  (pipewire.mixer_type = "sink") instead of turning the audio down in
  software

* Tue Nov 03 2026 Ari-Santa <apeach@live.no> - 5.6.2-1
- First RPM build, for Fedora: Shairport Sync 5.6.2 for AirPlay 2, NQPTP
  1.2.9 and the AirPlay Receiver app, matching the Debian package 5.6.2-1
- Shairport Sync 5.6.2 fixes out-of-bounds reads on malformed DACP TLV
  tokens (GHSA-vfxr-hxq9-9h35) and keeps sessions longer than about 12.5
  hours playing
