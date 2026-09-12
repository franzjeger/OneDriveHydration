// SPDX-License-Identifier: MIT OR Apache-2.0
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PC

ColumnLayout {
    id: conflict
    required property var host
    property string choice: ""
    Connections {
        target: conflict.host
        function onConflictReviewChanged() { conflict.choice = ""; }
    }
    visible: !!host.conflictReview.token || !!host.resolution.path
    spacing: Kirigami.Units.smallSpacing
    ColumnLayout {
        Layout.fillWidth: true
        visible: !!conflict.host.conflictReview.token
        PC.Label { text: "Review versions"; font.bold: true }
        PC.Label {
            Layout.fillWidth: true
            text: conflict.host.conflictReview.path || ""
            textFormat: Text.PlainText
            wrapMode: Text.WrapAnywhere
        }
        PC.Label {
            Layout.fillWidth: true
            text: "Local: " + conflict.host.formatBytes(conflict.host.conflictReview.local_size)
                + (conflict.host.conflictReview.local_complete ? " · Available on this device" : " · Incomplete local data")
                + "\nOneDrive: " + conflict.host.formatBytes(conflict.host.conflictReview.cloud_size)
                + (conflict.host.conflictReview.cloud_modified ? " · " + new Date(conflict.host.conflictReview.cloud_modified).toLocaleString(Qt.locale(), Locale.ShortFormat) : "")
            wrapMode: Text.Wrap
        }
        PC.Label {
            Layout.fillWidth: true
            text: "Recovery copies are kept before anything is replaced. Versions are checked again before applying your choice."
            wrapMode: Text.Wrap
        }
        Flow {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing
            Repeater {
                model: conflict.host.conflictReview.choices || []
                delegate: PC.Button {
                    required property string modelData
                    objectName: "conflictChoice_" + modelData
                    text: modelData === "both" ? "Keep both" : modelData === "local" ? "Use local version" : "Use cloud version"
                    checkable: true
                    checked: conflict.choice === modelData
                    onClicked: conflict.choice = modelData
                }
            }
        }
        PC.Label {
            Layout.fillWidth: true
            visible: conflict.choice !== ""
            text: conflict.choice === "both" ? "Upload the local version as a separate file and use the cloud version at the original name."
                : conflict.choice === "local" ? "Replace the reviewed cloud version with the local version. Both versions are saved in recovery."
                : "Replace the local file with the reviewed cloud version, available online-only. Existing local data remains in recovery."
            wrapMode: Text.Wrap
        }
        RowLayout {
            PC.Button {
                objectName: "confirmConflictChoice"
                text: "Confirm choice"
                enabled: conflict.choice !== "" && !conflict.host.controlBusy && !conflict.host.resolution.running
                onClicked: conflict.host.resolveConflict(conflict.host.conflictReview.token, conflict.choice)
            }
            PC.Button {
                text: "Open cloud version"
                visible: !!conflict.host.conflictReview.cloud_url
                onClicked: conflict.host.openCloudVersion(conflict.host.conflictReview.cloud_url)
            }
        }
    }
    ColumnLayout {
        Layout.fillWidth: true
        visible: !!conflict.host.resolution.path
        PC.Label {
            text: conflict.host.resolution.running ? "Resolving conflict…" : conflict.host.resolution.success ? "Conflict resolved" : "Resolution needs attention"
            font.bold: true
        }
        PC.BusyIndicator { visible: conflict.host.resolution.running === true; running: visible }
        PC.Label {
            Layout.fillWidth: true
            text: conflict.host.resolution.message || conflict.host.resolution.path || ""
            textFormat: Text.PlainText
            wrapMode: Text.WrapAnywhere
        }
        PC.Button {
            text: "Open recovery folder"
            visible: !!conflict.host.resolution.recovery_dir
            onClicked: conflict.host.openRecovery(conflict.host.resolution.recovery_dir)
        }
    }
}
