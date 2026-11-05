#!/bin/sh

# build-deb.sh - Builds the shairport-sync-desktop .deb in a container
#
# Usage: debian/build-deb.sh [image]      (default: debian:trixie)
#
# Examples: debian/build-deb.sh debian:trixie
#           debian/build-deb.sh ubuntu:26.10
#
# Packs the working tree (tracked and new, non-ignored files, plus the nqptp
# submodule), builds it with dpkg-buildpackage inside the given image using
# podman, and copies the .deb next to the source tree, where a host
# dpkg-buildpackage would put it.
#
# A binary .deb installs only on the release it was built on, because it
# depends on that release's library versions (libavcodec61 and libsodium23 on
# Debian 13, libavcodec62 and libsodium26 on Ubuntu 26.10, ...). When the
# image is not the release that debian/changelog names, the build gets a
# version suffix such as ~ubuntu26.10, so packages for several releases can
# be published side by side.

set -e

IMAGE=${1:-debian:trixie}
TOP=$(cd "$(dirname "$0")/.." && pwd)
OUT=$(dirname "$TOP")

if [ ! -f "$TOP/nqptp/configure.ac" ]; then
    echo "[FAIL] nqptp/ is empty: run 'git submodule update --init' first." >&2
    exit 1
fi
command -v podman >/dev/null 2>&1 || { echo "[FAIL] podman is needed." >&2; exit 1; }

WORK=$(mktemp -d)
CONTAINER=shairport-sync-desktop-build-$$
cleanup() {
    podman rm -f -t 0 "$CONTAINER" >/dev/null 2>&1 || true
    [ -n "$WORK" ] && [ -d "$WORK" ] && rm -r -- "$WORK"
}
trap cleanup EXIT
# dash skips the EXIT trap when a signal kills it, so turn the signals into
# an exit. podman does not pass them on (--sig-proxy=false): it returns at
# once, and cleanup removes the container, which is still building.
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -p "$WORK/src" "$WORK/out"

{
    git -C "$TOP" ls-files --cached --others --exclude-standard | grep -v -x nqptp
    git -C "$TOP/nqptp" ls-files | sed 's|^|nqptp/|'
} | sort -u > "$WORK/files"
tar -C "$TOP" -cf - -T "$WORK/files" | tar -C "$WORK/src" -xf -

echo "[..] Building shairport-sync-desktop in $IMAGE..."
podman run --rm --sig-proxy=false --name "$CONTAINER" -v "$WORK:/work:Z" "$IMAGE" sh -ec '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends build-essential >/dev/null
    cd /work/src

    . /etc/os-release
    if [ "$VERSION_CODENAME" != "$(dpkg-parsechangelog -S Distribution)" ]; then
        VERSION="$(dpkg-parsechangelog -S Version)~${ID}${VERSION_ID}"
        {
            echo "shairport-sync-desktop ($VERSION) $VERSION_CODENAME; urgency=medium"
            echo
            echo "  * Rebuild for $PRETTY_NAME."
            echo
            echo " -- $(dpkg-parsechangelog -S Maintainer)  $(date -R)"
            echo
            cat debian/changelog
        } > debian/changelog.new
        mv debian/changelog.new debian/changelog
    fi

    apt-get build-dep -y -qq --no-install-recommends ./ >/dev/null
    dpkg-buildpackage -b -us -uc
    cp /work/shairport-sync-desktop_*.deb /work/out/
'

cp "$WORK"/out/*.deb "$OUT/"
echo "[OK] Built in $IMAGE:"
for deb in "$WORK"/out/*.deb; do
    echo "$OUT/$(basename "$deb")"
done
