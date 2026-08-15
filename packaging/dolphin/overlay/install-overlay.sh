#!/bin/sh
# Build and install the Dolphin overlay plugin: the per-file on-device / cloud-
# only emblem for the OneDrive sync folder. Unlike the servicemenu (data, user
# scope), this is a compiled KF6 plugin and it must land in the *system* Qt
# plugin dir — measured: a plugin under ~/.local/lib/qt6/plugins is not searched
# by Dolphin, only the app-relative /usr/lib/qt6/plugins and $QT_PLUGIN_PATH are.
# So the plugin install needs root; the roots config it reads is written user
# scope, next to the rest of this deployment's per-user state.
#
# Three things happen here, and the script says which privilege each needs:
#   1. build the .so           (no privilege; needs cmake + Qt6 + KF6 dev)
#   2. install it system-wide  (root; into the Qt plugin dir)
#   3. write the roots config  (no privilege; $XDG_CONFIG_HOME)
# and one cleanup: the donor client's overlay plugin, which reads a different
# xattr and would now draw a *second*, wrong badge on every file, is removed —
# because for the first time this product ships an overlay of its own, so the two
# are a real collision, not the harmless dead weight the servicemenu note called
# it before.
set -eu

here=$(dirname "$(readlink -f "$0")")
mount=$HOME/OneDrive
build_dir=

usage() {
    cat >&2 <<EOF
usage: install-overlay.sh [--mount <path>] [--build-dir <dir>]

  --mount      the sync root the emblems apply to; files outside it are never
               badged. May be given more than once for several roots.
               default: \$HOME/OneDrive
  --build-dir  where to build; default: a fresh temp dir, removed after.

Builds the KF6 overlay plugin, installs it system-wide (needs sudo), writes the
sync roots to \$XDG_CONFIG_HOME/onedrive-hydration/overlay-roots, and removes the
donor client's conflicting overlay plugin if present.
EOF
    exit 2
}

roots=
add_root() {
    r=${1%/}
    if [ ! -d "$r" ]; then
        printf 'refused: sync root %s does not exist.\n' "$r" >&2
        printf 'The emblems key off this path; a wrong one silently badges nothing.\n' >&2
        exit 1
    fi
    roots="$roots$r
"
}

seen_mount=
while [ "$#" -gt 0 ]; do
    case $1 in
        --mount) [ "$#" -ge 2 ] || usage; add_root "$2"; seen_mount=1; shift 2 ;;
        --build-dir) [ "$#" -ge 2 ] || usage; build_dir=$2; shift 2 ;;
        -h | --help) usage ;;
        *) printf 'unknown argument: %s\n\n' "$1" >&2; usage ;;
    esac
done
[ -n "$seen_mount" ] || add_root "$mount"

# The build needs a Qt6/KF6 toolchain — the one dependency escalation this
# feature carries (docs/DOLPHIN-GROUNDWORK.md). Refuse with the missing tool
# named rather than let cmake fail three screens later.
if ! command -v cmake >/dev/null 2>&1; then
    printf 'refused: cmake is not installed.\n' >&2
    printf 'The overlay plugin is compiled; install cmake, extra-cmake-modules,\n' >&2
    printf 'and the Qt6 + KF6 (KIO) development packages, then re-run.\n' >&2
    exit 1
fi

cleanup_build=
if [ -z "$build_dir" ]; then
    build_dir=$(mktemp -d "${TMPDIR:-/tmp}/onedrive-overlay-build.XXXXXX")
    cleanup_build=$build_dir
fi
trap '[ -n "$cleanup_build" ] && rm -rf "$cleanup_build"' EXIT

printf 'building the overlay plugin...\n'
# Configure and build. Capture output so a failure shows what actually happened
# — never a swallowed diagnostic (that trap cost real CI time in the framework).
if ! cmake -S "$here" -B "$build_dir" -DCMAKE_INSTALL_PREFIX=/usr \
        -DCMAKE_BUILD_TYPE=Release >"$build_dir/configure.log" 2>&1; then
    printf 'refused: the plugin did not configure. cmake said:\n' >&2
    sed 's/^/  /' "$build_dir/configure.log" >&2
    printf 'This usually means a missing dev package (Qt6 Core, KF6 KIO, ECM).\n' >&2
    exit 1
fi
if ! cmake --build "$build_dir" >"$build_dir/build.log" 2>&1; then
    printf 'refused: the plugin did not build. the compiler said:\n' >&2
    sed 's/^/  /' "$build_dir/build.log" >&2
    exit 1
fi

so=$(find "$build_dir" -name 'onedrive-hydration-overlay.so' -print 2>/dev/null | head -n 1)
if [ -z "$so" ] || [ ! -f "$so" ]; then
    printf 'refused: the build reported success but produced no plugin .so.\n' >&2
    printf 'Nothing was installed. Look under %s.\n' "$build_dir" >&2
    exit 1
fi
printf 'built: %s\n' "$so"

# Install system-wide. `cmake --install` honours CMAKE_INSTALL_PREFIX=/usr and
# KDE_INSTALL_PLUGINDIR, so the .so lands in the same kf6/overlayicon dir Dolphin
# searches. Needs root; run under sudo only if we are not already root.
sudo=
if [ "$(id -u)" -ne 0 ]; then
    if command -v sudo >/dev/null 2>&1; then
        sudo=sudo
        printf 'installing system-wide (needs root)...\n'
    else
        printf 'refused: the plugin must be installed to the system Qt plugin dir,\n' >&2
        printf 'which needs root, and sudo is not available. Re-run this script as root.\n' >&2
        exit 1
    fi
fi
$sudo cmake --install "$build_dir" >"$build_dir/install.log" 2>&1 || {
    printf 'refused: install failed. cmake said:\n' >&2
    sed 's/^/  /' "$build_dir/install.log" >&2
    exit 1
}

# The roots config: user scope, one absolute path per line, read by the plugin
# at startup. Without it a resident file — which carries no mark — is
# indistinguishable from any other file on the system, so the plugin would badge
# nothing (safe) or everything (wrong). This is what scopes it to the sync
# folder. Rewritten wholesale from the roots given, so a re-run with a new
# --mount replaces rather than appends.
config_home=${XDG_CONFIG_HOME:-$HOME/.config}
roots_file=$config_home/onedrive-hydration/overlay-roots
mkdir -p "$(dirname "$roots_file")"
{
    printf '# Sync roots the OneDrive overlay plugin badges. One absolute path per\n'
    printf '# line. Written by install-overlay.sh; edit to add or remove roots.\n'
    printf '%s' "$roots"
} >"$roots_file.tmp"
mv -f "$roots_file.tmp" "$roots_file"

printf 'installed the overlay plugin.\n'
printf 'sync roots (%s):\n' "$roots_file"
printf '%s' "$roots" | sed 's/^/  /'

# The donor collision. The old OneDriveForLinux client's overlay plugin reads
# user.onedrive.syncstate; this product writes user.hydration.*. Before today
# that plugin drew nothing here and was left alone. Now that we ship an overlay
# of our own, two plugins returning emblems for the same files is a visible
# double badge, so the donor is removed — as part of the privileged install the
# user already consented to by running this script, not silently.
removed=
for dir in /usr/lib/qt6/plugins/kf6/overlayicon /usr/lib64/qt6/plugins/kf6/overlayicon; do
    [ -d "$dir" ] || continue
    for donor in "$dir"/*onedrive*.so; do
        [ -e "$donor" ] || continue
        case "$donor" in
            */onedrive-hydration-overlay.so) continue ;; # our own, just installed
        esac
        printf 'removing the donor client overlay plugin (it would double-badge): %s\n' "$donor"
        $sudo rm -f "$donor" && removed=1
    done
done
[ -n "$removed" ] || printf 'no conflicting donor overlay plugin found.\n'

# Dolphin loads overlay plugins at process start. A window already open will not
# pick this up; a Dolphin started from now on will. Not asserted: whether the
# file-manager service needs a restart beyond that (unmeasured, so not claimed).
printf 'A Dolphin started from now on shows the emblems.\n'
printf 'For an already-open window, close it and reopen (or: kquitapp6 dolphin).\n'
