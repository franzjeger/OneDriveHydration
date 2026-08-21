#!/bin/sh
# Install the Dolphin "Free Up Space" action for the current user. User scope
# on purpose, like the plasmoid: this needs no privileges, and a system package
# would install the same two files under /usr/share instead.
#
# The actions are data — KIO servicemenu .desktop files plus small wrappers — so
# there is no toolkit and no new dependency, the same trade the tray and the
# flyout made. Overlay emblems cannot be data-only and ship separately as the
# compiled KF6 plugin under overlay/.
#
# The icons are a prerequisite, not a bundled asset: the entry names the same
# hicolor application icon the tray names, and ../icons/install-icons.sh
# installs it. Without it Dolphin draws a generic fallback.
set -eu

here=$(dirname "$(readlink -f "$0")")
mount=$HOME/OneDrive
bin_dir=/usr/local/bin

usage() {
    cat >&2 <<EOF
usage: install-servicemenu.sh [--mount <path>] [--bin-dir <dir>]

  --mount    the sync root; the action refuses files outside it.
             default: \$HOME/OneDrive
  --bin-dir  where onedrive-hydrationctl lives, baked in as an absolute path
             because Dolphin runs the action with a minimal environment.
             default: /usr/local/bin

Installs two servicemenus and three wrappers under \$XDG_DATA_HOME
(default ~/.local/share).
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
# style, for the same reason: every one of these is baked into a generated file
# that nothing will validate later.
if [ ! -d "$mount" ]; then
    printf 'refused: sync root %s does not exist.\n' "$mount" >&2
    printf 'Pass --mount, or set the deployment up first; this script bakes the\n' >&2
    printf 'path into the action and cannot discover it later.\n' >&2
    exit 1
fi
if [ ! -x "$bin_dir/onedrive-hydrationctl" ]; then
    printf 'refused: %s/onedrive-hydrationctl is missing or not executable.\n' "$bin_dir" >&2
    printf 'The action would appear in Dolphin and fail only when clicked, which is\n' >&2
    printf 'too late to find out. Install the payload first, or pass --bin-dir.\n' >&2
    exit 1
fi
# Substitution below uses | as the sed delimiter; a path containing one would
# silently produce a corrupt wrapper.
case "$mount$bin_dir" in
    *'|'*)
        printf 'refused: a path contains "|", which this script uses as its\n' >&2
        printf 'substitution delimiter: %s %s\n' "$mount" "$bin_dir" >&2
        exit 1
        ;;
esac

action_dir=$data_home/onedrive-hydration
menu_dir=$data_home/kio/servicemenus
action=$action_dir/free-up-space.sh
action2=$action_dir/keep-on-device.sh
action3=$action_dir/free-up-space-folder.sh
mkdir -p "$action_dir" "$menu_dir"

for src_dst in "free-up-space.sh.in=$action" "keep-on-device.sh.in=$action2" "free-up-space-folder.sh.in=$action3"; do
    src=${src_dst%%=*}
    dst=${src_dst#*=}
    sed -e "s|@MOUNT@|$mount|g" -e "s|@CTL@|$bin_dir/onedrive-hydrationctl|g" \
        "$here/$src" > "$dst.tmp"
    chmod 755 "$dst.tmp"
    mv -f "$dst.tmp" "$dst"
done

for menu_pair in \
    "servicemenu.desktop.in=onedrive-hydration.desktop" \
    "servicemenu-folder.desktop.in=onedrive-hydration-folder.desktop"; do
    msrc=$here/${menu_pair%%=*}
    mdst=$menu_dir/${menu_pair#*=}
    sed -e "s|@ACTION@|$action|g" -e "s|@ACTION2@|$action2|g" -e "s|@ACTION3@|$action3|g" "$msrc" > "$mdst.tmp"
    chmod 755 "$mdst.tmp"
    mv -f "$mdst.tmp" "$mdst"
done

printf 'installed:\n  %s\n  %s\n  %s\n  %s/onedrive-hydration.desktop\n  %s/onedrive-hydration-folder.desktop\n' \
    "$action" "$action2" "$action3" "$menu_dir" "$menu_dir"
printf 'sync root: %s\n' "$mount"

# Measured with probes/servicemenu-match.cpp: a servicemenu dropped into the
# directory is matched by a freshly started process with no kbuildsycoca6 run
# and no cache rebuild — the entry was found on a cold cache that had never
# seen it. So this deliberately does not tell anyone to rebuild anything.
# Whether an *already open* Dolphin window rescans the directory was not
# measured, and is not asserted here.
printf 'A Dolphin started from now on shows it; no cache rebuild is needed.\n'
printf 'If a window that is already open does not show it, reopen that window.\n'

if ! command -v kdialog >/dev/null 2>&1 && ! command -v notify-send >/dev/null 2>&1; then
    printf '\nnote: neither kdialog nor notify-send is installed, so the action has\n'
    printf 'nowhere to show its result — including the daemon reasons when it keeps\n'
    printf 'a file. It will still work; the answers go to stderr.\n'
fi

# The other Dolphin surface. The per-file on-device / cloud-only emblems now
# ship too — but as a compiled KF6 overlay plugin, not data, so they have their
# own installer, overlay/install-overlay.sh. That installer also owns the donor
# collision: the old OneDriveForLinux client's overlay plugin reads
# `user.onedrive.syncstate` (this product writes `user.hydration.*`), and now
# that we ship an overlay of our own the two would double-badge, so the overlay
# installer removes it. This servicemenu installer no longer touches that plugin
# — a per-user data install has no business deleting from /usr, and the donor
# alone, without our overlay, draws nothing here anyway.
printf '\nThe on-device / cloud-only emblems ship separately, as a compiled\n'
printf 'plugin. Install them (and clear the donor overlay collision) with:\n'
printf '  %s\n' "$here/overlay/install-overlay.sh"
