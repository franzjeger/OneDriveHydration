#!/bin/sh
# Install the flyout plasmoid for the current user, so plasmashell's system
# tray can load it. User scope on purpose: the flyout watches a per-user
# session bus and this needs no privileges; a system package would install
# the same tree under /usr/share/plasma/plasmoids instead.
#
# The icons are a prerequisite, not a bundled asset: the plasmoid names the
# same hicolor icons the tray binary names, and ../icons/install-icons.sh
# installs them. Without them plasmashell renders a generic fallback.
set -eu

here=$(dirname "$(readlink -f "$0")")
pkg="$here/io.github.franzjeger.OneDriveHydration"
id="io.github.franzjeger.OneDriveHydration"
unit=onedrive-hydration-tray.service

# kpackagetool6 refuses --install over an existing package and --upgrade
# without one, so ask which case this is instead of guessing from the error.
if kpackagetool6 --type Plasma/Applet --show "$id" >/dev/null 2>&1; then
    mode=upgrade
    kpackagetool6 --type Plasma/Applet --upgrade "$pkg"
else
    mode=install
    kpackagetool6 --type Plasma/Applet --install "$pkg"
fi

# Say what this run did, not what the two cases do in general.
#
# Measured on Plasma 6.7.4: a running plasmashell appended a freshly
# *installed* applet to the system tray's knownItems and extraItems and
# instantiated it within seconds of kpackagetool6 finishing, with no restart —
# this script had originally guessed the opposite and said so. An upgrade is
# the other case: the QML already loaded stays loaded, so a running tray keeps
# executing the old version until plasmashell restarts.
#
# The branch above already knows which of the two happened, so printing both
# sentences would tell every first-time installer to consider restarting their
# shell for nothing. A restart instruction nobody needed is a small lie about
# what was measured.
if [ "$mode" = install ]; then
    printf 'installed. The running system tray adopts it by itself; no restart.\n'
else
    printf 'upgraded. The QML already loaded keeps running until the shell reloads:\n'
    printf '  systemctl --user restart plasma-plasmashell.service\n'
    printf 'That redraws the desktop and panels; windows are unaffected.\n'
fi

# The other tray surface, checked from the one place it can actually be seen.
# `onedrive-hydration-install` refuses this collision when it can see the applet
# on disk, but it runs as root and often before any session exists, so a running
# unit is not a fact it has. Here we are the user, inside the session, and can
# ask systemd directly.
#
# Reported, never acted on: which surface a deployment uses is the operator's
# decision, and installing an applet is not authority to stop somebody's
# service.
enabled=$(systemctl --user is-enabled "$unit" 2>/dev/null || true)
active=$(systemctl --user is-active "$unit" 2>/dev/null || true)
case "$enabled:$active" in
    enabled*|*:active|*:activating)
        printf '\nwarning: %s is %s (%s) in this session.\n' "$unit" "$enabled" "$active"
        printf 'It draws a StatusNotifierItem and this applet is a tray entry in its\n'
        printf 'own right, so plasmashell will show two identical icons. On Plasma the\n'
        printf 'applet is the better surface — it carries this flyout, which the binary\n'
        printf 'cannot draw. To retire the binary here:\n'
        printf '  systemctl --user disable --now %s\n' "$unit"
        printf 'and re-run onedrive-hydration-install with --tray plasmoid, or the next\n'
        printf 'install writes the unit back.\n'
        ;;
esac
