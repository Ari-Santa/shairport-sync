Name:           shairport-sync-desktop
Version:        5.6.2
Release:        1%{?dist}
Summary:        AirPlay 2 audio receiver for desktop users

# Shairport Sync is MIT with BSD-3-Clause parts (ALAC, pair_ap/evrtsp);
# NQPTP is GPL-2.0-only; the AirPlay Receiver app and packaging are MIT.
License:        MIT AND BSD-3-Clause AND GPL-2.0-only
URL:            https://github.com/Ari-Santa/shairport-sync
# Made by rpm/build-rpm.sh from the git tree, including the nqptp submodule.
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  autoconf
BuildRequires:  automake
BuildRequires:  desktop-file-utils
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
/etc/shairport-sync.conf the first time the receiver runs.

%prep
%autosetup
test -f nqptp/configure.ac
cp -p gui/README.md README.gui.md

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

sh packaging/apply-desktop-defaults.sh %{buildroot}%{_sysconfdir}/shairport-sync.conf

install -D -m 0644 packaging/shairport-sync.service %{buildroot}%{_userunitdir}/shairport-sync.service
install -D -m 0644 packaging/nqptp.service %{buildroot}%{_userunitdir}/nqptp.service
install -d %{buildroot}%{_userpresetdir}
echo "enable shairport-sync.service" > %{buildroot}%{_userpresetdir}/80-shairport-sync-desktop.preset

install -D -m 0755 gui/shairport-sync-gui %{buildroot}%{_datadir}/shairport-sync-gui/shairport-sync-gui
install -D -m 0644 gui/qml/Main.qml %{buildroot}%{_datadir}/shairport-sync-gui/qml/Main.qml
install -D -m 0644 gui/shairport-sync.svg %{buildroot}%{_datadir}/shairport-sync-gui/shairport-sync.svg
install -D -m 0644 gui/shairport-sync.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/shairport-sync.svg
install -D -m 0644 gui/shairport-sync.desktop %{buildroot}%{_datadir}/applications/shairport-sync.desktop
ln -s ../share/shairport-sync-gui/shairport-sync-gui %{buildroot}%{_bindir}/shairport-sync-gui

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/shairport-sync.desktop
%{buildroot}%{_bindir}/shairport-sync -V | grep -q AirPlay2

%post
# Enables the receiver for every user's future logins (via the preset).
%systemd_user_post shairport-sync.service
# Start it (or restart it, on upgrade) in the sessions of users logged in
# right now, so it works without logging out. Accounts below UID 1000, such
# as the login screen's, are skipped.
if command -v loginctl >/dev/null 2>&1; then
    loginctl list-users --no-legend 2>/dev/null | while read -r uid user _; do
        [ "$uid" -ge 1000 ] 2>/dev/null || continue
        if [ "$1" -eq 1 ]; then
            systemctl --user --machine="$user@" start shairport-sync.service >/dev/null 2>&1 || :
        else
            systemctl --user --machine="$user@" try-restart nqptp.service shairport-sync.service >/dev/null 2>&1 || :
        fi
    done
fi
:

%preun
# Stop the receiver in running sessions so nobody is left with a deleted
# binary still holding the AirPlay ports.
if [ "$1" -eq 0 ] && command -v loginctl >/dev/null 2>&1; then
    loginctl list-users --no-legend 2>/dev/null | while read -r uid user _; do
        [ "$uid" -ge 1000 ] 2>/dev/null || continue
        systemctl --user --machine="$user@" stop shairport-sync.service nqptp.service >/dev/null 2>&1 || :
    done
fi
%systemd_user_preun shairport-sync.service

%files
%license COPYING LICENSES nqptp/LICENSE
%doc README.md README.gui.md
%config(noreplace) %{_sysconfdir}/shairport-sync.conf
%config %{_sysconfdir}/shairport-sync.conf.sample
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
%{_mandir}/man1/shairport-sync.1*
%{_mandir}/man8/nqptp.8*

%changelog
* Tue Nov 03 2026 Ari-Santa <apeach@live.no> - 5.6.2-1
- First RPM build, for Fedora: Shairport Sync 5.6.2 for AirPlay 2, NQPTP
  1.2.9 and the AirPlay Receiver app, matching the Debian package 5.6.2-1
- Shairport Sync 5.6.2 fixes out-of-bounds reads on malformed DACP TLV
  tokens (GHSA-vfxr-hxq9-9h35) and keeps sessions longer than about 12.5
  hours playing
