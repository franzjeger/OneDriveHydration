/*
    SPDX-FileCopyrightText: 2026 Frank
    SPDX-License-Identifier: MIT OR Apache-2.0
*/

// The flyout: a plasmoid loaded by plasmashell into the system tray, talking
// to the same session-bus surface the tray binary subscribes to. Shipped as
// data, not linked as a dependency — on this desktop the panel is already a
// QML host, so the flyout costs no Rust toolkit and inherits the native look.
//
// The daemon's control socket stays the single authority and the state
// service its only translator; this file holds nothing but the last state it
// was told. It subscribes to `StateChanged` and never polls: one cold
// property read when the service (re)appears on the bus — the documented
// complement to the signal, because a freshly started service does not
// signal a state it considers unchanged — and signals from then on. This
// mirrors crates/onedrive-daemon/src/tray.rs, and the user-facing wording
// here is copied from tray.rs verbatim; tests/plasmoid_package.rs pins the
// two against each other so they cannot drift apart silently.
//
// Two facts about org.kde.plasma.workspace.dbus, measured on this machine's
// Plasma 6.7.4 against the real service (not taken from documentation):
//
//  * D-Bus `t` (u64) values arrive in QML as value-type wrappers with a
//    `.value` property, not as numbers — `Excluded` decodes as
//    `{value: 167652}`. Everything numeric goes through `u64()` below;
//    without it the panel would render "[object Object]" file counts.
//  * A D-Bus signal reaches QML through a plain function named
//    `dbus<SignalName>` on a SignalWatcher — there is no receivedSignal
//    signal to attach to. The subscription survives a service restart
//    without re-arming (Qt tracks the name's owner), also measured.

pragma ComponentBehavior: Bound

import QtCore
import QtQuick
import org.kde.coreaddons as KCoreAddons
import org.kde.kirigami as Kirigami
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasmoid
import org.kde.plasma.workspace.dbus as DBus

PlasmoidItem {
    id: root

    // The bus name, object path and interface the state service serves.
    // These mirror BUS_NAME, OBJECT_PATH and INTERFACE in
    // crates/onedrive-daemon/src/dbus.rs; the package test pins them.
    readonly property string busName: "io.github.franzjeger.OneDriveHydration"
    readonly property string objectPath: "/io/github/franzjeger/OneDriveHydration"

    // Icon names from packaging/icons, resolved through the hicolor theme.
    // The same constants as tray.rs; the package test pins them too.
    readonly property string iconSynced: "onedrive-hydration-synced"
    readonly property string iconUnsent: "onedrive-hydration-unsent"
    readonly property string iconExposed: "onedrive-hydration-exposed"
    readonly property string iconStopped: "onedrive-hydration-stopped"

    // What the daemon last told us, exactly as it said it. The counters keep
    // their last-seen values while daemonRunning is false — zeroing them
    // would manufacture a state the daemon never sent — and are only ever
    // *quoted* in that case, never presented as current.
    property bool daemonRunning: false
    property double unsent: 0
    property double excluded: 0
    property double exposures: 0

    // False until the first read or signal after the service (re)appears.
    // Reads are asynchronous here (unlike the tray's blocking cold read), so
    // there is a moment where the service is on the bus but has not answered
    // yet; presenting the previous state as current during that gap would be
    // a quiet lie, so it gets its own transient presentation instead.
    property bool stateKnown: false

    // Bumped on every applied signal so a cold read that was overtaken by a
    // signal mid-flight cannot roll the newer state back to an older one.
    property int stateGeneration: 0

    // The sync root. The daemon knows its mount but the D-Bus surface does
    // not expose it, so like the tray (which is told with --mount) the
    // flyout is told through configuration, defaulting to ~/OneDrive, the
    // path the deployment documents. Trailing slashes are stripped so the
    // eviction prefix check below cannot be fooled by "/path//".
    readonly property string mountPath: {
        let configured = Plasmoid.configuration.mountPath;
        if (!configured || configured === "") {
            const home = StandardPaths.writableLocation(StandardPaths.HomeLocation).toString();
            configured = home.replace(/^file:\/\//, "") + "/OneDrive";
        }
        return configured.replace(/\/+$/, "");
    }
    readonly property url mountUrl: "file://" + encodeURI(root.mountPath)

    // Eviction state lives here rather than in the flyout page because the
    // popup's contents can be destroyed while a call is in flight; the
    // PlasmoidItem lives as long as the applet does.
    property bool evictBusy: false
    property bool evictFailed: false
    property string evictResult: ""

    switchWidth: Kirigami.Units.gridUnit * 12
    switchHeight: Kirigami.Units.gridUnit * 12

    Plasmoid.icon: root.presentation.icon
    Plasmoid.status: root.presentation.attention
        ? PlasmaCore.Types.NeedsAttentionStatus
        : PlasmaCore.Types.ActiveStatus

    toolTipMainText: root.presentation.headline
    toolTipSubText: root.presentation.detail

    // "1 change" / "3 changes", matching tray.rs's count().
    function count(n, singular, plural) {
        return n === 1 ? n + " " + singular : n + " " + plural;
    }

    // The placeholders line shown while things are healthy; tray.rs's
    // placeholders_line().
    function placeholdersLine(excluded) {
        if (excluded === 0) {
            return "";
        }
        if (excluded === 1) {
            return " 1 file is a cloud-only placeholder.";
        }
        return " " + excluded + " files are cloud-only placeholders.";
    }

    // Map what we know to what the panel shows — tray.rs's present(), with
    // one extra transient state for the asynchronous read gap. Precedence,
    // most urgent knowledge first: service absent, state not yet read,
    // daemon not running, exposures (the §6.4a hazard — another mount
    // reaches the sync files and reads through it bypass hydration entirely,
    // the one condition a person can discover nowhere else), unsent work,
    // synced. Wording rule for the stopped states: the files are
    // *unreachable*, not lost, and the text says so explicitly.
    readonly property var presentation: {
        if (!serviceWatcher.registered) {
            return {
                icon: root.iconStopped,
                attention: false,
                headline: "State service not running",
                detail: "onedrive-hydration-dbus is not on the session bus, so the daemon's state is unknown. Files stay in OneDrive either way; nothing is lost."
            };
        }
        if (!root.stateKnown) {
            return {
                icon: root.iconStopped,
                attention: false,
                headline: "Reading sync state…",
                detail: "The state service is on the session bus; waiting for its first answer."
            };
        }
        if (!root.daemonRunning) {
            let detail = "Cloud-only files cannot be opened until the daemon starts. Nothing is lost: every synced file is still in OneDrive.";
            if (root.exposures > 0) {
                // Held, last-seen knowledge — quoted as such, not shown as live.
                detail += " Before it stopped, " + root.count(root.exposures, "other mount", "other mounts") + " exposed the sync folder.";
            }
            return {
                icon: root.iconStopped,
                attention: false,
                headline: "Sync daemon not running",
                detail: detail
            };
        }
        if (root.exposures > 0) {
            let detail = root.exposures === 1
                ? "Another mount exposes the OneDrive files, and reads through it bypass hydration: they can silently return empty placeholder content. Unmount the extra path to close the bypass."
                : "Other mounts expose the OneDrive files, and reads through them bypass hydration: they can silently return empty placeholder content. Unmount the extra paths to close the bypass.";
            if (root.unsent > 0) {
                detail += " " + root.count(root.unsent, "change is", "changes are") + " still waiting to upload.";
            }
            return {
                icon: root.iconExposed,
                attention: true,
                headline: root.exposures === 1
                    ? "1 mount bypasses hydration"
                    : root.exposures + " mounts bypass hydration",
                detail: detail
            };
        }
        if (root.unsent > 0) {
            return {
                icon: root.iconUnsent,
                attention: false,
                headline: root.count(root.unsent, "change", "changes") + " to upload",
                detail: root.count(root.unsent, "local change has", "local changes have") + " not reached OneDrive yet." + root.placeholdersLine(root.excluded)
            };
        }
        return {
            icon: root.iconSynced,
            attention: false,
            headline: "Up to date",
            detail: "All local changes are in OneDrive." + root.placeholdersLine(root.excluded)
        };
    }

    // D-Bus `t` values decode as {value: n} wrappers; see the module note.
    function u64(v) {
        return (v !== null && typeof v === "object" && "value" in v) ? Number(v.value) : Number(v);
    }

    function applyState(daemonRunning, unsent, excluded, exposures) {
        root.stateGeneration += 1;
        root.daemonRunning = daemonRunning;
        root.unsent = unsent;
        root.excluded = excluded;
        root.exposures = exposures;
        root.stateKnown = true;
    }

    // The one cold read. Applied only if no signal arrived while it was in
    // flight: a signal always carries newer knowledge than a read issued
    // before it, and the service emits StateChanged once per distinct state,
    // so skipping the stale answer loses nothing.
    function readAll() {
        const generation = root.stateGeneration;
        DBus.SessionBus.asyncCall({
            service: root.busName,
            path: root.objectPath,
            iface: "org.freedesktop.DBus.Properties",
            member: "GetAll",
            arguments: [root.busName]
        }, reply => {
            if (generation !== root.stateGeneration) {
                return;
            }
            const properties = reply.value;
            root.applyState(
                properties.DaemonRunning === true,
                root.u64(properties.Unsent),
                root.u64(properties.Excluded),
                root.u64(properties.Exposures));
        }, error => {
            // The service raced away between appearing and answering; the
            // service watcher flips `registered` off on its own, and the
            // presentation already says the state is unknown. Nothing to do.
        });
    }

    function openMount() {
        Qt.openUrlExternally(root.mountUrl);
    }

    // Return a hydrated file to a placeholder over the surface's Evict
    // method. The path sent is relative to the sync root, unchanged beyond
    // the prefix strip — the daemon's reclaim path is the only place that
    // decides what a path means, and it already refuses escapes. A refusal
    // comes back as the named Kept error with the daemon's reason, and that
    // reason is shown verbatim rather than summarised here.
    function evictFile(selected) {
        const url = selected.toString();
        if (!url.startsWith("file://")) {
            root.evictFailed = true;
            root.evictResult = "Only local files can be returned to cloud-only.";
            return;
        }
        const path = decodeURIComponent(url.slice("file://".length));
        if (!path.startsWith(root.mountPath + "/")) {
            root.evictFailed = true;
            root.evictResult = "\"" + path + "\" is not inside the OneDrive folder, so there is nothing to return to cloud-only.";
            return;
        }
        const relative = path.slice(root.mountPath.length + 1);
        root.evictBusy = true;
        root.evictFailed = false;
        root.evictResult = "";
        DBus.SessionBus.asyncCall({
            service: root.busName,
            path: root.objectPath,
            iface: root.busName,
            member: "Evict",
            arguments: [relative]
        }, reply => {
            root.evictBusy = false;
            root.evictFailed = false;
            const bytes = root.u64(reply.value);
            root.evictResult = "Freed " + KCoreAddons.Format.formatByteSize(bytes) + " — \"" + relative + "\" is cloud-only again.";
        }, reply => {
            // Both callbacks receive the pending reply; a rejection carries
            // its error at reply.error (measured — the callback's argument
            // itself has no name/message).
            root.evictBusy = false;
            root.evictFailed = true;
            if (reply.error.name.endsWith(".Error.Kept")) {
                root.evictResult = "\"" + relative + "\" was kept: " + reply.error.message;
            } else {
                root.evictResult = reply.error.message;
            }
        });
    }

    property DBus.DBusServiceWatcher serviceWatcher: DBus.DBusServiceWatcher {
        busType: DBus.BusType.Session
        watchedService: root.busName
        onRegisteredChanged: {
            if (registered) {
                root.readAll();
            } else {
                root.stateKnown = false;
            }
        }
    }

    property DBus.SignalWatcher stateSignals: DBus.SignalWatcher {
        busType: DBus.BusType.Session
        service: root.busName
        path: root.objectPath
        iface: root.busName

        // Named for the StateChanged member: org.kde.plasma.workspace.dbus
        // dispatches a received signal to the function called
        // "dbus" + member. Fired once per distinct state, carrying the same
        // values as the properties, so no follow-up read is ever needed.
        function dbusStateChanged(daemonRunning, unsent, excluded, exposures) {
            root.applyState(
                daemonRunning === true,
                root.u64(unsent),
                root.u64(excluded),
                root.u64(exposures));
        }
    }

    compactRepresentation: CompactRepresentation {
        plasmoidItem: root
    }

    fullRepresentation: FullRepresentation {
        host: root
    }

    PlasmaCore.Action {
        id: openFolderAction
        text: "Open OneDrive Folder"
        icon.name: "folder-open"
        onTriggered: root.openMount()
    }

    Plasmoid.contextualActions: [openFolderAction]

    Component.onCompleted: {
        if (serviceWatcher.registered) {
            readAll();
        }
    }
}
