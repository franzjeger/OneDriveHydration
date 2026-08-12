#!/bin/sh
# Install the icon theme fragment for the current user, so StatusNotifier
# hosts can resolve the tray's icon names. User scope on purpose: the tray is
# a per-user process and this needs no privileges; a system package would
# install the same tree under /usr/share/icons instead.
set -eu

here=$(dirname "$(readlink -f "$0")")
dest="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"

for svg in "$here"/hicolor/scalable/*/*.svg; do
    rel=${svg#"$here"/hicolor/}
    install -Dm644 "$svg" "$dest/$rel"
    printf 'installed %s\n' "$dest/$rel"
done

# Icon lookup caches invalidate on the theme directory's mtime; without this
# a session that already resolved a miss keeps the miss until relogin.
touch "$dest"

# The touch only helps processes that start later. A running KDE desktop
# rereads its icon caches when this change signal arrives, and not before —
# measured on Plasma 6.7.4: with the icons installed and the directory
# touched, the panel kept rendering the tray item's fallback icon until the
# signal was emitted. Best effort on purpose: without a session bus there is
# nobody to notify, and the install above is still complete.
if command -v busctl >/dev/null 2>&1 &&
    busctl --user emit /KIconLoader org.kde.KIconLoader iconChanged i 0 2>/dev/null; then
    printf 'notified running KDE processes\n'
else
    printf 'could not notify running processes; they see the icons after relogin\n'
fi
