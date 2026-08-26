//! Drift alarms between the GNOME Files scripts and the protocol they speak
//! — the Nautilus sibling of `dolphin_package.rs`, holding the same
//! couplings still for `packaging/nautilus/`: the scripts parse the control
//! socket's replies by prefix, and those prefixes are defined in Rust, in
//! `parse_evict_reply`. Reword the protocol and a script keeps running and
//! starts mis-reporting; these tests fail instead.
//!
//! There is no .desktop half to pin here: Nautilus names a script by its
//! filename and applies no filtering at all, which is *why* the containment
//! tests below matter — the entries appear on every file on the system.

use onedrive_hydration_daemon::dbus::{parse_evict_reply, EvictReply};
use std::path::{Path, PathBuf};

fn packaging_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/nautilus")
}

fn read(name: &str) -> String {
    let path = packaging_root().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
}

/// No content-reader command may sit in command position: on a hydration
/// mount a read is what hydrates a placeholder, so a script that inspected
/// its argument would fill the very file the user asked to empty. Command
/// *position*, not substring — the same lesson `dolphin_package.rs` records.
fn assert_never_opens_the_target(script: &str) {
    for line in script.lines() {
        let code = line.trim_start();
        if code.starts_with('#') {
            continue;
        }
        for segment in code.split(['|', ';', '&', '(', ')', '`', '{', '}']) {
            let segment = segment.trim_start();
            for reader in [
                "cat ", "head ", "tail ", "file ", "grep ", "md5sum ", "od ", "less ",
            ] {
                assert!(
                    !segment.starts_with(reader),
                    "the script must not read the target's content ({reader:?}): {line}"
                );
            }
        }
    }
}

#[test]
fn free_up_space_parses_the_replies_this_crate_actually_produces() {
    let script = read("free-up-space.sh.in");

    // Derived from the Rust parser rather than typed in twice.
    let reclaimed = "reclaimed 4096 bytes";
    let kept = "kept: OpenByAnotherProcess";
    assert_eq!(parse_evict_reply(reclaimed), EvictReply::Reclaimed(4096));
    assert_eq!(
        parse_evict_reply(kept),
        EvictReply::Kept("OpenByAnotherProcess".to_owned())
    );

    let (prefix, suffix) = reclaimed.split_at("reclaimed ".len());
    assert_eq!(prefix, "reclaimed ");
    let suffix = &suffix[suffix.len() - " bytes".len()..];
    assert!(
        script.contains(&format!("\"{prefix}\"*\"{suffix}\"")),
        "the script must match the daemon's success reply {reclaimed:?}"
    );
    assert!(
        script.contains(&format!("n=${{reply#{prefix}}}")),
        "the script must strip the success prefix to get the byte count"
    );

    // The exit-status trap: onedrive-hydrationctl exits 0 for a kept reply,
    // so the reply must be captured rather than trusted through $?.
    assert!(
        script.contains("|| true"),
        "the script must capture the reply rather than let a failing exit \
         status abort it"
    );
}

#[test]
fn free_up_space_never_opens_the_file_and_stays_inside_the_mount() {
    let script = read("free-up-space.sh.in");
    assert_never_opens_the_target(&script);
    // The containment has to live in the script — Nautilus filters nothing.
    assert!(
        script.contains("not inside the sync folder"),
        "the script must refuse paths outside the sync root by name"
    );
    // A directly pinned file is un-kept; an ancestor pin is left alone and
    // its refusal shown. The `via` path comes from the reply, never the file.
    assert!(script.contains("Pinned { via:"));
}

#[test]
fn keep_on_device_parses_pin_and_hydrate_replies() {
    let script = read("keep-on-device.sh.in");
    assert!(
        script.contains("pinned)"),
        "the script must recognise the pin verb's success reply"
    );
    assert!(
        script.contains("\"hydrated \"*\" bytes\""),
        "the script must recognise the hydrate verb's success reply"
    );
    assert!(
        script.contains("n=${reply#hydrated }"),
        "the script must strip the hydrate prefix to get the byte count"
    );
    assert!(script.contains("|| true"));
}

#[test]
fn keep_on_device_retries_only_the_measured_single_flight_eio() {
    let script = read("keep-on-device.sh.in");
    assert!(script.contains("hydrate_with_busy_retry"));
    assert!(script.contains("\"error: Input/output error (os error 5)\")"));
    assert!(script.contains("HYDRATE_BUSY_RETRIES=5"));
    assert!(
        !script.contains("*\"error:\"*)"),
        "generic errors must not be retried"
    );
}

#[test]
fn keep_on_device_never_opens_the_file_and_expands_folders_via_pending() {
    let script = read("keep-on-device.sh.in");
    assert_never_opens_the_target(&script);
    assert!(
        script.contains("[ -d \"$abs\" ]"),
        "the script must special-case a directory"
    );
    assert!(
        script.contains("\"$CTL\" pending"),
        "the script must ask the daemon to enumerate a directory, not walk it in shell"
    );
    // The listing is consumed by a here-doc, not a pipe: `pending | while`
    // would run the loop in a subshell and drop the accumulated byte total.
    assert!(
        !script.contains("| while"),
        "a pipe into while runs in a subshell and would lose the running totals"
    );
    // Before the pull begins, the coming reads are announced with `prefetch`,
    // so the daemon fetches and verifies ahead of the one-at-a-time loop.
    // Advisory by design: the reply is discarded, and an older daemon that
    // answers "unknown command:" costs nothing but the speed-up.
    assert!(
        script.contains("\"$CTL\" prefetch \"$rel\""),
        "the folder pull must announce the coming reads to the daemon"
    );
}

#[test]
fn the_menu_extension_only_builds_menus_and_delegates_to_the_wrappers() {
    let ext = read("onedrive-hydration-menu.py.in");
    // The extension must not speak the daemon's protocol itself — every
    // action is one Popen of a generated wrapper, so the reply-prefix
    // couplings pinned above stay in exactly one place.
    assert!(ext.contains("subprocess.Popen"));
    for verb in ["evict", "hydrate", "\"pin\"", "pending"] {
        assert!(
            !ext.contains(verb),
            "the extension must not speak the control protocol itself ({verb})"
        );
    }
    // The menu is filtered to the sync root — a courtesy the wrappers do not
    // rely on (their own containment stays the rule) — and never offered on
    // the root itself.
    assert!(ext.contains("startswith(MOUNT"));
    assert!(ext.contains("path == MOUNT"));
}

#[test]
fn the_installer_treats_the_extension_as_conditional_and_says_why() {
    let install = read("install-nautilus-scripts.sh");
    // nautilus-python is detected by the same import Nautilus performs, and
    // its absence downgrades to the Scripts submenu with the package named —
    // never a refusal, because the scripts alone are a working install.
    assert!(install.contains("from gi.repository import Nautilus"));
    assert!(install.contains("onedrive-hydration-menu.py"));
    assert!(
        install.contains("no nautilus-python"),
        "the fallback must be stated, not silent"
    );
    // Extensions load at Nautilus startup; the installer must say the
    // restart is needed rather than let the entry look broken.
    assert!(install.contains("nautilus -q"));
}

#[test]
fn the_installer_script_refuses_before_it_generates() {
    let install = read("install-nautilus-scripts.sh");
    assert!(install.contains("refused: sync root"));
    assert!(
        install.contains("onedrive-hydrationctl is missing or not executable"),
        "a missing CLI must be refused at install time, not at click time"
    );
    // Nautilus names a script by its filename, so the generated files must
    // be called exactly what the menu should show.
    assert!(install.contains("free-up-space.sh.in=Free Up Space"));
    assert!(install.contains("keep-on-device.sh.in=Keep on Device"));
    // The honest gap: no emblems in GNOME Files, said instead of implied.
    assert!(
        install.contains("no overlay"),
        "the installer must say plainly that GNOME gets no emblems"
    );
}
