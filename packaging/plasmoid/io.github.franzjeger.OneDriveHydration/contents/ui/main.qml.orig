/*
    SPDX-FileCopyrightText: 2026 Frank
    SPDX-License-Identifier: MIT OR Apache-2.0
*/

// Signal-driven state from the existing D-Bus service. Presentation.js holds
// the status rules mirrored by the Rust tray; ActivityLog keeps observed
// starts and confirmed user actions for this Plasma session only.
// D-Bus u64 values may arrive as wrappers with a `value` property.
// SignalWatcher dispatches members to functions named dbus<Member>.

pragma ComponentBehavior: Bound

import "Presentation.js" as Status
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

    // What the daemon last told us, exactly as it said it. The counters keep
    // their last-seen values while daemonRunning is false — zeroing them
    // would manufacture a state the daemon never sent — and are only ever
    // *quoted* in that case, never presented as current.
    property var desktop: ({})
    property var availabilityJob: ({})
    property int jobGeneration: 0
    function applyJob(text, expand) {
        jobGeneration++;
        let next;
        try { next = JSON.parse(text || "{}"); } catch (_) { next = ({}); }
        if (expand && next.operation && next.id !== availabilityJob.id) root.expanded = true;
        if (expand && next.operation && !next.running && (availabilityJob.running || next.id !== availabilityJob.id)) {
            const failed = (next.errors || []).length > 0;
            activityLog.add(next.operation === "keep" ? "Keep on Device" : "Free Up Space",
                next.cancelled ? "Cancelled" : failed ? "Finished with errors" : "Finished: " + next.done + " files processed",
                failed ? "dialog-error" : "emblem-success", "");
        }
        availabilityJob = next;
    }
    function cancelJob() { control("CancelAvailabilityJob", []); }
    function keepFolder(url) {
        const path = decodeURIComponent(url.toString().replace(/^file:\/\//, ""));
        if (!path.startsWith(mountPath + "/")) { controlResult = "Choose a folder inside OneDrive."; return; }
        control("StartAvailabilityJob", ["keep", [path]]);
    }
    property int desktopGeneration: 0
    readonly property bool desktopAvailable: activityVisible && desktop.available === true
    readonly property bool paused: desktopAvailable && desktop.paused === true
    readonly property var transferState: desktopAvailable ? (desktop.transfers || {}) : ({})
    readonly property var excludedFolders: desktopAvailable ? (desktop.selection || []) : []
    function setFolderSelection(paths) { control("SetFolderSelection", [paths]); }
    readonly property var syncIssues: desktopAvailable ? (desktop.issues || []) : []
    readonly property var queueRows: desktopAvailable ? (desktop.queue || []) : []
    readonly property var account: desktopAvailable ? (desktop.account || {}) : ({})
    property var conflictReview: ({})
    readonly property var resolution: desktopAvailable ? (desktop.resolution || {}) : ({})
    readonly property bool canResolve: desktopAvailable && desktop.can_resolve === true
    function reviewConflict(path) {
        if (controlBusy) return;
        controlBusy = true; controlResult = "";
        DBus.SessionBus.asyncCall({service: busName, path: objectPath, iface: busName,
            member: "ReviewConflict", arguments: [path]}, reply => {
                controlBusy = false;
                try { conflictReview = JSON.parse(reply.value); } catch (_) { controlResult = "Could not read the version comparison."; }
            }, reply => { controlBusy = false; controlResult = reply.error.message; });
    }
    function resolveConflict(token, choice) { control("ResolveConflict", [token, choice]); }
    function openRecovery(path) {
        if (path && path.startsWith("/") && !path.startsWith(mountPath + "/")) Qt.openUrlExternally(Status.fileUrl(path));
    }
    function openCloudVersion(url) { if (url && url.startsWith("https://")) Qt.openUrlExternally(url); }
    property DBus.uint64 pauseArgument: 0
    property bool controlBusy: false
    property string controlResult: ""
    function applyDesktop(text) {
        desktopGeneration++;
        try { desktop = JSON.parse(text || "{}"); if (desktop.resolution && desktop.resolution.running) conflictReview = ({}); } catch (_) { desktop = ({}); }
    }
    function control(member, args) {
        if (controlBusy) return;
        controlBusy = true;
        controlResult = "";
        // DBusMessage.signature describes the reply, not the input. Preserve
        // uint64 via the QML value type so JavaScript does not send a double.
        if (member === "Pause") pauseArgument = args[0];
        DBus.SessionBus.asyncCall({service: busName, path: objectPath, iface: busName,
            member: member, arguments: member === "Pause" ? [pauseArgument] : args}, () => {
                controlBusy = false;
                readAll();
            }, error => {
                controlBusy = false;
                controlResult = error.error.message;
            });
    }
    function setPause(seconds) { control("Pause", [seconds]); }
    function retryPending() { control("RetryPending", []); }
    function formatBytes(value) { return Status.bytes(value); }
    function openWeb() {
        const url = account.webUrl || "https://onedrive.com";
        if (url.startsWith("https://")) Qt.openUrlExternally(url);
    }
    property bool daemonRunning: false
    property double unsent: 0
    property double excluded: 0
    property double exposures: 0
    // Fetches the client is serving right now, from the Downloading property and
    // the DownloadChanged signal. A live number (0 or 1 today), not held stale
    // like the counters above: it is only shown while it is above zero.
    property double downloading: 0

    // Whether the daemon is applying a cloud delta right now — the tray's
    // "Checking for changes". Its own Indexing property and IndexingChanged signal, the same
    // pattern as downloading, so a service too old to expose it is simply not
    // shown as indexing (u64(undefined)/false).
    property bool indexing: false

    // The files the daemon is sending to OneDrive right now, as relative paths —
    // the flyout's per-file "Uploading" list. Its own Uploading property and
    // ActiveUploadsChanged signal, the same pattern as downloading; a service
    // too old to expose it answers undefined, which normalizes to an empty list
    // and the row simply does not appear.
    property var activeUploads: []

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
    property bool enrollmentBusy: false
    property bool enrollmentPrepared: false
    property bool enrollmentQueryPending: false
    property string enrollmentResult: ""
    property bool enrollmentFailed: false

    Timer {
        interval: 1000
        repeat: true
        running: root.enrollmentBusy && root.enrollmentPrepared
        onTriggered: root.readEnrollmentStatus()
    }

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
    property int uploadGeneration: 0

    // The sync root. The daemon knows its mount but the D-Bus surface does
    // not expose it, so like the tray (which is told with --mount) the
    // flyout is told through configuration, defaulting to ~/OneDrive, the
    // path the deployment documents. Trailing slashes are stripped so the
    // eviction prefix check below cannot be fooled by "/path//".
    readonly property string mountPath: {
        let configured = root.desktopAvailable && root.desktop.mount ? root.desktop.mount : Plasmoid.configuration.mountPath;
        if (!configured || configured === "") {
            const home = StandardPaths.writableLocation(StandardPaths.HomeLocation).toString();
            configured = home.replace(/^file:\/\//, "") + "/OneDrive";
        }
        return configured.replace(/\/+$/, "");
    }
    readonly property url mountUrl: Status.fileUrl(root.mountPath)

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

    readonly property bool serviceAvailable: serviceWatcher.registered
    readonly property bool activityVisible: stateKnown && daemonRunning
    readonly property bool working: activityVisible && (downloading > 0 || indexing || activeUploads.length > 0)
    readonly property var presentation: Status.present(root)

    function count(n, singular, plural) { return Status.count(n, singular, plural); }

    // Only observed starts and confirmed user actions are recorded. An upload
    // disappearing from the active list is not proof that it succeeded.
    readonly property var recentActivity: {
        const history = (desktop.history || []).map(function(e) {
            return {title: e.path ? e.path.split("/").pop() : "OneDrive", detail: e.detail,
                icon: e.status === "error" ? "dialog-error" : "emblem-success", path: e.path || "", time: new Date(e.time * 1000)};
        });
        return history.concat(activityLog.entries).sort((a, b) => b.time - a.time).slice(0, 100);
    }
    ActivityLog { id: activityLog }
    function addActivity(title, detail, icon, relative) {
        activityLog.add(title, detail, icon, relative);
    }

    function openContainingFolder(relative) {
        const url = Status.parentUrl(root.mountPath, relative);
        if (url) Qt.openUrlExternally(url);
    }

    function configure() { Plasmoid.internalAction("configure").trigger(); }

    Timer {
        id: enrollmentFeedback
        interval: 6000
        onTriggered: { if (!root.enrollmentFailed) root.enrollmentResult = ""; }
    }

    // D-Bus `t` values decode as {value: n} wrappers; see the module note.
    function u64(v) {
        const number = Number((v !== null && typeof v === "object" && "value" in v) ? v.value : v);
        return Number.isFinite(number) && number >= 0 ? number : 0;
    }

    function applyState(daemonRunning, unsent, excluded, exposures) {
        root.stateGeneration += 1;
        root.daemonRunning = daemonRunning;
        root.unsent = unsent;
        root.excluded = excluded;
        root.exposures = exposures;
        root.stateKnown = true;
        if (!daemonRunning) {
            root.downloading = 0;
            root.indexing = false;
            root.activeUploads = [];
            activityLog.observeUploads([], false);
        }
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

    // The per-file upload list rides its own signal too, so it carries its own
    // generation the same way — an ActiveUploadsChanged that lands while a cold
    // GetAll is in flight must win over the older read. A D-Bus `as` (array of
    // strings) decodes to a JS array of strings; against a service too old to
    // expose the property it is undefined, which normalizes to an empty list.
    function applyUploads(value, recordActivity = true) {
        root.uploadGeneration += 1;
        root.activeUploads = (value !== null && typeof value === "object" && "value" in value)
            ? value.value
            : value;
        if (!Array.isArray(root.activeUploads))
            root.activeUploads = [];
        activityLog.observeUploads(root.activeUploads, recordActivity && root.activityVisible);
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
        const uploadGeneration = root.uploadGeneration;
        const desktopGeneration = root.desktopGeneration;
        const jobGeneration = root.jobGeneration;
        DBus.SessionBus.asyncCall({
            service: root.busName,
            path: root.objectPath,
            iface: "org.freedesktop.DBus.Properties",
            member: "GetAll",
            arguments: [root.busName]
        }, reply => {
            const properties = reply.value;
            if (jobGeneration === root.jobGeneration) root.applyJob(properties.AvailabilityJob, false);
            if (desktopGeneration === root.desktopGeneration) root.applyDesktop(properties.DesktopState);
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
            // Uploading may be undefined against a service too old to expose it;
            // applyUploads normalizes that to an empty list.
            if (uploadGeneration === root.uploadGeneration) {
                root.applyUploads(properties.Uploading, false);
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

    function beginEnrollment() {
        if (root.enrollmentBusy) return;
        root.enrollmentPrepared = false;
        enrollmentFeedback.stop();
        root.enrollmentBusy = true;
        root.enrollmentFailed = false;
        root.enrollmentResult = "Preparing secure browser sign-in…";
        DBus.SessionBus.asyncCall({
            service: root.busName,
            path: root.objectPath,
            iface: root.busName,
            member: "BeginEnrollment",
            arguments: []
        }, reply => {
            root.enrollmentPrepared = true;
            root.enrollmentResult = "Finish signing in in your browser.";
            if (!Qt.openUrlExternally(reply.value)) {
                root.enrollmentFailed = true;
                root.enrollmentResult = "The browser could not be opened. Open this URL manually: " + reply.value;
            }
        }, reply => {
            root.enrollmentBusy = false;
            root.enrollmentFailed = true;
            root.enrollmentResult = reply.error.message;
        });
    }

    function readEnrollmentStatus() {
        if (!root.enrollmentBusy || !root.enrollmentPrepared || root.enrollmentQueryPending) return;
        root.enrollmentQueryPending = true;
        DBus.SessionBus.asyncCall({
            service: root.busName,
            path: root.objectPath,
            iface: root.busName,
            member: "EnrollmentStatus",
            arguments: []
        }, reply => {
            root.enrollmentQueryPending = false;
            const status = reply.value;
            if (status === "complete") {
                root.enrollmentBusy = false;
                root.enrollmentFailed = false;
                root.enrollmentResult = "Sign-in completed.";
                root.addActivity("Signed in to OneDrive", "Sign-in completed", "system-log-in", "");
                enrollmentFeedback.restart();
            } else if (typeof status === "string" && status.startsWith("error:")) {
                root.enrollmentBusy = false;
                root.enrollmentFailed = true;
                root.enrollmentResult = status.slice("error:".length);
            }
        }, reply => {
            root.enrollmentQueryPending = false;
            root.enrollmentBusy = false;
            root.enrollmentFailed = true;
            root.enrollmentResult = reply.error.message;
        });
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
            root.addActivity(relative.split("/").pop(), "Freed " + KCoreAddons.Format.formatByteSize(bytes), "folder-cloud", relative);
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
                root.stateGeneration += 1;
                root.credentialGeneration += 1;
                root.downloadGeneration += 1;
                root.indexGeneration += 1;
                root.uploadGeneration += 1;
                root.downloading = 0;
                root.indexing = false;
                root.activeUploads = [];
                activityLog.observeUploads([], false);
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

        // The per-file upload list, on its own member for the same reason: an
        // older flyout has no dbusActiveUploadsChanged and ignores it.
        function dbusDesktopChanged(state) { root.applyDesktop(state); }
        function dbusAvailabilityJobChanged(state) { root.applyJob(state, true); }

        function dbusActiveUploadsChanged(paths) {
            root.applyUploads(paths);
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
