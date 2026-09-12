// SPDX-License-Identifier: MIT OR Apache-2.0
.pragma library

function count(n, singular, plural) {
    return n + " " + (n === 1 ? singular : plural);
}

function present(s) {
    function result(icon, headline, detail, attention) {
        return {icon: "onedrive-hydration-" + icon, headline: headline,
            detail: detail, attention: attention === true};
    }
    if (!s.serviceAvailable)
        return result("stopped", "Sync status unavailable", "Waiting to reconnect to OneDrive. Your synced files are safe in OneDrive.");
    if (!s.stateKnown)
        return result("stopped", "Reading sync status…", "Connecting to OneDrive.");
    if (!s.daemonRunning) {
        let detail = "Online-only files will be available when sync starts. Your synced files are safe in OneDrive.";
        if (s.exposures > 0)
            detail += " Before it stopped, " + count(s.exposures, "other mount", "other mounts") + " exposed the sync folder.";
        return result("stopped", "Sync is stopped", detail);
    }
    const warning = s.credentialState === "unsaved"
        ? " Unlock your keyring to keep OneDrive signed in after restarting." : "";
    const pending = s.unsent > 0
        ? " " + count(s.unsent, "change is", "changes are") + " still waiting to upload." : "";
    if (s.exposures > 0)
        return result("exposed", "Check your OneDrive folder", "Files opened through an extra mount may be empty. Unmount the extra location before opening files there." + pending + warning, true);
    if (s.credentialState === "rejected")
        return result("stopped", "Sign-in required", "Sign in again to sync changes and open online-only files. Your synced files are safe in OneDrive." + pending, true);
    if (s.resolution && s.resolution.running)
        return result("syncing", "Resolving file conflict", "Saving recovery copies and applying your choice.");
    if ((s.syncIssues || []).length > 0)
        return result("unsent", "Needs attention", count(s.syncIssues.length, "file needs", "files need")
            + " review. See the details below." + (s.paused ? " Background sync is paused." : "") + warning, true);
    if (s.paused)
        return result("stopped", s.working ? "Pausing after current work" : "Sync paused",
            "Background sync is paused. Files you open can still download." + pending + warning);
    if (s.downloading > 0 && s.activeUploads.length > 0)
        return result("syncing", "Syncing files", "Uploading and downloading your files." + warning);
    if (s.activeUploads.length > 0)
        return result("syncing", "Uploading " + count(s.activeUploads.length, "file", "files"), "Saving your changes to OneDrive." + warning);
    if (s.downloading > 0)
        return result("syncing", "Downloading " + count(s.downloading, "file", "files"), "Making your files available on this device." + warning);
    if (s.indexing)
        return result("syncing", "Checking for changes", "Looking for updates in OneDrive." + warning);
    if (s.unsent > 0) {
        const online = s.excluded === 0 ? "" : (s.excluded === 1
            ? " 1 file is available online only." : " " + s.excluded + " files are available online only.");
        return result("unsent", count(s.unsent, "change", "changes") + " waiting to upload",
            count(s.unsent, "local change has", "local changes have") + " not reached OneDrive yet." + online + warning);
    }
    return result("synced", "Up to date", (s.excludedFolders && s.excludedFolders.length
        ? "Selected folders are up to date. " + count(s.excludedFolders.length, "folder is", "folders are") + " not synced on this device."
        : "All local changes are saved in OneDrive.") + warning);
}

function bytes(value) {
    if (value === undefined || value === null || !Number.isFinite(Number(value))) return "Unavailable";
    let n = Number(value), units = ["B", "KiB", "MiB", "GiB", "TiB"], i = 0;
    while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
    return n.toLocaleString(Qt.locale(), 'f', i ? 1 : 0) + " " + units[i];
}

// Encode path segments, including literal # and ? characters, without
// treating a local filename as a URL fragment or query.
function fileUrl(path) {
    return "file://" + path.split("/").map(encodeURIComponent).join("/");
}

function parentUrl(mount, relative) {
    const parts = relative.split("/");
    if (!relative || parts.some(function(p) { return !p || p === "." || p === ".." || p.indexOf("\0") >= 0; }))
        return "";
    parts.pop();
    return fileUrl(mount + (parts.length ? "/" + parts.join("/") : ""));
}
