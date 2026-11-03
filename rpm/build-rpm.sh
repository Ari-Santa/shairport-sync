#!/bin/sh

# build-rpm.sh - Builds the shairport-sync-desktop RPM in a Fedora container
#
# Usage: rpm/build-rpm.sh [fedora-release]      (default: 45)
#
# Packs the working tree (tracked and new, non-ignored files, plus the nqptp
# submodule) into a source tarball, builds it with rpmbuild inside
# registry.fedoraproject.org/fedora:<release> using podman, and copies the
# binary and source RPMs next to the source tree, where dpkg-buildpackage
# puts the .deb.

set -e

FEDORA=${1:-45}
TOP=$(cd "$(dirname "$0")/.." && pwd)
SPEC="$TOP/rpm/shairport-sync-desktop.spec"
OUT=$(dirname "$TOP")

VERSION=$(sed -n 's/^AC_INIT(\[shairport-sync\], \[\([^]]*\)\].*/\1/p' "$TOP/configure.ac")
SPEC_VERSION=$(sed -n 's/^Version:[[:space:]]*//p' "$SPEC")
if [ "$VERSION" != "$SPEC_VERSION" ]; then
    echo "[FAIL] configure.ac says $VERSION but the spec says $SPEC_VERSION." >&2
    exit 1
fi
if [ ! -f "$TOP/nqptp/configure.ac" ]; then
    echo "[FAIL] nqptp/ is empty: run 'git submodule update --init' first." >&2
    exit 1
fi
command -v podman >/dev/null 2>&1 || { echo "[FAIL] podman is needed." >&2; exit 1; }

WORK=$(mktemp -d)
cleanup() { [ -n "$WORK" ] && [ -d "$WORK" ] && rm -r -- "$WORK"; }
trap cleanup EXIT
mkdir -p "$WORK/SOURCES"

{
    git -C "$TOP" ls-files --cached --others --exclude-standard | grep -v -x nqptp
    git -C "$TOP/nqptp" ls-files | sed 's|^|nqptp/|'
} | sort -u > "$WORK/files"
tar -C "$TOP" -czf "$WORK/SOURCES/shairport-sync-desktop-$VERSION.tar.gz" \
    --transform "s|^|shairport-sync-desktop-$VERSION/|" -T "$WORK/files"
cp "$SPEC" "$WORK/"

echo "[..] Building shairport-sync-desktop $VERSION on Fedora $FEDORA..."
podman run --rm -v "$WORK:/work:Z" "registry.fedoraproject.org/fedora:$FEDORA" sh -ec '
    dnf -y -q install rpm-build "dnf-command(builddep)" >/dev/null
    dnf -y -q builddep /work/shairport-sync-desktop.spec >/dev/null
    rpmbuild -ba --define "_topdir /work" /work/shairport-sync-desktop.spec
'

cp "$WORK"/RPMS/*/shairport-sync-desktop-"$VERSION"-*.rpm "$WORK"/SRPMS/*.src.rpm "$OUT/"
echo "[OK] RPMs are in $OUT:"
ls -1 "$OUT"/shairport-sync-desktop-"$VERSION"-*.rpm
