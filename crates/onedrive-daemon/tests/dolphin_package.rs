//! Drift alarms between the Dolphin action and the protocol it speaks.
//!
//! The action is data — a KIO servicemenu and a POSIX shell wrapper under
//! `packaging/dolphin/` — so cargo cannot execute it, and Dolphin is not
//! available to a test runner anyway. Its *matching* behaviour was measured
//! separately, against this machine's real KIO, with
//! `probes/servicemenu-match.cpp`; that answer is recorded in
//! `docs/DOLPHIN-GROUNDWORK.md` and is not re-derived here.
//!
//! What cargo can hold still is the coupling that would otherwise break in
//! silence: the wrapper parses the control socket's replies by prefix, and
//! those prefixes are defined in Rust, in `parse_evict_reply`. Reword the
//! protocol and the wrapper keeps compiling, keeps running, and starts
//! reporting kept files as freed. So the prefixes are derived from the Rust
//! side here and looked for in the shipped text.

use onedrive_hydration_daemon::dbus::{parse_evict_reply, EvictReply};
use onedrive_hydration_daemon::tray::ICON_APP;
use std::path::{Path, PathBuf};

fn packaging_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/dolphin")
}

fn read(name: &str) -> String {
    let path = packaging_root().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
}

/// `Key=value` lines outside comments — the reading a .desktop parser applies.
fn keyed(text: &str, key: &str) -> Option<String> {
    text.lines()
        .map(str::trim_start)
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix(key).map(|v| v.trim().to_string()))
}

#[test]
fn the_entry_matches_files_and_not_directories() {
    let desktop = read("servicemenu.desktop.in");
    assert_eq!(keyed(&desktop, "Type=").as_deref(), Some("Service"));

    // all/allfiles, not all/all. Measured with probes/servicemenu-match.cpp:
    // all/allfiles reaches a regular file of any mimetype and does NOT reach a
    // directory. That distinction is the whole reason for the value — the
    // daemon's evict verb takes a file, and there is no bulk-evict to offer on
    // a folder — so a change to all/all here is a silent behaviour change.
    assert_eq!(
        keyed(&desktop, "MimeType=").as_deref(),
        Some("all/allfiles;")
    );

    // %F, not %f: the entry survives a multi-file selection (measured), and
    // the wrapper loops. With %f only the first file would ever be evicted,
    // while the menu still offered the action for all of them.
    let exec = keyed(&desktop, "Exec=").expect("the action must have an Exec");
    assert!(exec.ends_with(" %F"), "{exec}");
    assert!(
        exec.contains("@ACTION@"),
        "the wrapper path is substituted: {exec}"
    );

    // The icon the tray already publishes, so install-icons.sh remains the
    // single prerequisite rather than this adding an asset of its own.
    assert_eq!(keyed(&desktop, "Icon=").as_deref(), Some(ICON_APP));
}

#[test]
fn the_wrapper_parses_the_replies_this_crate_actually_produces() {
    let wrapper = read("free-up-space.sh.in");

    // Derived from the Rust parser rather than typed in twice: each of these
    // is a reply shape parse_evict_reply recognises, and the wrapper has to
    // recognise the same three or it will mis-report one of them.
    let reclaimed = "reclaimed 4096 bytes";
    let kept = "kept: OpenByAnotherProcess";
    assert_eq!(parse_evict_reply(reclaimed), EvictReply::Reclaimed(4096));
    assert_eq!(
        parse_evict_reply(kept),
        EvictReply::Kept("OpenByAnotherProcess".to_owned())
    );

    // The success prefix and suffix the wrapper strips to get a byte count.
    // Written as the two halves the shell actually matches on, so rewording
    // either end of the Rust reply fails here.
    let (prefix, suffix) = reclaimed.split_at("reclaimed ".len());
    assert_eq!(prefix, "reclaimed ");
    let suffix = &suffix[suffix.len() - " bytes".len()..];
    assert!(
        wrapper.contains(&format!("\"{prefix}\"*\"{suffix}\"")),
        "the wrapper must match the daemon's success reply {reclaimed:?}"
    );
    assert!(
        wrapper.contains(&format!("n=${{reply#{prefix}}}")),
        "the wrapper must strip the success prefix to get the byte count"
    );

    // Everything that is not a success is quoted verbatim rather than
    // classified, so `kept:` needs no branch of its own — but the wrapper must
    // not key off the exit status, which is the trap this whole test exists
    // for. onedrive-hydrationctl exits 0 for a kept reply.
    assert!(
        wrapper.contains("|| true"),
        "the wrapper must capture the reply rather than let a failing exit \
         status abort it"
    );
    assert!(
        kept.starts_with("kept: ") && wrapper.contains("kept:"),
        "the kept case must at least be named in the wrapper's reasoning"
    );
}

#[test]
fn the_wrapper_never_opens_the_file_it_is_about_to_evict() {
    // On a hydration mount a read is what hydrates a placeholder, so a wrapper
    // that inspected its argument would fill the very file the user asked to
    // empty — and would do it *before* asking the daemon to empty it. Only
    // path operations are allowed on the target.
    let wrapper = read("free-up-space.sh.in");
    for line in wrapper.lines() {
        let code = line.trim_start();
        if code.starts_with('#') {
            continue;
        }
        // Command *position*, not substring. The first draft of this test
        // searched for "file " anywhere on the line and tripped over the
        // sentence "not a file in it" inside a user-facing message — a test
        // that fails for a reason it does not claim is no better than one
        // that passes for a reason it does not claim.
        for segment in code.split(['|', ';', '&', '(', ')', '`', '{', '}']) {
            let segment = segment.trim_start();
            for reader in [
                "cat ", "head ", "tail ", "file ", "grep ", "md5sum ", "od ", "less ",
            ] {
                assert!(
                    !segment.starts_with(reader),
                    "the wrapper must not read the target's content ({reader:?}): {line}"
                );
            }
        }
    }
}

#[test]
fn the_keep_on_device_action_is_present_and_file_only() {
    let desktop = read("servicemenu.desktop.in");

    // Both actions are registered in the shared entry.
    let actions = keyed(&desktop, "Actions=").expect("Actions= must be present");
    assert!(
        actions.contains("onedriveHydrationFreeUpSpace"),
        "{actions}"
    );
    assert!(
        actions.contains("onedriveHydrationKeepOnDevice"),
        "Keep on Device is not registered: {actions}"
    );

    // Its own block: own Name, own wrapper via @ACTION2@, same %F contract.
    let block = desktop
        .split("[Desktop Action onedriveHydrationKeepOnDevice]")
        .nth(1)
        .expect("the Keep on Device action block is missing");
    assert_eq!(keyed(block, "Name=").as_deref(), Some("Keep on Device"));
    let exec = keyed(block, "Exec=").expect("Keep on Device needs an Exec");
    assert!(exec.ends_with(" %F"), "{exec}");
    assert!(
        exec.contains("@ACTION2@"),
        "the second wrapper path is substituted: {exec}"
    );

    // File-only: it shares the measured all/allfiles matching and makes no
    // unverified directory claim (that needs probes/servicemenu-match.cpp on
    // inode/directory, still unrun — see DOLPHIN-GROUNDWORK / KEEP-ON-DEVICE).
    assert_eq!(
        keyed(&desktop, "MimeType=").as_deref(),
        Some("all/allfiles;")
    );
}

#[test]
fn the_keep_on_device_wrapper_parses_pin_and_hydrate_replies() {
    let wrapper = read("keep-on-device.sh.in");

    // `pin`'s success is the bare word the framework's control socket answers
    // (`pinned`), and `hydrate`'s is `hydrated <n> bytes` from
    // onedrive-hydrationctl. The wrapper has to recognise both, or it reports a
    // kept file as a failure.
    assert!(
        wrapper.contains("pinned)"),
        "the wrapper must recognise the pin verb's success reply"
    );
    assert!(
        wrapper.contains("\"hydrated \"*\" bytes\""),
        "the wrapper must recognise the hydrate verb's success reply"
    );
    assert!(
        wrapper.contains("n=${reply#hydrated }"),
        "the wrapper must strip the hydrate prefix to get the byte count"
    );

    // The same exit-status trap Free Up Space documents: a `pin` refusal is
    // `error:` (exit 1), but a captured reply must not abort the loop.
    assert!(
        wrapper.contains("|| true"),
        "the wrapper must capture the reply rather than let a failing exit \
         status abort it"
    );
}

#[test]
fn keep_on_device_retries_only_the_measured_single_flight_eio() {
    let wrapper = read("keep-on-device.sh.in");
    assert!(wrapper.contains("hydrate_with_busy_retry"));
    assert!(wrapper.contains("\"error: Input/output error (os error 5)\")"));
    assert!(wrapper.contains("HYDRATE_BUSY_RETRIES=5"));
    assert!(wrapper.contains("reply=$(hydrate_with_busy_retry \"$MOUNT/$childrel\")"));
    assert!(wrapper.contains("reply=$(hydrate_with_busy_retry \"$abs\")"));
    assert!(
        !wrapper.contains("*\"error:\"*)"),
        "generic errors must not be retried"
    );
}

#[test]
fn the_keep_on_device_wrapper_never_opens_the_file() {
    // The read that hydrates the file is inside onedrive-hydrationctl, invoked
    // as a command — never a read in this shell. Same rule as Free Up Space,
    // checked the same way: command position, not substring.
    let wrapper = read("keep-on-device.sh.in");
    for line in wrapper.lines() {
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
                    "the wrapper must not read the target's content ({reader:?}): {line}"
                );
            }
        }
    }
}

#[test]
fn the_folder_entry_matches_directories_and_serves_both_actions() {
    let desktop = read("servicemenu-folder.desktop.in");
    assert_eq!(keyed(&desktop, "Type=").as_deref(), Some("Service"));

    // inode/directory, measured with probes/servicemenu-match.cpp on KIO 6.28:
    // reaches a directory, NOT a regular file (so it never doubles the file
    // entry), survives a multi-directory selection, and matches nothing for a
    // mixed file+directory selection.
    assert_eq!(
        keyed(&desktop, "MimeType=").as_deref(),
        Some("inode/directory;")
    );

    // Both actions, both registered in the shared entry. The folder-specific
    // Free Up Space uses the third wrapper (free-up-space-folder.sh.in),
    // distinguished from the file one so the loop is bounded by the folder
    // rather than by a single file.
    let actions = keyed(&desktop, "Actions=").expect("Actions= must be present");
    assert!(
        actions.contains("onedriveHydrationFreeUpSpace"),
        "the folder Free Up Space action is not registered: {actions}"
    );
    assert!(
        actions.contains("onedriveHydrationKeepOnDevice"),
        "the folder Keep on Device action is not registered: {actions}"
    );

    // Each action on its own block: own Name, own wrapper (@ACTION3@ / @ACTION2@),
    // same %F contract, same app icon as the file entry.
    let free_block = desktop
        .split("[Desktop Action onedriveHydrationFreeUpSpace]")
        .nth(1)
        .expect("the Free Up Space action block is missing");
    assert_eq!(keyed(free_block, "Name=").as_deref(), Some("Free Up Space"));
    let exec = keyed(free_block, "Exec=").expect("Free Up Space needs an Exec");
    assert!(exec.ends_with(" %F"), "{exec}");
    assert!(
        exec.contains("@ACTION3@"),
        "the folder-specific wrapper is substituted: {exec}"
    );
    assert_eq!(keyed(free_block, "Icon=").as_deref(), Some(ICON_APP));

    let keep_block = desktop
        .split("[Desktop Action onedriveHydrationKeepOnDevice]")
        .nth(1)
        .expect("the Keep on Device action block is missing");
    assert_eq!(
        keyed(keep_block, "Name=").as_deref(),
        Some("Keep on Device")
    );
    let exec = keyed(keep_block, "Exec=").expect("Keep on Device needs an Exec");
    assert!(exec.ends_with(" %F"), "{exec}");
    assert!(
        exec.contains("@ACTION2@"),
        "the shared wrapper is substituted: {exec}"
    );
    assert_eq!(keyed(keep_block, "Icon=").as_deref(), Some(ICON_APP));
}

#[test]
fn the_installer_makes_both_servicemenus_executable() {
    // Plasma 6 answers "You are not authorized to execute this file" when a
    // servicemenu action is clicked and the .desktop it lives in is not itself
    // executable — the entry appears, then cannot run. Measured on plasmashell
    // 6.7.4. The installer generates the file and folder menus in a loop and
    // must chmod each one, not only the wrappers they point at.
    let install = read("install-servicemenu.sh");
    assert!(
        install.contains("chmod 755 \"$mdst.tmp\""),
        "the installer must make each generated menu .desktop executable"
    );
    assert!(
        install.contains("servicemenu-folder.desktop.in=onedrive-hydration-folder.desktop"),
        "the installer must generate the folder menu entry too"
    );
}

#[test]
fn the_keep_on_device_wrapper_expands_a_directory_via_pending() {
    let wrapper = read("keep-on-device.sh.in");
    // A directory is special-cased: pinned as one mark, then its dehydrated
    // files are listed by the daemon's `pending` and hydrated one at a time.
    assert!(
        wrapper.contains("[ -d \"$abs\" ]"),
        "the wrapper must special-case a directory"
    );
    assert!(
        wrapper.contains("\"$CTL\" pending"),
        "the wrapper must ask the daemon to enumerate a directory, not walk it in shell"
    );
    // The listing is consumed by a here-doc, not a pipe: `pending | while` would
    // run the loop in a subshell and drop the accumulated byte total.
    assert!(
        !wrapper.contains("| while"),
        "a pipe into while runs in a subshell and would lose the running totals"
    );
    // Before the pull begins, the coming reads are announced with `prefetch`,
    // so the daemon fetches and verifies ahead of the one-at-a-time loop.
    // Advisory by design: the reply is discarded, and an older daemon that
    // answers "unknown command:" costs nothing but the speed-up.
    assert!(
        wrapper.contains("\"$CTL\" prefetch \"$rel\""),
        "the folder pull must announce the coming reads to the daemon"
    );
}

#[test]
fn the_installer_script_refuses_before_it_generates() {
    let install = read("install-servicemenu.sh");
    // Each of these is baked into a generated file that nothing validates
    // later, which is why they are refusals and not warnings.
    assert!(install.contains("refused: sync root"));
    assert!(install.contains("refused: "), "{install}");
    assert!(
        install.contains("onedrive-hydrationctl is missing or not executable"),
        "a missing CLI must be refused at install time, not at click time"
    );
    // The donor-plugin handling moved to overlay/install-overlay.sh: this
    // product now ships an overlay of its own, so the donor is a real collision
    // that installer owns (dolphin_overlay_package.rs holds it). A per-user data
    // install has no business deleting from /usr, so the servicemenu installer
    // points at the overlay installer and touches no system plugin itself.
    assert!(
        install.contains("overlay/install-overlay.sh"),
        "the servicemenu installer must point at the overlay installer"
    );
    assert!(
        !install.contains("sudo rm"),
        "donor removal is the overlay installer's job now, not the servicemenu's"
    );
    // Measured: no cache rebuild is needed, so the script must not ask for one.
    assert!(
        !install.contains("kbuildsycoca") || install.contains("no kbuildsycoca6 run"),
        "the script must not tell anyone to rebuild a cache that needs no rebuild"
    );
}
