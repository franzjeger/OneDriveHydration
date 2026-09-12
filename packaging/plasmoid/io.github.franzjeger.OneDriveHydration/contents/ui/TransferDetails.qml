// SPDX-License-Identifier: MIT OR Apache-2.0
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PC

ColumnLayout {
    id: transfers
    required property var host
    visible: host.transferState.count > 0
    spacing: Kirigami.Units.smallSpacing
    PC.Label {
        text: "Transfers"
        font.bold: true
    }
    PC.Label {
        Layout.fillWidth: true
        text: "↑ " + transfers.host.formatBytes(transfers.host.transferState.upload_rate || 0) + "/s   ↓ "
            + transfers.host.formatBytes(transfers.host.transferState.download_rate || 0) + "/s"
        Accessible.name: "Upload and download speed"
    }
    Repeater {
        model: transfers.host.transferState.active || []
        delegate: ColumnLayout {
            id: row
            required property var modelData
            Layout.fillWidth: true
            PC.Label {
                Layout.fillWidth: true
                text: row.modelData.path || "OneDrive file"
                textFormat: Text.PlainText
                elide: Text.ElideMiddle
            }
            PC.ProgressBar {
                objectName: "transferProgress"
                Layout.fillWidth: true
                from: 0
                to: row.modelData.total || 1
                value: row.modelData.position || 0
                indeterminate: !row.modelData.total
            }
            PC.Label {
                Layout.fillWidth: true
                font: Kirigami.Theme.smallFont
                text: (row.modelData.direction === "upload" ? "Sending" : row.modelData.total < row.modelData.object_size ? "Downloading part of this file" : "Downloading")
                    + " · " + transfers.host.formatBytes(row.modelData.position || 0) + " of " + transfers.host.formatBytes(row.modelData.total)
                    + (row.modelData.direction === "upload" && row.modelData.position >= row.modelData.total ? " · Waiting for confirmation" : "")
                wrapMode: Text.Wrap
            }
        }
    }
}
