// SPDX-License-Identifier: MIT OR Apache-2.0
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Dialogs
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PC

ColumnLayout {
    id: selection
    required property var host
    property var draft: []
    property string validation: ""
    spacing: Kirigami.Units.smallSpacing
    Component.onCompleted: draft = host.excludedFolders.slice()
    Connections {
        target: selection.host
        function onExcludedFoldersChanged() { selection.draft = selection.host.excludedFolders.slice(); }
    }
    PC.Label { text: "Folders synced on this device"; font.bold: true }
    PC.Label {
        Layout.fillWidth: true
        text: "All folders sync unless listed below. Excluded folders keep their existing files, but their changes stop syncing. Opening an online-only file can still download it."
        wrapMode: Text.Wrap
    }
    Repeater {
        model: selection.draft
        delegate: RowLayout {
            id: folder
            required property string modelData
            required property int index
            Layout.fillWidth: true
            PC.Label {
                Layout.fillWidth: true
                text: folder.modelData
                textFormat: Text.PlainText
                wrapMode: Text.WrapAnywhere
            }
            PC.ToolButton {
                icon.name: "list-remove"
                Accessible.name: "Sync " + folder.modelData + " again"
                onClicked: { let paths = selection.draft.slice(); paths.splice(folder.index, 1); selection.draft = paths; }
            }
        }
    }
    PC.Label {
        text: "All folders selected"
        visible: selection.draft.length === 0
        opacity: 0.7
    }
    RowLayout {
        PC.Button { text: "Exclude a folder…"; onClicked: picker.open(); enabled: !selection.host.controlBusy }
        PC.Button {
            objectName: "applyFolderSelection"
            text: selection.host.controlBusy ? "Applying…" : "Apply"
            enabled: !selection.host.controlBusy && JSON.stringify(selection.draft) !== JSON.stringify(selection.host.excludedFolders)
            onClicked: selection.host.setFolderSelection(selection.draft)
        }
    }
    PC.Label {
        text: selection.validation
        visible: text !== ""
        Layout.fillWidth: true
        wrapMode: Text.Wrap
        color: Kirigami.Theme.negativeTextColor
    }
    FolderDialog {
        id: picker
        title: "Choose a folder to exclude from sync"
        currentFolder: selection.host.mountUrl
        onAccepted: {
            const url = selectedFolder.toString();
            const path = decodeURIComponent(url.replace(/^file:\/\//, ""));
            if (!url.startsWith("file://") || !path.startsWith(selection.host.mountPath + "/")) {
                selection.validation = "Choose a folder inside OneDrive."; return;
            }
            const relative = path.slice(selection.host.mountPath.length + 1);
            if (!selection.draft.includes(relative)) selection.draft = selection.draft.concat([relative]).sort();
            selection.validation = "";
        }
    }
}
