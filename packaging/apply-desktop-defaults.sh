#!/bin/sh

# apply-desktop-defaults.sh - Turns the stock shairport-sync.conf into the
# default for a receiver running in a desktop session: PipeWire output, the
# D-Bus and MPRIS interfaces on the session bus, and metadata with cover art.
# Used by both the Debian and the RPM packaging.
#
# Usage: apply-desktop-defaults.sh path/to/shairport-sync.conf

set -e

CONF="$1"
if [ ! -f "$CONF" ]; then
    echo "usage: $0 path/to/shairport-sync.conf" >&2
    exit 1
fi

TAB=$(printf '\t')
sed -i \
    -e "s|^//${TAB}output_backend = \"alsa\";|${TAB}output_backend = \"pipewire\";|" \
    -e "s|^//${TAB}dbus_service_bus = \"system\";|${TAB}dbus_service_bus = \"session\";|" \
    -e "s|^//${TAB}mpris_service_bus = \"system\";|${TAB}mpris_service_bus = \"session\";|" \
    -e "s|^//${TAB}enabled = \"yes\"; // set this to yes to get Shairport Sync to solicit metadata|${TAB}enabled = \"yes\"; // set this to yes to get Shairport Sync to solicit metadata|" \
    -e "s|^//${TAB}include_cover_art = \"yes\";|${TAB}include_cover_art = \"yes\";|" \
    "$CONF"

# Fail the build if upstream reworded any of these lines.
applied=$(grep -c -E "^${TAB}(output_backend = \"pipewire\"|dbus_service_bus = \"session\"|mpris_service_bus = \"session\"|enabled = \"yes\"|include_cover_art = \"yes\")" "$CONF" || true)
if [ "$applied" -ne 5 ]; then
    echo "$0: expected 5 default settings in $CONF, applied $applied" >&2
    exit 1
fi
