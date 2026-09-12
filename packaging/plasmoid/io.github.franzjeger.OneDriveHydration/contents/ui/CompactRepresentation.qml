// SPDX-License-Identifier: MIT OR Apache-2.0
pragma ComponentBehavior: Bound
import QtQuick
import org.kde.kirigami as Kirigami

FocusScope {
    id: compact
    required property var plasmoidItem
    activeFocusOnTab: true
    Accessible.role: Accessible.Button
    Accessible.name: plasmoidItem.presentation.headline
    Accessible.description: "Open OneDrive activity"
    Accessible.onPressAction: toggle()
    function toggle() { plasmoidItem.expanded = !plasmoidItem.expanded; }
    Keys.onSpacePressed: toggle()
    Keys.onReturnPressed: toggle()
    Keys.onEnterPressed: toggle()
    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        onClicked: compact.toggle()
    }
    Kirigami.Icon {
        anchors.fill: parent
        source: compact.plasmoidItem.presentation.icon
        active: mouse.containsMouse || compact.activeFocus
    }
}
