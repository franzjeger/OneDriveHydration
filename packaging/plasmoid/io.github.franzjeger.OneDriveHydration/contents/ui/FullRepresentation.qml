// SPDX-License-Identifier: MIT OR Apache-2.0
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Dialogs
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents3

ColumnLayout {
    id: full
    required property var host
    property bool detailsExpanded: false
    property bool selectionExpanded: false
    spacing: 0

    Layout.minimumWidth: Kirigami.Units.gridUnit * 20
    Layout.preferredWidth: Kirigami.Units.gridUnit * 24
    Layout.minimumHeight: Kirigami.Units.gridUnit * 16
    Layout.preferredHeight: Kirigami.Units.gridUnit * 20

    RowLayout {
        Layout.fillWidth: true
        Layout.margins: Kirigami.Units.largeSpacing
        spacing: Kirigami.Units.largeSpacing
        Kirigami.Icon {
            source: "onedrive-hydration"
            Layout.preferredWidth: Kirigami.Units.iconSizes.smallMedium
            Layout.preferredHeight: Kirigami.Units.iconSizes.smallMedium
        }
        Kirigami.Heading { text: "OneDrive"; level: 3; Layout.fillWidth: true }
        PlasmaComponents3.ToolButton {
            icon.name: "configure"
            Accessible.name: "OneDrive settings"
            PlasmaComponents3.ToolTip.text: "OneDrive settings"
            PlasmaComponents3.ToolTip.visible: hovered
            onClicked: full.host.configure()
        }
    }

    Kirigami.Separator { Layout.fillWidth: true }

    // The header and actions stay reachable at every popup size. Messages,
    // transfers and the bounded activity list share one scrolling area.
    PlasmaComponents3.ScrollView {
        id: scroll
        objectName: "activityScroll"
        Layout.fillWidth: true
        Layout.fillHeight: true
        clip: true
        contentWidth: availableWidth
        PlasmaComponents3.ScrollBar.horizontal.policy: PlasmaComponents3.ScrollBar.AlwaysOff

        ColumnLayout {
            width: scroll.availableWidth
            spacing: Kirigami.Units.largeSpacing

            RowLayout {
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.largeSpacing
                Layout.topMargin: Kirigami.Units.largeSpacing * 2
                spacing: Kirigami.Units.largeSpacing
                Kirigami.Icon {
                    source: full.host.presentation.icon
                    Layout.preferredWidth: Kirigami.Units.iconSizes.medium
                    Layout.preferredHeight: Kirigami.Units.iconSizes.medium
                    Layout.alignment: Qt.AlignTop
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing
                    Kirigami.Heading {
                        objectName: "statusHeadline"
                        text: full.host.presentation.headline
                        textFormat: Text.PlainText
                        level: 2
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                        color: full.host.presentation.attention ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
                    }
                    PlasmaComponents3.Label {
                        text: full.host.presentation.detail
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }
                    PlasmaComponents3.Button {
                        objectName: "signInButton"
                        visible: full.host.activityVisible && full.host.credentialState === "rejected"
                        enabled: !full.host.enrollmentBusy
                        text: full.host.enrollmentBusy ? "Waiting for browser…" : "Sign in"
                        icon.name: "system-log-in"
                        onClicked: full.host.beginEnrollment()
                    }
                }
            }

            PlasmaComponents3.ProgressBar {
                objectName: "workIndicator"
                Layout.fillWidth: true
                Layout.leftMargin: Kirigami.Units.largeSpacing
                Layout.rightMargin: Kirigami.Units.largeSpacing
                visible: full.host.working
                indeterminate: true
                Accessible.name: "OneDrive is working"
            }

            // Pending is a count of changes, including active uploads. It is
            // not a byte total or a promise that a transfer is progressing.
            PlasmaComponents3.Label {
                visible: full.host.activityVisible && full.host.unsent > 0 && full.host.working
                text: full.host.count(full.host.unsent, "local change", "local changes") + " still to upload"
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                Layout.fillWidth: true
                Layout.leftMargin: Kirigami.Units.largeSpacing
                Layout.rightMargin: Kirigami.Units.largeSpacing
                color: Kirigami.Theme.textColor; opacity: 0.7
            }

            Repeater {
                model: full.host.activityVisible && !(full.host.transferState.count > 0) ? full.host.activeUploads : []
                delegate: RowLayout {
                    id: transfer
                    required property string modelData
                    Layout.fillWidth: true
                    Layout.leftMargin: Kirigami.Units.largeSpacing
                    Layout.rightMargin: Kirigami.Units.largeSpacing
                    spacing: Kirigami.Units.largeSpacing
                    Kirigami.Icon {
                        source: "document-send"
                        Layout.preferredWidth: Kirigami.Units.iconSizes.smallMedium
                        Layout.preferredHeight: Kirigami.Units.iconSizes.smallMedium
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 0
                        PlasmaComponents3.Label {
                            text: transfer.modelData.split("/").pop()
                            textFormat: Text.PlainText
                            elide: Text.ElideMiddle
                            Layout.fillWidth: true
                        }
                        PlasmaComponents3.Label {
                            text: "Uploading"
                            color: Kirigami.Theme.textColor; opacity: 0.7
                            font: Kirigami.Theme.smallFont
                        }
                    }
                    PlasmaComponents3.ToolButton {
                        icon.name: "document-open-folder"
                        Accessible.name: "Open folder for " + transfer.modelData
                        PlasmaComponents3.ToolTip.text: transfer.modelData
                        PlasmaComponents3.ToolTip.visible: hovered
                        onClicked: full.host.openContainingFolder(transfer.modelData)
                    }
                }
            }

            TransferDetails {
                host: full.host
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.largeSpacing
            }
            Loader {
                active: full.selectionExpanded
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.largeSpacing
                sourceComponent: FolderSelection { host: full.host }
            }
            SyncDetails {
                host: full.host
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.largeSpacing
            }

            RowLayout {
                visible: full.host.enrollmentResult !== ""
                Layout.fillWidth: true
                Layout.leftMargin: Kirigami.Units.largeSpacing
                Layout.rightMargin: Kirigami.Units.largeSpacing
                PlasmaComponents3.Label {
                    text: full.host.enrollmentResult
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                    Layout.fillWidth: true
                    color: full.host.enrollmentFailed ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
                }
                PlasmaComponents3.ToolButton {
                    visible: !full.host.enrollmentBusy
                    icon.name: "dialog-close"
                    Accessible.name: "Dismiss sign-in message"
                    onClicked: full.host.enrollmentResult = ""
                }
            }

            RowLayout {
                visible: full.host.evictResult !== ""
                Layout.fillWidth: true
                Layout.leftMargin: Kirigami.Units.largeSpacing
                Layout.rightMargin: Kirigami.Units.largeSpacing
                PlasmaComponents3.Label {
                    text: full.host.evictResult
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                    Layout.fillWidth: true
                    color: full.host.evictFailed ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.positiveTextColor
                }
                PlasmaComponents3.ToolButton {
                    icon.name: "dialog-close"
                    Accessible.name: "Dismiss space message"
                    onClicked: full.host.evictResult = ""
                }
            }

            Kirigami.Separator { Layout.fillWidth: true }
            RowLayout {
                Layout.fillWidth: true
                Layout.leftMargin: Kirigami.Units.largeSpacing
                Layout.rightMargin: Kirigami.Units.largeSpacing
                PlasmaComponents3.Label { text: "Recent activity"; font.bold: true; Layout.fillWidth: true }
                PlasmaComponents3.Label { text: full.host.desktopAvailable ? "Saved history" : "This session"; font: Kirigami.Theme.smallFont; color: Kirigami.Theme.textColor; opacity: 0.7 }
            }
            PlasmaComponents3.Label {
                visible: full.host.recentActivity.length === 0
                text: "Activity appears here when uploads start or you free up space."
                wrapMode: Text.Wrap
                Layout.fillWidth: true
                Layout.leftMargin: Kirigami.Units.largeSpacing
                Layout.rightMargin: Kirigami.Units.largeSpacing
                color: Kirigami.Theme.textColor; opacity: 0.7
            }
            Repeater {
                model: full.host.recentActivity
                delegate: RowLayout {
                    id: event
                    required property var modelData
                    Layout.fillWidth: true
                    Layout.leftMargin: Kirigami.Units.largeSpacing
                    Layout.rightMargin: Kirigami.Units.largeSpacing
                    spacing: Kirigami.Units.largeSpacing
                    Kirigami.Icon {
                        source: event.modelData.icon
                        Layout.preferredWidth: Kirigami.Units.iconSizes.smallMedium
                        Layout.preferredHeight: Kirigami.Units.iconSizes.smallMedium
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 0
                        PlasmaComponents3.Label {
                            text: event.modelData.title
                            textFormat: Text.PlainText
                            elide: Text.ElideMiddle
                            Layout.fillWidth: true
                        }
                        PlasmaComponents3.Label {
                            text: event.modelData.detail + " · " + Qt.formatTime(event.modelData.time, Qt.locale().timeFormat(Locale.ShortFormat))
                            textFormat: Text.PlainText
                            wrapMode: Text.Wrap
                            font: Kirigami.Theme.smallFont
                            color: Kirigami.Theme.textColor; opacity: 0.7
                            Layout.fillWidth: true
                        }
                    }
                    PlasmaComponents3.ToolButton {
                        visible: event.modelData.path !== ""
                        icon.name: "document-open-folder"
                        Accessible.name: "Open folder for " + event.modelData.title
                        PlasmaComponents3.ToolTip.text: event.modelData.path
                        PlasmaComponents3.ToolTip.visible: hovered
                        onClicked: full.host.openContainingFolder(event.modelData.path)
                    }
                }
            }

            PlasmaComponents3.ToolButton {
                text: full.detailsExpanded ? "Hide sync details" : "Sync details"
                icon.name: full.detailsExpanded ? "arrow-up" : "arrow-down"
                Layout.leftMargin: Kirigami.Units.smallSpacing
                onClicked: full.detailsExpanded = !full.detailsExpanded
            }
            GridLayout {
                visible: full.detailsExpanded
                columns: 2
                columnSpacing: Kirigami.Units.largeSpacing
                rowSpacing: Kirigami.Units.smallSpacing
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.largeSpacing
                Layout.topMargin: 0
                PlasmaComponents3.Label { text: "Online-only files" }
                PlasmaComponents3.Label { text: full.host.activityVisible ? Number(full.host.excluded).toLocaleString(Qt.locale(), 'f', 0) : "Unavailable" }
                PlasmaComponents3.Label { text: "Changes to upload" }
                PlasmaComponents3.Label { text: full.host.activityVisible ? Number(full.host.unsent).toLocaleString(Qt.locale(), 'f', 0) : "Unavailable" }
                PlasmaComponents3.Label { text: "Extra mounts" }
                PlasmaComponents3.Label { text: full.host.activityVisible ? full.host.exposures : "Unavailable" }
                PlasmaComponents3.Label { text: "OneDrive folder" }
                PlasmaComponents3.Label { text: full.host.mountPath; textFormat: Text.PlainText; wrapMode: Text.WrapAnywhere; Layout.fillWidth: true }
            }
            Item { implicitHeight: Kirigami.Units.smallSpacing }
        }
    }

    Kirigami.Separator { Layout.fillWidth: true }
    RowLayout {
        objectName: "footer"
        Layout.fillWidth: true
        Layout.margins: Kirigami.Units.largeSpacing
        PlasmaComponents3.Button {
            text: "Open OneDrive Folder"
            icon.name: "folder-open"
            onClicked: { full.host.openMount(); full.host.expanded = false; }
        }
        Item { Layout.fillWidth: true }
        PlasmaComponents3.ToolButton {
            id: moreButton
            icon.name: "view-more-symbolic"
            Accessible.name: "More OneDrive actions"
            PlasmaComponents3.ToolTip.text: "More actions"
            PlasmaComponents3.ToolTip.visible: hovered
            onClicked: moreMenu.popup()
            PlasmaComponents3.Menu {
                id: moreMenu
                y: -implicitHeight
                PlasmaComponents3.MenuItem {
                    text: full.host.evictBusy ? "Freeing space…" : "Free Up Space…"
                    icon.name: "folder-cloud"
                    enabled: full.host.activityVisible && !full.host.evictBusy
                    onTriggered: evictDialog.open()
                }
                PlasmaComponents3.MenuItem {
                    text: "Keep a folder on this device…"
                    icon.name: "onedrive-hydration-pinned"
                    enabled: full.host.desktopAvailable && !full.host.availabilityJob.running
                    onTriggered: offlineFolderDialog.open()
                }
                PlasmaComponents3.MenuItem {
                    text: full.selectionExpanded ? "Hide folder selection" : "Choose folders to sync…"
                    icon.name: "folder-sync"
                    enabled: full.host.desktopAvailable
                    onTriggered: full.selectionExpanded = !full.selectionExpanded
                }
                PlasmaComponents3.MenuItem {
                    text: "Open OneDrive on the web"
                    icon.name: "internet-services"
                    onTriggered: full.host.openWeb()
                }
            }
        }
    }

    FileDialog {
        id: evictDialog
        title: "Free up space"
        currentFolder: full.host.mountUrl
        fileMode: FileDialog.OpenFile
        onAccepted: full.host.evictFile(selectedFile)
    }
    FolderDialog {
        id: offlineFolderDialog
        title: "Choose a OneDrive folder to keep on this device"
        currentFolder: full.host.mountUrl
        onAccepted: full.host.keepFolder(selectedFolder)
    }
}
