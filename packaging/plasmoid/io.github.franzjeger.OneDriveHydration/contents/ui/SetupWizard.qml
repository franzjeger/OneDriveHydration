// SPDX-License-Identifier: MIT OR Apache-2.0
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import org.kde.plasma.workspace.dbus as DBus

Kirigami.ApplicationWindow {
    id: root
    title: "OneDrive Setup"
    width: 600
    height: 460
    visible: true

    readonly property string busName: "io.github.franzjeger.OneDriveHydration"
    readonly property string objectPath: "/io/github/franzjeger/OneDriveHydration"

    property bool autoFetchOnStart: true
    property string credentialState: "unknown"
    property var cloudFolders: []
    property var excludedSet: ({})
    property bool loading: false
    property bool saving: false
    property bool enrollmentBusy: false
    property bool enrollmentPrepared: false
    property bool enrollmentQueryPending: false
    property bool folderSelectionPushed: false
    property string statusMessage: ""
    property string errorMessage: ""

    // Seams overridable in qmltestrunner tests so every branch runs deterministically
    // without touching a live session D-Bus or spawning a real browser.
    property var callDbus: function(spec, onSuccess, onError) {
        DBus.SessionBus.asyncCall(spec, onSuccess, onError);
    }
    property var openExternalUrl: function(url) {
        return Qt.openUrlExternally(url);
    }
    property var closeWindow: function() {
        root.close();
        Qt.quit();
    }

    function unwrapValue(v) {
        let cur = v;
        while (cur !== null && typeof cur === "object" && !Array.isArray(cur) && "value" in cur) {
            cur = cur.value;
        }
        return cur;
    }

    function errorText(err) {
        if (!err) return "Request failed.";
        if (err.error && err.error.message) return err.error.message;
        if (err.message) return err.message;
        return String(err);
    }

    function isSignedInWord(word) {
        return word === "healthy" || word === "unsaved";
    }

    function toggleExcluded(folderName, checked) {
        const clean = String(folderName || "").replace(/^\/+/, "");
        if (!clean) return;
        const next = Object.assign({}, root.excludedSet);
        if (checked) {
            next[clean] = true;
        } else {
            delete next[clean];
        }
        root.excludedSet = next;
    }

    function isFolderExcluded(folderName) {
        const clean = String(folderName || "").replace(/^\/+/, "");
        return root.excludedSet[clean] === true;
    }

    function selectedExclusions() {
        return Object.keys(root.excludedSet)
            .filter(function(k) { return root.excludedSet[k] === true; })
            .map(function(k) { return k.replace(/^\/+/, ""); })
            .filter(function(k) { return k.length > 0; })
            .sort();
    }

    function applyDesktopSelection(desktopJson) {
        const text = unwrapValue(desktopJson);
        if (typeof text !== "string" || text === "") return;
        try {
            const parsed = JSON.parse(text);
            if (Array.isArray(parsed.selection)) {
                const next = {};
                for (let i = 0; i < parsed.selection.length; i++) {
                    const clean = String(parsed.selection[i] || "").replace(/^\/+/, "");
                    if (clean) next[clean] = true;
                }
                root.excludedSet = next;
            }
        } catch (_) {}
    }

    function applyCredentialState(rawState) {
        const word = unwrapValue(rawState);
        root.credentialState = (typeof word === "string" && word.length > 0) ? word : "unknown";
        if (isSignedInWord(root.credentialState) && !root.folderSelectionPushed) {
            root.folderSelectionPushed = true;
            root.enrollmentBusy = false;
            root.enrollmentPrepared = false;
            root.errorMessage = "";
            root.pageStack.push(folderSelectionPage);
            root.loadCloudFolders("");
        }
    }

    DBus.DBusServiceWatcher {
        busType: DBus.BusType.Session
        watchedService: root.busName
        onRegisteredChanged: {
            if (registered && root.autoFetchOnStart) {
                root.fetchState();
            } else if (!registered) {
                root.credentialState = "unknown";
            }
        }
    }

    DBus.SignalWatcher {
        busType: DBus.BusType.Session
        service: root.busName
        path: root.objectPath
        iface: root.busName
        function dbusCredentialStateChanged(state) {
            root.applyCredentialState(state);
        }
    }

    Timer {
        interval: 1000
        repeat: true
        running: root.enrollmentBusy && root.enrollmentPrepared
        onTriggered: root.pollEnrollmentStatus()
    }

    Component.onCompleted: {
        if (root.autoFetchOnStart) {
            root.fetchState();
        }
    }

    function fetchState() {
        root.callDbus({
            service: root.busName,
            path: root.objectPath,
            iface: "org.freedesktop.DBus.Properties",
            member: "GetAll",
            arguments: [root.busName]
        }, function(reply) {
            const props = unwrapValue(reply) || {};
            root.applyDesktopSelection(props.DesktopState);
            root.applyCredentialState(props.CredentialState);
        }, function() {});
    }

    function beginEnrollment() {
        if (root.enrollmentBusy) return;
        root.enrollmentBusy = true;
        root.enrollmentPrepared = false;
        root.errorMessage = "";
        root.statusMessage = "Preparing secure browser sign-in…";
        root.callDbus({
            service: root.busName,
            path: root.objectPath,
            iface: root.busName,
            member: "BeginEnrollment",
            arguments: []
        }, function(reply) {
            const url = String(unwrapValue(reply) || "");
            root.enrollmentPrepared = true;
            root.statusMessage = "Finish signing in in your browser.";
            if (!url.startsWith("https://") || !root.openExternalUrl(url)) {
                root.errorMessage = "Could not open browser automatically. Open this URL manually: " + url;
            }
        }, function(err) {
            root.enrollmentBusy = false;
            root.statusMessage = "";
            root.errorMessage = root.errorText(err);
        });
    }

    function pollEnrollmentStatus() {
        if (!root.enrollmentBusy || !root.enrollmentPrepared || root.enrollmentQueryPending) return;
        root.enrollmentQueryPending = true;
        root.callDbus({
            service: root.busName,
            path: root.objectPath,
            iface: root.busName,
            member: "EnrollmentStatus",
            arguments: []
        }, function(reply) {
            root.enrollmentQueryPending = false;
            const status = String(unwrapValue(reply) || "");
            if (status === "complete") {
                root.enrollmentBusy = false;
                root.enrollmentPrepared = false;
                root.statusMessage = "Sign-in completed.";
                root.applyCredentialState("healthy");
            } else if (status.startsWith("error:")) {
                root.enrollmentBusy = false;
                root.enrollmentPrepared = false;
                root.statusMessage = "";
                root.errorMessage = status.slice("error:".length).trim();
            }
        }, function(err) {
            root.enrollmentQueryPending = false;
            root.enrollmentBusy = false;
            root.enrollmentPrepared = false;
            root.statusMessage = "";
            root.errorMessage = root.errorText(err);
        });
    }

    function loadCloudFolders(path) {
        root.loading = true;
        root.errorMessage = "";
        root.callDbus({
            service: root.busName,
            path: root.objectPath,
            iface: root.busName,
            member: "ListCloudFolders",
            arguments: [path]
        }, function(reply) {
            root.loading = false;
            const folders = unwrapValue(reply);
            root.cloudFolders = Array.isArray(folders) ? folders : [];
        }, function(err) {
            root.loading = false;
            root.errorMessage = "Could not load folders: " + root.errorText(err);
        });
    }

    function finishSetup() {
        if (root.saving) return;
        root.saving = true;
        root.errorMessage = "";
        const excluded = root.selectedExclusions();
        root.callDbus({
            service: root.busName,
            path: root.objectPath,
            iface: root.busName,
            member: "SetFolderSelection",
            arguments: [excluded]
        }, function() {
            root.saving = false;
            root.closeWindow();
        }, function(err) {
            root.saving = false;
            root.errorMessage = root.errorText(err);
        });
    }

    pageStack.initialPage: Kirigami.Page {
        id: initialPage
        title: "Sign in to OneDrive"
        ColumnLayout {
            anchors.centerIn: parent
            width: Math.min(parent.width - Kirigami.Units.largeSpacing * 2, Kirigami.Units.gridUnit * 24)
            spacing: Kirigami.Units.largeSpacing
            Controls.Label {
                text: "Welcome to OneDrive Hydration.\nSign in with your Microsoft account to get started."
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }
            Controls.Button {
                objectName: "wizardSignInButton"
                text: root.enrollmentBusy ? "Waiting for browser…" : "Sign In"
                enabled: !root.enrollmentBusy
                icon.name: "system-log-in"
                Layout.alignment: Qt.AlignHCenter
                onClicked: root.beginEnrollment()
            }
            Controls.Label {
                objectName: "wizardStatusLabel"
                visible: root.statusMessage !== ""
                text: root.statusMessage
                textFormat: Text.PlainText
                wrapMode: Text.WrapAnywhere
                horizontalAlignment: Text.AlignHCenter
                Layout.fillWidth: true
            }
            Controls.Label {
                objectName: "wizardErrorLabel"
                visible: root.errorMessage !== ""
                text: root.errorMessage
                textFormat: Text.PlainText
                wrapMode: Text.WrapAnywhere
                horizontalAlignment: Text.AlignHCenter
                color: Kirigami.Theme.negativeTextColor
                Layout.fillWidth: true
            }
        }
    }

    Component {
        id: folderSelectionPage
        Kirigami.Page {
            title: "Choose Folders to Sync"
            ColumnLayout {
                anchors.fill: parent
                anchors.margins: Kirigami.Units.largeSpacing
                spacing: Kirigami.Units.smallSpacing
                Controls.Label {
                    text: "Check any folders you want to exclude from syncing on this device. Unchecked folders stay available as online-only placeholders."
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }

                Controls.BusyIndicator {
                    visible: root.loading
                    running: root.loading
                    Layout.alignment: Qt.AlignHCenter
                }

                ListView {
                    id: folderList
                    objectName: "wizardFolderList"
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    model: root.cloudFolders
                    clip: true
                    delegate: Controls.CheckBox {
                        id: folderDelegate
                        required property string modelData
                        text: folderDelegate.modelData
                        width: ListView.view ? ListView.view.width : implicitWidth
                        checked: root.isFolderExcluded(folderDelegate.modelData)
                        onToggled: root.toggleExcluded(folderDelegate.modelData, checked)
                    }
                }

                Controls.Label {
                    objectName: "wizardFolderErrorLabel"
                    visible: root.errorMessage !== ""
                    text: root.errorMessage
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    color: Kirigami.Theme.negativeTextColor
                    Layout.fillWidth: true
                }

                RowLayout {
                    Layout.fillWidth: true
                    Item { Layout.fillWidth: true }
                    Controls.Button {
                        objectName: "wizardFinishButton"
                        text: root.saving ? "Saving…" : "Finish Setup"
                        enabled: !root.loading && !root.saving
                        icon.name: "dialog-ok"
                        onClicked: root.finishSetup()
                    }
                }
            }
        }
    }
}
