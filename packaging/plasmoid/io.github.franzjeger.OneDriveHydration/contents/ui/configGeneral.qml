/*
    SPDX-FileCopyrightText: 2026 Frank
    SPDX-License-Identifier: MIT OR Apache-2.0
*/

pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Dialogs
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

ColumnLayout {
    id: page

    property alias cfg_mountPath: mountField.text
    property string cfg_mountPathDefault: ""

    Kirigami.FormLayout {
        Layout.fillWidth: true

        RowLayout {
            Kirigami.FormData.label: "OneDrive folder:"

            QQC2.TextField {
                id: mountField
                placeholderText: "~/OneDrive"
                Layout.fillWidth: true
            }

            QQC2.Button {
                icon.name: "document-open-folder"
                text: "Choose…"
                onClicked: folderDialog.open()
            }
        }

        QQC2.Label {
            text: "Must match the daemon's --mount path. Leave empty for ~/OneDrive."
            font: Kirigami.Theme.smallFont
            opacity: 0.75
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }
    }

    Item {
        Layout.fillHeight: true
    }

    FolderDialog {
        id: folderDialog
        onAccepted: {
            mountField.text = decodeURIComponent(
                selectedFolder.toString().replace(/^file:\/\//, ""));
        }
    }
}
