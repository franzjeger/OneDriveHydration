//! Drift alarms for the Dolphin overlay plugin — the compiled KF6 emblem that
//! draws on-device / cloud-only badges. cargo cannot build it (that needs a
//! Qt6/KF6 toolchain) and cannot run Dolphin, so what it *can* hold still is the
//! text coupling that would otherwise break in silence: the xattr the plugin
//! probes, the promise that it only ever reads metadata, and the installer's
//! contract with the system plugin dir and the donor collision.
//!
//! Its loading and drawing were measured separately, against this machine's real
//! KF6, and recorded in docs/DOWNLOAD-VISIBILITY-GROUNDWORK.md (the P1/P2 gates).

use std::path::{Path, PathBuf};

fn overlay_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/dolphin/overlay")
}

fn read(name: &str) -> String {
    let path = overlay_dir().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
}

/// The one fact the plugin and the framework must agree on: the placeholder
/// mark. If HydrationAPI ever renames `hydration_protocol::xattr::DEHYDRATED`,
/// this plugin — a separate repo, a different language — would keep probing the
/// old name and badge every placeholder as resident. The value is a stable
/// on-disk format (renaming it would strip the mark from every placeholder on
/// every deployed disk), so it is asserted as the literal here, with this note
/// as the coupling that a reviewer of either side is meant to see.
#[test]
fn the_plugin_probes_the_frameworks_dehydrated_mark() {
    let cpp = read("hydrationoverlay.cpp");
    assert!(
        cpp.contains("user.hydration.dehydrated"),
        "the plugin must probe the framework's dehydrated mark \
         (hydration_protocol::xattr::DEHYDRATED)"
    );
}

/// The load-bearing safety property (HydrationAPI 6a-ter, measured event-free in
/// probes/xattrread.c): the emblem is answered by a metadata probe, never a read
/// of the file's content — a content read on a hydration mount hydrates the very
/// placeholder it is badging, or deadlocks. The plugin reads *config* with QFile,
/// which is fine; what must never appear is a content-read primitive aimed at the
/// item being badged.
#[test]
fn the_plugin_reads_metadata_never_content() {
    let cpp = read("hydrationoverlay.cpp");
    assert!(
        cpp.contains("lgetxattr"),
        "the plugin must answer with the lgetxattr presence probe"
    );
    // Code only: the header comment names these primitives to say it never uses
    // them, so cut each line at `//` before checking — the same comment-skipping
    // the shell-wrapper tests do, so the ban catches a real call, not a mention.
    let code: String = cpp
        .lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    for content_read in ["mmap", "pread", "sendfile", "fopen", "::read(", "readAll("] {
        assert!(
            !code.contains(content_read),
            "the plugin must not read file content ({content_read:?}) — that would \
             hydrate the placeholder it is drawing a badge for"
        );
    }
}

/// The emblem is scoped to the sync roots: a resident file carries no mark, so
/// without the roots the plugin could only tell "on-device sync file" from "any
/// file on the system" by badging everything. The roots config is what keeps the
/// check emblem off unrelated files, and the plugin and its installer must name
/// the same file.
#[test]
fn the_plugin_and_installer_agree_on_the_roots_config() {
    let cpp = read("hydrationoverlay.cpp");
    let install = read("install-overlay.sh");
    assert!(
        cpp.contains("overlay-roots"),
        "the plugin must read the roots config"
    );
    assert!(
        install.contains("overlay-roots"),
        "the installer must write the roots config the plugin reads"
    );
    // User scope, like the rest of this deployment's per-user state.
    assert!(
        install.contains("XDG_CONFIG_HOME") && install.contains("onedrive-hydration/overlay-roots"),
        "the roots config is user scope, under XDG_CONFIG_HOME"
    );
}

/// The plugin lands in the system Qt plugin dir — measured: a user-scope
/// ~/.local/lib plugin is not searched by Dolphin. So the installer builds and
/// installs system-wide, and the CMake registers the KF6 overlay namespace.
#[test]
fn the_plugin_installs_into_the_kf6_overlay_namespace() {
    let cmake = read("CMakeLists.txt");
    assert!(
        cmake.contains("kf6/overlayicon"),
        "the plugin must install into Dolphin's overlay-icon plugin namespace"
    );
    assert!(
        cmake.contains("KF6::KIOCore"),
        "the plugin must link the KIO core that defines KOverlayIconPlugin"
    );
    let install = read("install-overlay.sh");
    // Install to where Qt ACTUALLY searches, not the KDE cmake convention: on a
    // CachyOS/Arch desktop `cmake --install`'s KDE_INSTALL_PLUGINDIR was
    // /usr/lib/plugins while Qt6 searches /usr/lib/qt6/plugins, so the emblems
    // silently never loaded. The installer asks Qt for its plugin dir and
    // installs the .so into that kf6/overlayicon namespace directly.
    assert!(
        install.contains("qtpaths6 --plugin-dir") || install.contains("QT_INSTALL_PLUGINS"),
        "the installer must ask Qt where its plugins go, not assume the KDE dir"
    );
    assert!(
        install.contains("kf6/overlayicon/onedrive-hydration-overlay.so"),
        "the installer must place the .so in Qt's kf6/overlayicon namespace"
    );
}

/// The donor collision is now real: this product ships an overlay of its own, so
/// the old OneDriveForLinux plugin (which reads user.onedrive.syncstate) would
/// draw a second, wrong badge on every file. The overlay installer owns removing
/// it — and must not remove the plugin it just installed.
#[test]
fn the_installer_clears_the_donor_overlay_collision() {
    let install = read("install-overlay.sh");
    assert!(
        install.contains("user.onedrive.syncstate"),
        "the installer must name the donor's xattr so a reader knows what collides"
    );
    assert!(
        install.contains("rm -f \"$donor\""),
        "the installer must remove the conflicting donor overlay plugin"
    );
    assert!(
        install.contains("onedrive-hydration-overlay.so) continue"),
        "the removal must skip the plugin this installer just placed"
    );
}

/// Same refuse-before-acting discipline as the servicemenu installer: a compiled
/// plugin whose toolchain is missing, or a sync root that does not exist, is
/// caught up front — not left to fail deep in cmake, and never with a swallowed
/// diagnostic (the plugin build's real output is printed on failure).
#[test]
fn the_installer_refuses_before_it_acts() {
    let install = read("install-overlay.sh");
    assert!(
        install.contains("refused: cmake is not installed"),
        "a missing build toolchain must be refused with the tool named"
    );
    assert!(
        install.contains("refused: sync root"),
        "a non-existent sync root must be refused up front"
    );
    // Never invent a diagnostic: on a build failure the installer prints what
    // cmake actually said rather than a guess.
    assert!(
        install.contains("the compiler said") && install.contains("build.log"),
        "a build failure must print the compiler's real output"
    );
}
