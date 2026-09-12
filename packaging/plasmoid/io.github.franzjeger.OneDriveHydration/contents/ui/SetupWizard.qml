import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import org.kde.plasma.workspace.dbus as DBus

Kirigami.ApplicationWindow {
    id: root
    title: "OneDrive Setup"
    width: 600
    height: 450
    visible: true

    property string busName: "io.github.franzjeger.OneDriveHydration"
    property string objectPath: "/io/github/franzjeger/OneDriveHydration"
    property string credentialState: "unknown"
    property var cloudFolders: []
    property bool loading: false

    DBus.DBusServiceWatcher {
        busType: DBus.BusType.Session
        watchedService: root.busName
        onRegisteredChanged: if(registered) { fetchState(); } else { root.credentialState = "unknown"; }
    }

    DBus.SignalWatcher {
        busType: DBus.BusType.Session
        service: root.busName
        path: root.objectPath
        iface: root.busName
        function dbusCredentialStateChanged(state) {
            root.credentialState = state;
            if (state === "Healthy" && pageStack.currentItem === initialPage) {
                pageStack.push(folderSelectionPage);
                loadCloudFolders("/");
            }
        }
    }

    Component.onCompleted: fetchState()

    function fetchState() {
        DBus.SessionBus.asyncCall({
            service: root.busName,
            path: root.objectPath,
            iface: "org.freedesktop.DBus.Properties",
            member: "Get",
            arguments: [root.busName, "credentialState"]
        }, (reply) => {
            root.credentialState = reply;
            if (reply === "Healthy" && pageStack.currentItem === initialPage) {
                pageStack.push(folderSelectionPage);
                loadCloudFolders("/");
            }
        }, () => {});
    }

    function beginEnrollment() {
        DBus.SessionBus.asyncCall({
            service: root.busName,
            path: root.objectPath,
            iface: root.busName,
            member: "BeginEnrollment",
            arguments: []
        });
    }

    function loadCloudFolders(path) {
        root.loading = true;
        DBus.SessionBus.asyncCall({
            service: root.busName,
            path: root.objectPath,
            iface: root.busName,
            member: "ListCloudFolders",
            arguments: [path]
        }, (folders) => {
            root.loading = false;
            root.cloudFolders = folders;
        }, (err) => {
            root.loading = false;
            console.warn("Failed to load folders:", err);
        });
    }

    function finishSetup(excludedFolders) {
        DBus.SessionBus.asyncCall({
            service: root.busName,
            path: root.objectPath,
            iface: root.busName,
            member: "SetFolderSelection",
            arguments: [excludedFolders]
        });
        root.close();
    }

    pageStack.initialPage: Kirigami.Page {
        id: initialPage
        title: "Sign in to OneDrive"
        ColumnLayout {
            anchors.centerIn: parent
            spacing: Kirigami.Units.largeSpacing
            Controls.Label {
                text: "Welcome to OneDrive Hydration.\nSign in to get started."
                horizontalAlignment: Text.AlignHCenter
            }
            Controls.Button {
                text: "Sign In"
                icon.name: "system-users"
                Layout.alignment: Qt.AlignHCenter
                onClicked: beginEnrollment()
            }
        }
    }

    Component {
        id: folderSelectionPage
        Kirigami.Page {
            title: "Exclude Cloud-Only Folders"
            ColumnLayout {
                anchors.fill: parent
                anchors.margins: Kirigami.Units.largeSpacing
                Controls.Label {
                    text: "Select folders you want to remain cloud-only. They will not be downloaded to this device."
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                }
                
                Controls.BusyIndicator {
                    visible: root.loading
                    Layout.alignment: Qt.AlignHCenter
                }

                ListView {
                    id: folderList
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    model: root.cloudFolders
                    clip: true
                    delegate: Controls.CheckBox {
                        text: modelData
                        width: ListView.view.width
                    }
                }

                Controls.Button {
                    text: "Finish Setup"
                    Layout.alignment: Qt.AlignRight
                    icon.name: "dialog-ok"
                    onClicked: {
                        let excluded = [];
                        for (let i = 0; i < folderList.count; i++) {
                            let item = folderList.itemAtIndex(i);
                            if (item && item.checked) {
                                excluded.push("/" + root.cloudFolders[i]);
                            }
                        }
                        finishSetup(excluded);
                    }
                }
            }
        }
    }
}
