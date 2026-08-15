//! Drift alarms between the flyout plasmoid and the Rust surfaces it fronts.
//!
//! The plasmoid is data — QML loaded by plasmashell, packaged under
//! `packaging/plasmoid/` — so cargo cannot execute it. Its runtime behaviour
//! was verified against a live session bus (a real `onedrive-hydration-dbus`
//! over a scripted control socket, walked through every state and screenshot
//! at each; see packaging/plasmoid/README.md). What cargo *can* hold still
//! are the couplings that would break silently: the bus names the QML dials,
//! the icon names it asks the theme for, and the user-facing wording it
//! shares with the tray. These tests grep the shipped sources for exactly
//! those, deriving the expected strings from the tray's own `present()`
//! wherever the sentence is static, so rewording the tray without rewording
//! the flyout fails here rather than shipping two products that disagree.

use onedrive_hydration_daemon::auth_state::CredentialState;
use onedrive_hydration_daemon::dbus::{DaemonState, BUS_NAME, INTERFACE, OBJECT_PATH};
use onedrive_hydration_daemon::tray::{
    present, ICON_APP, ICON_EXPOSED, ICON_STOPPED, ICON_SYNCED, ICON_UNSENT,
};
use std::path::{Path, PathBuf};

fn package_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/plasmoid")
        .join(BUS_NAME)
}

fn read(relative: &str) -> String {
    let path = package_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
}

fn state(daemon_running: bool, unsent: u64, excluded: u64, exposures: u64) -> DaemonState {
    DaemonState {
        daemon_running,
        unsent,
        excluded,
        exposures,
        downloading: 0,
    }
}

/// The tray's mapping with no credential asserted — how every pre-existing
/// state renders, pinned unchanged.
fn shown(state: Option<DaemonState>) -> onedrive_hydration_daemon::tray::Presentation {
    present(state, CredentialState::Unknown)
}

#[test]
fn the_package_is_a_tray_applet_named_like_the_bus() {
    let metadata: serde_json::Value =
        serde_json::from_str(&read("metadata.json")).expect("metadata.json must parse");
    assert_eq!(metadata["KPackageStructure"], "Plasma/Applet");
    // One name everywhere a user might look: the package id is the bus name,
    // which is also the directory name package_root() already resolved.
    assert_eq!(metadata["KPlugin"]["Id"], BUS_NAME);
    // The launcher icon, same as the tray item's tooltip icon.
    assert_eq!(metadata["KPlugin"]["Icon"], ICON_APP);
    // What makes the system tray adopt it, and where it files the entry —
    // the same category the StatusNotifierItem declares.
    assert_eq!(metadata["X-Plasma-NotificationArea"], "true");
    assert_eq!(
        metadata["X-Plasma-NotificationAreaCategory"],
        "ApplicationStatus"
    );
    let main_script = metadata["X-Plasma-MainScript"]
        .as_str()
        .expect("the package declares its main script");
    assert!(
        package_root().join("contents").join(main_script).is_file(),
        "the declared main script must exist"
    );
}

#[test]
fn the_flyout_dials_the_surface_this_crate_serves() {
    let qml = read("contents/ui/main.qml");
    assert!(qml.contains(BUS_NAME), "the QML must name the bus");
    assert!(
        qml.contains(OBJECT_PATH),
        "the QML must name the object path"
    );
    // INTERFACE equals BUS_NAME today; if that ever diverges this assert
    // makes someone look at the QML's iface fields.
    assert!(qml.contains(INTERFACE));
    // The subscription: org.kde.plasma.workspace.dbus dispatches a bus
    // signal to a function named "dbus" + member, so this is both the
    // subscription and the member name in one string.
    assert!(
        qml.contains("function dbusStateChanged("),
        "the flyout must subscribe to StateChanged, not poll"
    );
    // The sign-in conclusion travels on its own member, subscribed the same
    // way, and its property is part of the cold read.
    assert!(
        qml.contains("function dbusCredentialStateChanged("),
        "the flyout must subscribe to CredentialStateChanged, not poll"
    );
    assert!(qml.contains("properties.CredentialState"));
    // The in-flight download count travels on its own member the same way, and
    // its property is part of the cold read.
    assert!(
        qml.contains("function dbusDownloadChanged("),
        "the flyout must subscribe to DownloadChanged, not poll"
    );
    assert!(qml.contains("properties.Downloading"));
    // The documented complement to the signal: one cold read of all the
    // properties when the service (re)appears.
    assert!(qml.contains("\"GetAll\""));
    // The one method the surface offers.
    assert!(qml.contains("\"Evict\""));
    // The named errors the flyout branches on.
    assert!(qml.contains(".Error.Kept"));
    for icon in [ICON_SYNCED, ICON_UNSENT, ICON_EXPOSED, ICON_STOPPED] {
        assert!(qml.contains(icon), "the QML must use the theme icon {icon}");
    }
}

#[test]
fn the_flyout_wording_cannot_drift_from_the_tray() {
    let qml = read("contents/ui/main.qml");

    // Wherever present() produces a static sentence, require it verbatim.
    let service_absent = shown(None);
    assert!(qml.contains(&service_absent.headline));
    assert!(qml.contains(&service_absent.detail));

    let stopped = shown(Some(state(false, 0, 0, 0)));
    assert!(qml.contains(&stopped.headline));
    assert!(
        qml.contains(&stopped.detail),
        "the stopped state must say, verbatim, that nothing is lost"
    );

    let exposed_one = shown(Some(state(true, 0, 0, 1)));
    assert!(qml.contains(&exposed_one.headline));
    assert!(qml.contains(&exposed_one.detail));
    let exposed_many = shown(Some(state(true, 0, 0, 2)));
    assert!(qml.contains(&exposed_many.detail));

    let synced_bare = shown(Some(state(true, 0, 0, 0)));
    assert!(qml.contains(&synced_bare.headline));
    assert!(qml.contains(&synced_bare.detail));

    // The sign-in states, derived the same way. The rejected detail with no
    // unsent work is one static sentence in both sources — including the
    // tool that works on this deployment and why the device-code flow does
    // not.
    let rejected = present(Some(state(true, 0, 0, 0)), CredentialState::Rejected);
    assert!(qml.contains(&rejected.headline));
    assert!(
        qml.contains(&rejected.detail),
        "the signed-out state must carry the tray's wording verbatim, \
         instruction included: {:?}",
        rejected.detail
    );

    // The unsaved caveat is appended to a base sentence in both sources, so
    // the contiguous literal is the caveat itself: derive it by stripping
    // the base the tray put it after.
    let unsaved = present(Some(state(true, 0, 0, 0)), CredentialState::Unsaved);
    let caveat = unsaved
        .detail
        .strip_prefix(&synced_bare.detail)
        .expect("the caveat is appended to the synced detail");
    assert!(!caveat.is_empty());
    assert!(
        qml.contains(caveat),
        "the flyout must carry the store caveat verbatim: {caveat:?}"
    );

    // Interpolated sentences appear in the QML as fragments around the
    // count; pin the fragments that carry the meaning. Each is a contiguous
    // literal in both tray.rs and the QML.
    for fragment in [
        " mounts bypass hydration",
        " still waiting to upload.",
        " to upload",
        " not reached OneDrive yet.",
        "local change has",
        "local changes have",
        " 1 file is a cloud-only placeholder.",
        " files are cloud-only placeholders.",
        "Before it stopped, ",
        "exposed the sync folder.",
        "other mount",
    ] {
        assert!(
            qml.contains(fragment),
            "the flyout must carry the tray's wording fragment {fragment:?}"
        );
    }
}

#[test]
fn the_folder_entry_keeps_the_menu_label() {
    // The tray's DBusMenu entry and the flyout's button are the same action
    // and must read the same. The label lives as a literal in both sources.
    let label = "Open OneDrive Folder";
    let tray_rs =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tray.rs"))
            .expect("tray.rs is part of this crate");
    assert!(tray_rs.contains(label));
    assert!(read("contents/ui/FullRepresentation.qml").contains(label));
    assert!(
        read("contents/ui/main.qml").contains(label),
        "the context menu action carries the same label"
    );
}

#[test]
fn the_mount_is_configuration_not_a_guess() {
    // The D-Bus surface does not expose the mount path, so the flyout is
    // told through plasmoid configuration the way the tray is told through
    // --mount. The config schema must declare that key.
    let schema = read("contents/config/main.xml");
    assert!(schema.contains("name=\"mountPath\""));
    let qml = read("contents/ui/main.qml");
    assert!(qml.contains("Plasmoid.configuration.mountPath"));
}
