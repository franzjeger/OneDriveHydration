#!/bin/sh
# Install the GNOME Files (Nautilus) "Free Up Space" and "Keep on Device"
# scripts for the current user. User scope on purpose, like the Dolphin
# servicemenu: this needs no privileges, and the scripts are data — POSIX
# shell under ~/.local/share/nautilus/scripts, no toolkit, no new dependency.
#
# Nautilus shows a script under right-click → Scripts, named by its
# *filename*, so the generated files are called exactly what the menu should
# say. There is no mimetype filter at all — the entries appear on every
# selection — which is why the containment check lives inside the scripts,
# the same place KIO's wrappers keep it.
#
# The icons are a prerequisite only for the result notifications
# (notify-send names the hicolor application icon); ../icons/install-icons.sh
# installs them. Without them the notification shows no icon and everything
# else still works.
set -eu

here=$(dirname "$(readlink -f "$0")")
mount=$HOME/OneDrive
bin_dir=/usr/local/bin

usage() {
    cat >&2 <<EOF
usage: install-nautilus-scripts.sh [--mount <path>] [--bin-dir <dir>]

  --mount    the sync root; the scripts refuse files outside it.
             default: \$HOME/OneDrive
  --bin-dir  where onedrive-hydrationctl lives, baked in as an absolute path
             so the script never depends on the session's PATH.
             default: /usr/local/bin

Installs two scripts under \$XDG_DATA_HOME/nautilus/scripts
(default ~/.local/share/nautilus/scripts).
EOF
    exit 2
}

while [ "$#" -gt 0 ]; do
    case $1 in
        --mount) [ "$#" -ge 2 ] || usage; mount=$2; shift 2 ;;
        --bin-dir) [ "$#" -ge 2 ] || usage; bin_dir=$2; shift 2 ;;
        -h | --help) usage ;;
        *) printf 'unknown argument: %s\n\n' "$1" >&2; usage ;;
    esac
done

mount=${mount%/}
data_home=${XDG_DATA_HOME:-$HOME/.local/share}

# Refuse rather than guess, and say which fact was wrong — the installer's
# style, for the same reason: every one of these is baked into a generated
# file that nothing will validate later.
if [ ! -d "$mount" ]; then
    printf 'refused: sync root %s does not exist.\n' "$mount" >&2
    printf 'Pass --mount, or set the deployment up first; this script bakes the\n' >&2
    printf 'path into the action and cannot discover it later.\n' >&2
    exit 1
fi
if [ ! -x "$bin_dir/onedrive-hydrationctl" ]; then
    printf 'refused: %s/onedrive-hydrationctl is missing or not executable.\n' "$bin_dir" >&2
    printf 'The script would appear in Files and fail only when clicked, which is\n' >&2
    printf 'too late to find out. Install the payload first, or pass --bin-dir.\n' >&2
    exit 1
fi
# Substitution below uses | as the sed delimiter; a path containing one would
# silently produce a corrupt script.
case "$mount$bin_dir" in
    *'|'*)
        printf 'refused: a path contains "|", which this script uses as its\n' >&2
        printf 'substitution delimiter: %s %s\n' "$mount" "$bin_dir" >&2
        exit 1
        ;;
esac

script_dir=$data_home/nautilus/scripts
mkdir -p "$script_dir"

for src_dst in "free-up-space.sh.in=Free Up Space" "keep-on-device.sh.in=Keep on Device"; do
    src=${src_dst%%=*}
    dst=$script_dir/${src_dst#*=}
    sed -e "s|@MOUNT@|$mount|g" -e "s|@CTL@|$bin_dir/onedrive-hydrationctl|g" \
        "$here/$src" > "$dst.tmp"
    chmod 755 "$dst.tmp"
    mv -f "$dst.tmp" "$dst"
done

printf 'installed:\n  %s/Free Up Space\n  %s/Keep on Device\n' "$script_dir" "$script_dir"
printf 'sync root: %s\n' "$mount"
printf 'They appear under right-click -> Scripts in GNOME Files. If a window that\n'
printf 'is already open does not show them, close its Files windows (nautilus -q)\n'
printf 'and reopen; whether a running Nautilus rescans the directory was not\n'
printf 'measured here, so this does not claim it does.\n'

if ! command -v zenity >/dev/null 2>&1 && ! command -v notify-send >/dev/null 2>&1; then
    printf '\nnote: neither zenity nor notify-send is installed, so the scripts have\n'
    printf 'nowhere to show their results — including the daemon reasons when it\n'
    printf 'keeps a file. They still work; the answers go to stderr.\n'
fi

# What GNOME does not get: per-file cloud/on-device emblems. The Dolphin
# side ships them as a compiled KF6 KOverlayIconPlugin, and GNOME Files has
# no equivalent overlay-plugin API to port it to — saying so here beats a
# reader hunting for an installer that does not exist.
printf '\nGNOME Files shows no cloud/on-device emblems: Nautilus has no overlay\n'
printf 'plugin API for the KF6 plugin the Dolphin side ships. The actions above\n'
printf 'work without them.\n'
