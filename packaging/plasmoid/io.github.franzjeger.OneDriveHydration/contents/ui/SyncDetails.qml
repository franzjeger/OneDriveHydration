// SPDX-License-Identifier: MIT OR Apache-2.0
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PC

ColumnLayout {
    id: details
    required property var host
    property bool showAll: false
    visible: host.desktopAvailable || host.controlResult !== ""
    spacing: Kirigami.Units.smallSpacing

    PC.Label {
        Layout.fillWidth: true
        text: details.host.account.owner && details.host.account.owner.user
            ? details.host.account.owner.user.displayName || details.host.account.name || "OneDrive"
            : details.host.account.name || "OneDrive"
        font.bold: true
        textFormat: Text.PlainText
        elide: Text.ElideRight
    }
    PC.Label {
        Layout.fillWidth: true
        visible: details.host.account.quota !== undefined
        text: details.host.account.quota ? details.host.formatBytes(details.host.account.quota.used)
            + " of " + details.host.formatBytes(details.host.account.quota.total) + " used in OneDrive" : ""
        wrapMode: Text.Wrap
        font: Kirigami.Theme.smallFont
    }
    PC.Label {
        Layout.fillWidth: true
        visible: details.host.account.disk_available !== undefined
        text: details.host.formatBytes(details.host.account.disk_available) + " free on this disk"
        font: Kirigami.Theme.smallFont
    }
    RowLayout {
        Layout.fillWidth: true
        PC.Button {
            objectName: "pauseSyncButton"
            text: details.host.paused ? "Resume sync" : "Pause for 2 hours"
            icon.name: details.host.paused ? "media-playback-start" : "media-playback-pause"
            enabled: details.host.desktopAvailable && !details.host.controlBusy
            onClicked: details.host.setPause(details.host.paused ? 0 : 7200)
        }
        PC.Button {
            text: "Retry pending"
            visible: details.host.queueRows.length > 0
            enabled: !details.host.paused && !details.host.controlBusy
            onClicked: details.host.retryPending()
        }
    }
    PC.Label {
        Layout.fillWidth: true
        visible: details.host.controlResult !== ""
        text: details.host.controlResult
        color: Kirigami.Theme.negativeTextColor
        textFormat: Text.PlainText
        wrapMode: Text.WrapAnywhere
    }
    PC.Label {
        text: "Needs attention"
        font.bold: true
        color: Kirigami.Theme.negativeTextColor
        visible: details.host.syncIssues.length > 0
        Layout.topMargin: Kirigami.Units.smallSpacing
    }
    Repeater {
        model: details.host.syncIssues
        delegate: ColumnLayout {
            id: issueRow
            required property var modelData
            Layout.fillWidth: true
            spacing: 0
            PC.Label {
                Layout.fillWidth: true
                text: issueRow.modelData.path
                textFormat: Text.PlainText
                wrapMode: Text.WrapAnywhere
                font.bold: true
            }
            PC.Label {
                Layout.fillWidth: true
                text: issueRow.modelData.detail
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                font: Kirigami.Theme.smallFont
            }
            PC.Button {
                text: "Review versions…"
                visible: details.host.canResolve && !!issueRow.modelData.path
                enabled: !details.host.controlBusy && !details.host.resolution.running
                onClicked: details.host.reviewConflict(issueRow.modelData.path)
            }
        }
    }
    ConflictDetails { host: details.host; Layout.fillWidth: true }
    PC.Label {
        text: "Waiting to sync"
        font.bold: true
        visible: details.host.queueRows.length > 0
        Layout.topMargin: Kirigami.Units.smallSpacing
    }
    ColumnLayout {
        visible: !!details.host.availabilityJob.operation
        Layout.fillWidth: true
        PC.Label {
            text: (details.host.availabilityJob.operation === "keep" ? "Keep on Device" : "Free Up Space")
                + (details.host.availabilityJob.cancelled ? " · Cancelled" : details.host.availabilityJob.running ? "" : (details.host.availabilityJob.errors || []).length ? " · Needs attention" : " · Finished")
            font.bold: true
        }
        PC.ProgressBar {
            Layout.fillWidth: true
            visible: details.host.availabilityJob.running === true
            indeterminate: !details.host.availabilityJob.total
            from: 0; to: details.host.availabilityJob.total || 1
            value: details.host.availabilityJob.done || 0
        }
        PC.Label {
            Layout.fillWidth: true
            text: (details.host.availabilityJob.done || 0) + " of " + (details.host.availabilityJob.total || 0)
                + " files processed · " + details.host.formatBytes(details.host.availabilityJob.bytes || 0)
                + (details.host.availabilityJob.operation === "keep" ? " made available" : " freed")
            wrapMode: Text.Wrap
        }
        PC.Label {
            Layout.fillWidth: true
            text: details.host.availabilityJob.current || ""
            textFormat: Text.PlainText
            elide: Text.ElideMiddle
        }
        PC.Button {
            text: "Cancel after current file"
            visible: details.host.availabilityJob.running === true
            enabled: !details.host.controlBusy
            onClicked: details.host.cancelJob()
        }
        PC.Label {
            Layout.fillWidth: true
            text: (details.host.availabilityJob.errors || []).join("\n")
            visible: text !== ""
            color: Kirigami.Theme.negativeTextColor
            textFormat: Text.PlainText
            wrapMode: Text.WrapAnywhere
        }
    }
    Repeater {
        model: details.showAll ? details.host.queueRows : details.host.queueRows.slice(0, 10)
        delegate: RowLayout {
            id: row
            required property var modelData
            Layout.fillWidth: true
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0
                PC.Label {
                    text: row.modelData.path || "Looking up filename…"
                    textFormat: Text.PlainText
                    elide: Text.ElideMiddle
                    Layout.fillWidth: true
                }
                PC.Label {
                    text: (row.modelData.detail || (row.modelData.status === "uploading" ? "Uploading" : "Waiting to upload"))
                        + (details.host.paused ? " · Paused" : row.modelData.retry_after > 0 ? " · Next attempt in " + row.modelData.retry_after + " s" : "")
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                    Layout.fillWidth: true
                    font: Kirigami.Theme.smallFont
                    color: row.modelData.status === "retry" ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
                }
            }
            PC.ToolButton {
                icon.name: "document-properties"
                visible: details.host.canResolve && row.modelData.status === "retry"
                enabled: !!row.modelData.path && !details.host.controlBusy && !details.host.resolution.running
                Accessible.name: "Review versions of " + (row.modelData.path || "file")
                onClicked: details.host.reviewConflict(row.modelData.path)
            }
            PC.ToolButton {
                icon.name: "document-open-folder"
                Accessible.name: "Open folder for " + (row.modelData.path || "file")
                enabled: !!row.modelData.path
                onClicked: details.host.openContainingFolder(row.modelData.path)
            }
        }
    }
    PC.Button {
        text: details.showAll ? "Show fewer" : "Show all pending items"
        visible: details.host.queueRows.length > 10
        onClicked: details.showAll = !details.showAll
    }
}
