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
    // Fetches the client is serving right now, from the Downloading property and
    // the DownloadChanged signal. A live number (0 or 1 today), not held stale
    // like the counters above: it is only shown while it is above zero.
    property double downloading: 0

    // Whether the daemon is applying a cloud delta right now — the tray's
    // "Indexing…". Its own Indexing property and IndexingChanged signal, the same
    // pattern as downloading, so a service too old to expose it is simply not
    // shown as indexing (u64(undefined)/false).
    property bool indexing: false

    // The daemon's sign-in conclusion, from the CredentialState property and
    // the CredentialStateChanged signal. "unknown" until a running daemon
    // asserts one; words this build does not recognise behave as "unknown"
    // because the arms below test for the words they know. Only consulted
    // while daemonRunning is true — the service resets it when the daemon
    // dies, but the mapping must not depend on that: a stopped daemon
    // cannot tell a missing credential from a locked keyring, and a
    // re-enroll instruction over a locked keyring is the exact wrong
    // message. Mirrors tray.rs's present().
    property string credentialState: "unknown"

    // False until the first read or signal after the service (re)appears.
    // Reads are asynchronous here (unlike the tray's blocking cold read), so
    // there is a moment where the service is on the bus but has not answered
    // yet; presenting the previous state as current during that gap would be
    // a quiet lie, so it gets its own transient presentation instead.
    property bool stateKnown: false

    // Bumped on every applied signal so a cold read that was overtaken by a
    // signal mid-flight cannot roll the newer state back to an older one.
    // The credential has its own generation because it arrives on its own
    // signal: a counter signal must not discard the credential a cold read
    // carries, nor the other way round.
    property int stateGeneration: 0
    property int credentialGeneration: 0
    property int downloadGeneration: 0
    property int indexGeneration: 0

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

    // The caveat appended to every running-state detail while the daemon
    // reports it cannot persist the rotated sign-in; tray.rs's
    // store_caveat(). A caveat and not a state: syncing still works, so the
    // headline stays about the work.
    function storeCaveat() {
        if (root.credentialState !== "unsaved") {
            return "";
        }
        return " Warning: the sign-in works but its rotation could not be saved to Linux Secret Service — unlock the keyring, or the next daemon start may require signing in again.";
    }

    // Map what we know to what the panel shows — tray.rs's present(), with
    // one extra transient state for the asynchronous read gap. Precedence,
    // most urgent knowledge first: service absent, state not yet read,
    // daemon not running, exposures (the §6.4a hazard — another mount
    // reaches the sync files and reads through it bypass hydration entirely,
    // the one condition a person can discover nowhere else), sign-in
    // required (the service has conclusively refused the stored sign-in;
    // exposure still outranks it because exposure corrupts reads happening
    // now, while a dead sign-in stops sync loudly), unsent work, synced.
    // Wording rule for the stopped states: the files are *unreachable*, not
    // lost, and the text says so explicitly — and the signed-out state
    // follows the same rule, naming the enrollment tool that actually works
    // on this deployment. Deliberately no sign-in button: the flyout cannot
    // run a browser flow and does not even know the daemon's client id, so
    // the honest ceiling is a sentence a person can follow in a terminal.
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
            detail += root.storeCaveat();
            return {
                icon: root.iconExposed,
                attention: true,
                headline: root.exposures === 1
                    ? "1 mount bypasses hydration"
                    : root.exposures + " mounts bypass hydration",
                detail: detail
            };
        }
        if (root.credentialState === "rejected") {
            let detail = "OneDrive no longer accepts this machine's saved sign-in — it was revoked, expired, or invalidated by a password change or policy. Nothing is lost: every synced file is still in OneDrive, but nothing syncs and cloud-only files cannot be opened until you sign in again. Sign in from a terminal with tools/pkce-enroll.py (Conditional Access blocks the built-in device-code sign-in here); the daemon adopts it and restarts by itself.";
            if (root.unsent > 0) {
                detail += " " + root.count(root.unsent, "change is", "changes are") + " still waiting to upload.";
            }
            return {
                icon: root.iconStopped,
                attention: true,
                headline: "Sign-in required",
                detail: detail
            };
        }
        if (root.unsent > 0) {
            return {
                icon: root.iconUnsent,
                attention: false,
                headline: root.count(root.unsent, "change", "changes") + " to upload",
                detail: root.count(root.unsent, "local change has", "local changes have") + " not reached OneDrive yet." + root.placeholdersLine(root.excluded) + root.storeCaveat()
            };
        }
        return {
            icon: root.iconSynced,
            attention: false,
            headline: "Up to date",
            detail: "All local changes are in OneDrive." + root.placeholdersLine(root.excluded) + root.storeCaveat()
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

    // D-Bus strings arrive plain, but tolerate the {value: x} wrapper the
    // way u64() does, and reduce anything that is not a string to
    // "unknown" — the word for "nobody has asserted anything", which is
    // also what an older service without the property answers through the
    // undefined it leaves in GetAll's dictionary.
    function credentialWord(v) {
        const raw = (v !== null && typeof v === "object" && "value" in v) ? v.value : v;
        return (typeof raw === "string") ? raw : "unknown";
    }

    function applyCredential(value) {
        root.credentialGeneration += 1;
        root.credentialState = root.credentialWord(value);
    }

    // The download count travels on its own signal, so it carries its own
    // generation the way the credential does — a DownloadChanged that lands
    // while a cold GetAll is in flight must win over the older read.
    function applyDownloading(value) {
        root.downloadGeneration += 1;
        root.downloading = root.u64(value);
    }

    // Indexing rides its own signal too, so it carries its own generation the
    // same way — an IndexingChanged that lands while a cold GetAll is in flight
    // must win over the older read.
    function applyIndexing(value) {
        root.indexGeneration += 1;
        root.indexing = value === true;
    }

    // The one cold read. Each half is applied only if no signal of its kind
    // arrived while the read was in flight: a signal always carries newer
    // knowledge than a read issued before it, and the service emits each
    // signal once per distinct state, so skipping a stale answer loses
    // nothing.
    function readAll() {
        const stateGeneration = root.stateGeneration;
        const credentialGeneration = root.credentialGeneration;
        const downloadGeneration = root.downloadGeneration;
        const indexGeneration = root.indexGeneration;
        DBus.SessionBus.asyncCall({
            service: root.busName,
            path: root.objectPath,
            iface: "org.freedesktop.DBus.Properties",
            member: "GetAll",
            arguments: [root.busName]
        }, reply => {
            const properties = reply.value;
            if (stateGeneration === root.stateGeneration) {
                root.applyState(
                    properties.DaemonRunning === true,
                    root.u64(properties.Unsent),
                    root.u64(properties.Excluded),
                    root.u64(properties.Exposures));
            }
            if (credentialGeneration === root.credentialGeneration) {
                root.applyCredential(properties.CredentialState);
            }
            // Downloading may be undefined against a service too old to expose
            // it; u64(undefined) is 0, which is the right "not downloading".
            if (downloadGeneration === root.downloadGeneration) {
                root.applyDownloading(properties.Downloading);
            }
            // Indexing may be undefined against a service too old to expose it;
            // `=== true` makes that the right "not indexing".
            if (indexGeneration === root.indexGeneration) {
                root.applyIndexing(properties.Indexing);
            }
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
                // Nothing the service asserted survives it leaving the bus.
                root.applyCredential("unknown");
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

        // The sign-in conclusion travels on its own member — a new argument
        // on StateChanged would have broken every subscriber that decodes
        // it by signature — and a member with no matching function here is
        // simply not dispatched, which is what lets an older flyout ignore
        // a newer service.
        function dbusCredentialStateChanged(state) {
            root.applyCredential(state);
        }

        // The in-flight download count, on its own member for the same reason:
        // a member with no matching function here is simply not dispatched, so
        // an older flyout ignores it and a newer one shows it.
        function dbusDownloadChanged(downloading) {
            root.applyDownloading(downloading);
        }

        // Whether a cloud delta is applying, on its own member for the same
        // reason: an older flyout has no dbusIndexingChanged and ignores it.
        function dbusIndexingChanged(indexing) {
            root.applyIndexing(indexing);
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
