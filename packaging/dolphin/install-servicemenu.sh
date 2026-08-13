#!/bin/sh
# Install the Dolphin "Free Up Space" action for the current user. User scope
# on purpose, like the plasmoid: this needs no privileges, and a system package
# would install the same two files under /usr/share instead.
#
# The action is data — a KIO servicemenu .desktop plus a small wrapper — so
# there is no toolkit and no new dependency, the same trade the tray and the
# flyout made. The overlay emblems in the roadmap's other half cannot be done
# this way; see docs/DOLPHIN-GROUNDWORK.md for why, and what it would cost.
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

Installs two files under \$XDG_DATA_HOME (default ~/.local/share):
  kio/servicemenus/onedrive-hydration.desktop
  onedrive-hydration/free-up-space.sh
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
mkdir -p "$action_dir" "$menu_dir"

sed -e "s|@MOUNT@|$mount|g" -e "s|@CTL@|$bin_dir/onedrive-hydrationctl|g" \
    "$here/free-up-space.sh.in" > "$action.tmp"
chmod 755 "$action.tmp"
mv -f "$action.tmp" "$action"

sed -e "s|@ACTION@|$action|g" \
    "$here/servicemenu.desktop.in" > "$menu_dir/onedrive-hydration.desktop.tmp"
mv -f "$menu_dir/onedrive-hydration.desktop.tmp" "$menu_dir/onedrive-hydration.desktop"

printf 'installed:\n  %s\n  %s/onedrive-hydration.desktop\n' "$action" "$menu_dir"
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

# The other Dolphin surface, reported because it is invisible from inside
# Dolphin and belongs to nobody. A KOverlayIconPlugin from the donor client can
# still be installed system-wide, reading an xattr this product does not write:
# `user.onedrive.syncstate`, where the deployment writes `user.hydration.*`.
# Measured on the live mount: 0 of 400 files carried the old name.
#
# Reported and never removed. It is a root-owned file outside this user's
# scope, and a per-user script that deleted from /usr would be doing something
# nobody asked it to.
found=''
for dir in /usr/lib/qt6/plugins/kf6/overlayicon /usr/lib64/qt6/plugins/kf6/overlayicon \
    "$data_home/../lib/qt6/plugins/kf6/overlayicon"; do
    [ -d "$dir" ] || continue
    real=$(readlink -f -- "$dir" 2>/dev/null) || continue
    case " $found " in
        *" $real "*) continue ;;
    esac
    found="$found $real"
    for so in "$real"/*onedrive*.so; do
        [ -e "$so" ] || continue
        printf '\nnote: a OneDrive Dolphin overlay plugin is installed system-wide:\n'
        printf '  %s\n' "$so"
        printf 'This product does not ship one and does not manage that file. If it is\n'
        printf 'the donor client'"'"'s plugin it reads user.onedrive.syncstate, which this\n'
        printf 'deployment never writes — it writes user.hydration.* — so it draws no\n'
        printf 'emblems here. Check with:\n'
        printf '  getfattr -d -m user %s/<some file>\n' "$mount"
        printf 'and if it is dead weight, remove it yourself:\n'
        printf '  sudo rm %s\n' "$so"
    done
done
