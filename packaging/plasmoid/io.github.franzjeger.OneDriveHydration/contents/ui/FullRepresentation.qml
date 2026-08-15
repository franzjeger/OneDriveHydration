/*
    SPDX-FileCopyrightText: 2026 Frank
    SPDX-License-Identifier: MIT OR Apache-2.0
*/

// The flyout page, in the order a person cares: whether the thing works at
// all and what that means for their files; the exposure hazard, which
// outranks everything because nothing else in the product can surface it;
// the work in flight and the cloud-only population; and the two things they
// can do from here — open the folder, and hand a hydrated file's bytes back.
//
// The counters are shown only while the daemon runs. While it is stopped
// they are last-seen values, and the headline's detail already quotes what
// matters from them as history; a table of stale numbers presented as
// current would contradict that honesty.

pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Dialogs
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents3

ColumnLayout {
    id: full

    // The PlasmoidItem in main.qml: state, presentation and the eviction
    // plumbing all live there, because this page is destroyed and rebuilt
    // as the popup opens and closes.
    required property var host

    spacing: Kirigami.Units.largeSpacing

    Layout.minimumWidth: Kirigami.Units.gridUnit * 20
    Layout.preferredWidth: Kirigami.Units.gridUnit * 24
    Layout.minimumHeight: Kirigami.Units.gridUnit * 12
    Layout.preferredHeight: Kirigami.Units.gridUnit * 14

    RowLayout {
        Layout.fillWidth: true
        spacing: Kirigami.Units.largeSpacing

        Kirigami.Icon {
            source: full.host.presentation.icon
            Layout.preferredWidth: Kirigami.Units.iconSizes.medium
            Layout.preferredHeight: Kirigami.Units.iconSizes.medium
            Layout.alignment: Qt.AlignTop
        }

        Kirigami.Heading {
            level: 3
            text: full.host.presentation.headline
            color: full.host.presentation.attention
                ? Kirigami.Theme.negativeTextColor
                : Kirigami.Theme.textColor
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }
    }

    PlasmaComponents3.Label {
        text: full.host.presentation.detail
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        Layout.fillWidth: true
    }

    Kirigami.Separator {
        Layout.fillWidth: true
        visible: counters.visible
    }

    GridLayout {
        id: counters
        visible: full.host.stateKnown && full.host.daemonRunning
        columns: 2
        columnSpacing: Kirigami.Units.largeSpacing
        rowSpacing: Kirigami.Units.smallSpacing
        Layout.fillWidth: true

        PlasmaComponents3.Label {
            text: "Waiting to upload:"
            opacity: 0.75
        }
        PlasmaComponents3.Label {
            text: full.host.count(full.host.unsent, "change", "changes")
        }

        PlasmaComponents3.Label {
            text: "Cloud-only placeholders:"
            opacity: 0.75
        }
        PlasmaComponents3.Label {
            text: full.host.count(full.host.excluded, "file", "files")
        }

        // Only while something is actually coming down. Both cells hide together,
        // and a GridLayout skips invisible items, so the row collapses cleanly
        // when the count is zero rather than leaving a blank line. Informational,
        // not an attention state — it is a plain row like the two above, never a
        // colour or a warning.
        PlasmaComponents3.Label {
            visible: full.host.downloading > 0
            text: "Downloading:"
            opacity: 0.75
        }
        PlasmaComponents3.Label {
            visible: full.host.downloading > 0
            text: full.host.count(full.host.downloading, "file", "files")
        }
    }

    PlasmaComponents3.Label {
        visible: full.host.evictResult !== ""
        text: full.host.evictResult
        color: full.host.evictFailed
            ? Kirigami.Theme.negativeTextColor
            : Kirigami.Theme.positiveTextColor
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        Layout.fillWidth: true
    }

    Item {
        Layout.fillHeight: true
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: Kirigami.Units.smallSpacing

        PlasmaComponents3.Button {
            // The same label as the tray menu's entry, on purpose.
            text: "Open OneDrive Folder"
            icon.name: "folder-open"
            onClicked: {
                full.host.openMount();
                full.host.expanded = false;
            }
        }

        Item {
            Layout.fillWidth: true
        }

        PlasmaComponents3.Button {
            text: full.host.evictBusy ? "Freeing…" : "Free Up Space…"
            icon.name: "folder-cloud"
            // Eviction needs a daemon to answer; the surface would only
            // return DaemonUnavailable otherwise, so say so with a disabled
            // button instead of a failed call.
            enabled: full.host.stateKnown && full.host.daemonRunning && !full.host.evictBusy
            onClicked: evictDialog.open()
        }
    }

    FileDialog {
        id: evictDialog
        title: "Return a file to cloud-only"
        currentFolder: full.host.mountUrl
        fileMode: FileDialog.OpenFile
        onAccepted: full.host.evictFile(selectedFile)
    }
}
