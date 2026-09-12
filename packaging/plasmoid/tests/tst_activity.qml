// SPDX-License-Identifier: MIT OR Apache-2.0
import QtQuick
import QtQuick.Layouts
import QtTest
import "../io.github.franzjeger.OneDriveHydration/contents/ui" as Product
import "../io.github.franzjeger.OneDriveHydration/contents/ui/Presentation.js" as Status

TestCase {
    id: testCase
    name: "OneDriveActivity"
    when: windowShown
    visible: true
    width: 460
    height: 460

    QtObject {
        id: model
        property bool serviceAvailable: true
        property bool stateKnown: true
        property bool daemonRunning: true
        property bool desktopAvailable: false
        property bool paused: false
        property bool controlBusy: false
        property string controlResult: ""
        property var syncIssues: []
        property var conflictReview: ({})
        property var resolution: ({})
        property bool canResolve: false
        function reviewConflict(path) {}
        function resolveConflict(token, choice) {}
        function openRecovery(path) {}
        function openCloudVersion(url) {}
        property var excludedFolders: []
        property var transferState: ({})
        function setFolderSelection(paths) { excludedFolders = paths.slice(); }
        property var queueRows: []
        property var account: ({})
        property var availabilityJob: ({})
        function cancelJob() {}
        function setPause(seconds) { paused = seconds > 0; }
        function retryPending() {}
        function formatBytes(value) { return Status.bytes(value); }
        function openWeb() {}
        property int unsent: 0
        property int excluded: 165994
        property int exposures: 0
        property int downloading: 0
        property bool indexing: false
        property var activeUploads: []
        property string credentialState: "healthy"
        property bool enrollmentBusy: false
        property string enrollmentResult: ""
        property bool enrollmentFailed: false
        property string evictResult: ""
        property bool evictFailed: false
        property bool evictBusy: false
        property var recentActivity: []
        property string mountPath: "/tmp/OneDrive #1"
        property url mountUrl: Status.fileUrl(mountPath)
        property bool expanded: true
        readonly property bool activityVisible: stateKnown && daemonRunning
        readonly property bool working: activityVisible && (downloading > 0 || indexing || activeUploads.length > 0)
        readonly property var presentation: Status.present(model)
        function count(n, singular, plural) { return Status.count(n, singular, plural); }
        function beginEnrollment() {}
        function openMount() {}
        function configure() {}
        function openContainingFolder(relative) {}
    }
    Product.FullRepresentation { id: panel; anchors.fill: parent; host: model }
    Product.ActivityLog { id: activityLog }
    Product.CompactRepresentation { id: compact; plasmoidItem: model; width: 24; height: 24; visible: false }

    function init() {
        model.serviceAvailable = true;
        model.stateKnown = true;
        model.daemonRunning = true;
        model.unsent = 0;
        model.desktopAvailable = false;
        model.paused = false;
        model.queueRows = [];
        model.transferState = ({});
        model.conflictReview = ({});
        model.resolution = ({});
        model.canResolve = false;
        model.excludedFolders = [];
        panel.selectionExpanded = false;
        model.syncIssues = [];
        model.account = ({});
        model.exposures = 0;
        model.downloading = 0;
        model.indexing = false;
        model.activeUploads = [];
        model.credentialState = "healthy";
        model.enrollmentResult = "";
        model.evictResult = "";
        model.recentActivity = [];
        panel.detailsExpanded = false;
        compact.visible = false;
        testCase.width = 460;
        testCase.height = 460;
    }

    function test_engine_refusal_is_not_up_to_date() {
        model.desktopAvailable = true;
        model.syncIssues = [{path: "Documents/a.txt", kind: "availability", detail: "Local data needs review"}];
        compare(model.unsent, 0);
        compare(model.presentation.headline, "Needs attention");
        verify(model.presentation.attention);
        model.paused = true;
        compare(model.presentation.headline, "Needs attention");
        model.syncIssues = [];
        compare(model.presentation.headline, "Sync paused");
    }

    function test_transfer_progress_comes_from_payload_bytes() {
        model.desktopAvailable = true;
        model.transferState = {count: 1, upload_rate: 128, download_rate: 0,
            active: [{path: "a.txt", direction: "upload", total: 100, position: 40, confirmed: 0, object_size: 100}]};
        const progress = findChild(panel, "transferProgress");
        verify(progress !== null);
        compare(progress.value, 40);
        compare(progress.to, 100);
    }
    function test_folder_selection_keeps_status_honest() {
        model.desktopAvailable = true;
        model.excludedFolders = ["Archive"];
        verify(model.presentation.detail.indexOf("Selected folders") >= 0);
        panel.selectionExpanded = true;
        wait(50);
        const apply = findChild(panel, "applyFolderSelection");
        verify(apply !== null);
        verify(!apply.enabled, "unchanged selection does not start a write");
    }

    function test_conflict_choice_requires_review_and_resets_for_a_new_file() {
        model.desktopAvailable = true;
        panel.detailsExpanded = true;
        model.conflictReview = {token: "review-one", path: "document.txt", choices: ["cloud"], local_complete: false, cloud_size: 100, local_size: 90, cloud_modified: "2026-09-10T10:00:00Z"};
        wait(50);
        const confirm = findChild(panel, "confirmConflictChoice");
        verify(confirm !== null);
        verify(!confirm.enabled);
        verify(findChild(panel, "conflictChoice_local") === null);
        verify(findChild(panel, "conflictChoice_both") === null);
        const cloud = findChild(panel, "conflictChoice_cloud");
        verify(cloud !== null);
        cloud.clicked();
        verify(confirm.enabled);
        model.conflictReview = {token: "review-two", path: "another.txt", choices: ["both", "local", "cloud"], local_complete: true};
        verify(!confirm.enabled, "a choice for the previous file must not carry over");
        model.resolution = {running: true, path: "another.txt"};
        compare(model.presentation.headline, "Resolving file conflict");
        verify(!confirm.enabled);
    }

    function test_status_data() {
        return [
            {tag: "idle", downloading: 0, indexing: false, uploads: [], expected: "Up to date"},
            {tag: "download", downloading: 1, indexing: false, uploads: [], expected: "Downloading 1 file"},
            {tag: "index", downloading: 0, indexing: true, uploads: [], expected: "Checking for changes"},
            {tag: "upload", downloading: 0, indexing: false, uploads: ["a.txt"], expected: "Uploading 1 file"},
            {tag: "both", downloading: 1, indexing: true, uploads: ["a.txt"], expected: "Syncing files"}
        ];
    }
    function test_status(data) {
        model.downloading = data.downloading;
        model.indexing = data.indexing;
        model.activeUploads = data.uploads;
        compare(model.presentation.headline, data.expected);
        model.credentialState = "rejected";
        compare(model.presentation.headline, "Sign-in required");
        model.exposures = 1;
        compare(model.presentation.headline, "Check your OneDrive folder");
        model.daemonRunning = false;
        compare(model.presentation.headline, "Sync is stopped");
        verify(!model.working);
        model.serviceAvailable = false;
        compare(model.presentation.headline, "Sync status unavailable");
    }
    function test_queue_is_not_transfer_progress() {
        model.unsent = 1;
        compare(model.presentation.headline, "1 change waiting to upload");
        const bar = findChild(panel, "workIndicator");
        verify(bar);
        verify(!bar.visible);
        model.activeUploads = ["one-large-file.bin"];
        tryCompare(bar, "visible", true);
        verify(bar.indeterminate);
        model.daemonRunning = false;
        tryCompare(bar, "visible", false);
    }

    function test_pause_and_queue_controls() {
        model.desktopAvailable = true;
        model.queueRows = [{path: "Documents/report.txt", status: "retry", detail: "Conflict: remote file changed", retry_after: 30}];
        model.account = {owner: {user: {displayName: "Test account"}}, quota: {used: 1024, total: 4096}};
        const pause = findChild(panel, "pauseSyncButton");
        verify(pause);
        tryCompare(pause, "visible", true);
        wait(50);
        mouseClick(pause);
        compare(model.paused, true);
        compare(model.presentation.headline, "Sync paused");
        model.activeUploads = ["finishing.txt"];
        compare(model.presentation.headline, "Pausing after current work");
        mouseClick(pause);
        compare(model.paused, false);
        compare(Status.bytes(1024), "1.0 KiB");
    }
    function test_footer_survives_long_content() {
        testCase.width = 360;
        testCase.height = 288;
        model.credentialState = "rejected";
        model.enrollmentResult = "A long browser error with a URL: " + "a".repeat(500);
        model.activeUploads = Array.from({length: 25}, (_, i) => "Reports/" + "long-file-name-".repeat(8) + i + ".txt");
        panel.detailsExpanded = true;
        wait(100);
        const footer = findChild(panel, "footer");
        const position = footer.mapToItem(panel, 0, 0);
        verify(position.y >= 0);
        verify(position.y + footer.height <= panel.height + 1, "footer must remain inside the popup");
        const scroll = findChild(panel, "activityScroll");
        verify(scroll.contentHeight > scroll.height, "long content must scroll");
        verify(panel.implicitWidth <= testCase.width + 1, "long filenames must not widen the popup");
    }
    function test_file_urls_preserve_names_and_reject_escapes() {
        compare(Status.fileUrl("/tmp/OneDrive #1/Å? 50%.txt"), "file:///tmp/OneDrive%20%231/%C3%85%3F%2050%25.txt");
        compare(Status.parentUrl("/tmp/OneDrive #1", "Reports/#final?.txt"), "file:///tmp/OneDrive%20%231/Reports");
        for (const path of ["../outside/file", "/absolute/file", "Reports/../../escape", "Reports//file", "bad\0name"])
            compare(Status.parentUrl("/tmp/OneDrive", path), "");
    }

    function test_history_does_not_invent_success_or_replay_snapshots() {
        activityLog.entries = [];
        activityLog.activeUploads = [];
        activityLog.observeUploads(["a.txt"], false);
        compare(activityLog.entries.length, 0);
        activityLog.observeUploads(["a.txt", "b.txt"], true);
        compare(activityLog.entries.length, 1);
        compare(activityLog.entries[0].path, "b.txt");
        compare(activityLog.entries[0].detail, "Upload started");
        activityLog.observeUploads(["a.txt", "b.txt"], true);
        activityLog.observeUploads([], true);
        compare(activityLog.entries.length, 1);
        for (let i = 0; i < 40; i++)
            activityLog.observeUploads([i + ".txt"], true);
        compare(activityLog.entries.length, 20);
        compare(activityLog.entries[0].path, "39.txt");
    }

    function test_tray_can_be_opened_with_the_keyboard() {
        compact.visible = true;
        model.expanded = false;
        compact.forceActiveFocus();
        verify(compact.activeFocus);
        keyClick(Qt.Key_Space);
        compare(model.expanded, true);
        keyClick(Qt.Key_Return);
        compare(model.expanded, false);
    }

    function test_settings_accept_plasmas_initial_page_properties() {
        const component = Qt.createComponent("../io.github.franzjeger.OneDriveHydration/contents/ui/configGeneral.qml");
        compare(component.status, Component.Ready, component.errorString());
        const settings = component.createObject(testCase, {title: "OneDrive settings", cfg_mountPath: "/tmp/OneDrive"});
        verify(settings !== null, "Plasma injects title when creating a configuration page");
        compare(settings.title, "OneDrive settings");
        compare(settings.cfg_mountPath, "/tmp/OneDrive");
        settings.destroy();
    }
}
