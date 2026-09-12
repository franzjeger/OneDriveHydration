// SPDX-License-Identifier: MIT OR Apache-2.0
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Dialogs
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.workspace.dbus as DBus

Kirigami.ScrollablePage {
    id: page
    title: "OneDrive"
    property alias cfg_mountPath: mountField.text
    property string cfg_mountPathDefault: ""
    property var desktop: ({})
    property string result: ""
    readonly property var account: desktop.account || ({})

    Kirigami.FormLayout {
        QQC2.Label {
            Kirigami.FormData.label: "Account:"
            text: page.account.owner && page.account.owner.user ? page.account.owner.user.displayName || "Unavailable" : "Unavailable"
            textFormat: Text.PlainText
        }
        QQC2.Label {
            Kirigami.FormData.label: "Connected folder:"
            text: page.desktop.mount || "Sync service unavailable"
            textFormat: Text.PlainText
            wrapMode: Text.WrapAnywhere
            Layout.fillWidth: true
        }
        QQC2.Label {
            text: "Choose folders to keep offline from OneDrive’s menu or by right-clicking them in Dolphin. A filled green circle means Keep on Device; a green square means downloaded."
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }
        QQC2.Label {
            text: "Dolphin previews can download online-only files while you browse. Turn off Show Previews in Dolphin if you want those files to stay online-only."
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }
        QQC2.Label {
            text: "Choose folders to sync from the OneDrive panel’s menu. Excluded folders show a grey pause badge in Dolphin; files already on this device stay in place."
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }
        Kirigami.Separator { Layout.fillWidth: true }
        RowLayout {
            Kirigami.FormData.label: "Fallback folder:"
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
            text: "Used only when the sync service cannot report its connected folder. The running service supplies the correct path automatically."
            textFormat: Text.PlainText
            font: Kirigami.Theme.smallFont
            opacity: 0.75
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }
    }
    FolderDialog {
        id: folderDialog
        onAccepted: mountField.text = decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, ""))
    }
    Component.onCompleted: DBus.SessionBus.asyncCall({
        service: "io.github.franzjeger.OneDriveHydration", path: "/io/github/franzjeger/OneDriveHydration",
        iface: "org.freedesktop.DBus.Properties", member: "GetAll",
        arguments: ["io.github.franzjeger.OneDriveHydration"]
    }, reply => { try { page.desktop = JSON.parse(reply.value.DesktopState || "{}"); } catch (_) {} }, () => {})
}
