#!/bin/sh
# Install and verify the complete Plasma/Dolphin surface for the current user.
set -eu
here=$(dirname "$(readlink -f "$0")")
mount=$HOME/OneDrive
bin_dir=/usr/local/bin
check=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --mount) mount=$2; shift 2;;
        --bin-dir) bin_dir=$2; shift 2;;
        --check) check=1; shift;;
        *) printf 'usage: install-desktop.sh [--mount path] [--bin-dir path] [--check]\n' >&2; exit 2;;
    esac
done
plugin_dir=$(qtpaths6 --plugin-dir 2>/dev/null || /usr/lib/qt6/bin/qtpaths --plugin-dir 2>/dev/null || /usr/lib64/qt6/bin/qtpaths --plugin-dir)
data=${XDG_DATA_HOME:-$HOME/.local/share}
config=${XDG_CONFIG_HOME:-$HOME/.config}
if [ "$check" -eq 0 ]; then
    for tool in cmake kdialog qdbus6; do
        command -v "$tool" >/dev/null 2>&1 || { printf 'Missing desktop dependency: %s\n' "$tool" >&2; exit 1; }
    done
    "$here/icons/install-icons.sh"
    "$here/dolphin/install-servicemenu.sh" --mount "$mount" --bin-dir "$bin_dir"
    "$here/dolphin/overlay/install-overlay.sh" --mount "$mount"
    "$here/plasmoid/install-plasmoid.sh"
    # The dynamic plugin scopes menus to OneDrive and supports mixed selections.
    # Remove only our static fallbacks so the same actions do not appear twice.
    rm -f "$data/kio/servicemenus/onedrive-hydration.desktop" "$data/kio/servicemenus/onedrive-hydration-folder.desktop"
    systemctl --user disable --now onedrive-hydration-tray.service 2>/dev/null || true
fi
failed=0
verify() {
    if [ -e "$2" ]; then printf 'OK: %s\n' "$1"
    else printf 'MISSING: %s (%s)\n' "$1" "$2"; failed=1; fi
}
verify 'Control client' "$bin_dir/onedrive-hydrationctl"
verify 'Plasma panel' "$data/plasma/plasmoids/io.github.franzjeger.OneDriveHydration/metadata.json"
verify 'Dolphin status icons' "$plugin_dir/kf6/overlayicon/onedrive-hydration-overlay.so"
verify 'Dolphin context menus' "$plugin_dir/kf6/kfileitemaction/onedrive-hydration-actions.so"
verify 'Dolphin action runner' "$data/onedrive-hydration/selection-action.sh"
verify 'Pinned file icon' "$data/icons/hicolor/scalable/status/onedrive-hydration-pinned.svg"
if ! command -v kdialog >/dev/null 2>&1; then printf 'MISSING: progress and cancellation dialogs (kdialog)\n'; failed=1; fi
if ! test -f "$config/onedrive-hydration/overlay-roots" || ! grep -Fxq -- "$mount" "$config/onedrive-hydration/overlay-roots"; then
    printf 'MISSING: Dolphin sync-root configuration\n'; failed=1
fi
printf 'Dolphin previews read file contents and can download online-only files. Disable Show Previews to avoid this.\n'
printf 'Restart Dolphin after installation; rerun this command with --check to diagnose missing components.\n'
exit "$failed"
