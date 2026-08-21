#!/bin/sh
# Assemble the complete relocatable /usr/local-shaped product payload.
set -eu

if [ "$#" -ne 3 ]; then
    printf 'usage: %s <product-root> <hydration-api-root> <output.tar.gz>\n' "$0" >&2
    exit 2
fi

product=$(cd "$1" && pwd)
framework=$(cd "$2" && pwd)
output=$3
case "$output" in
    /*) ;;
    *) output=$(pwd)/$output ;;
esac

stage=$(mktemp -d "${TMPDIR:-/tmp}/onedrive-hydration-release.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM
payload=$stage/payload

install -d \
    "$payload/bin" \
    "$payload/share/doc/onedrive-hydration" \
    "$payload/share/onedrive-hydration"

for binary in \
    onedrive-hydration-daemon \
    onedrive-hydrationctl \
    onedrive-hydration-dbus \
    onedrive-hydration-tray \
    onedrive-hydration-install
do
    install -m 0755 "$product/target/release/$binary" "$payload/bin/$binary"
done
# The validated installer expects every unit payload in the configured
# --bin-dir. Keep the privileged helper beside the product binaries rather
# than inventing a libexec path the installer cannot use.
install -m 0755 "$framework/target/release/hydrationd" "$payload/bin/hydrationd"

install -m 0644 \
    "$product/README.md" \
    "$product/LICENSE-APACHE" \
    "$product/LICENSE-MIT" \
    "$payload/share/doc/onedrive-hydration/"
cp -a "$product/docs" "$payload/share/doc/onedrive-hydration/"
cp -a "$product/packaging" "$payload/share/onedrive-hydration/"

revision=$("$product/tools/hydration-api-rev.sh" "$product/Cargo.lock")
printf '%s\n' "$revision" > "$payload/share/doc/onedrive-hydration/HYDRATION_API_REVISION"
(
    cd "$payload"
    find . -type f ! -name MANIFEST.sha256 -print0 \
        | sort -z \
        | xargs -0 sha256sum > share/doc/onedrive-hydration/MANIFEST.sha256
)

epoch=${SOURCE_DATE_EPOCH:-0}
mkdir -p "$(dirname "$output")"
tar \
    --sort=name \
    --mtime="@$epoch" \
    --owner=0 --group=0 --numeric-owner \
    -czf "$output" -C "$payload" .
(
    cd "$(dirname "$output")"
    sha256sum "$(basename "$output")"
) > "$output.sha256"
