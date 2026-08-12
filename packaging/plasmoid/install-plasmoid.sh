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

# kpackagetool6 refuses --install over an existing package and --upgrade
# without one, so ask which case this is instead of guessing from the error.
if kpackagetool6 --type Plasma/Applet --show "$id" >/dev/null 2>&1; then
    kpackagetool6 --type Plasma/Applet --upgrade "$pkg"
else
    kpackagetool6 --type Plasma/Applet --install "$pkg"
fi

# A first install needs no further step — measured on Plasma 6.7.4: the
# running plasmashell appended the applet to the system tray's knownItems
# and extraItems and instantiated it within seconds of kpackagetool6
# finishing, no restart involved. An *upgrade* is different: the QML that is
# already loaded stays loaded, so a running tray keeps executing the old
# version until plasmashell restarts. Restarting plasmashell only redraws
# the desktop and panels; windows are unaffected.
printf 'installed. A first install appears in the system tray by itself;\n'
printf 'after an upgrade, reload the running shell with:\n'
printf '  systemctl --user restart plasma-plasmashell.service\n'
