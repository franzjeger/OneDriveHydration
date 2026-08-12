/*
    SPDX-FileCopyrightText: 2026 Frank
    SPDX-License-Identifier: MIT OR Apache-2.0
*/

// The tray face: the state icon, and a click that opens or closes the
// flyout. The icon itself is whatever main.qml's presentation chose, so this
// stays a dumb view.

pragma ComponentBehavior: Bound

import QtQuick
import org.kde.kirigami as Kirigami
import org.kde.plasma.plasmoid

MouseArea {
    id: root

    required property PlasmoidItem plasmoidItem

    hoverEnabled: true

    onClicked: {
        root.plasmoidItem.expanded = !root.plasmoidItem.expanded;
    }

    Kirigami.Icon {
        anchors.fill: parent
        source: Plasmoid.icon
        active: root.containsMouse
    }
}
