//! Every refusal fires, or it is not a check.
//!
//! The pattern throughout: real probe functions are exercised against real
//! conditions wherever a machine-independent one exists (a temp directory is
//! genuinely not a mount point; `/proc/self/mountinfo` genuinely lists a
//! tmpfs; a `busctl` call against a dead path genuinely fails), and the
//! judgments in `plan::install` are exercised by handing them measurements
//! this machine cannot produce (a 6.8 kernel, a bypass mount, an unreachable
//! Secret Service). What is asserted is always the refusal itself: its check
//! name, the load-bearing words of its message, and that nothing was written.

use onedrive_hydration_install::plan::{
    execute, install, uninstall, Action, ExecMode, Observed, Options, Outcome, Planned,
};
use onedrive_hydration_install::probes::{
    self, secret_service_at, KernelSupport, Plasmoid, SecretService, Vantage,
};
use onedrive_hydration_install::units::{self, Templates, Tray};
use onedrive_hydration_install::Facts;
use std::path::{Path, PathBuf};

fn facts(mount: &str, bin_dir: &Path) -> Facts {
    Facts {
        user: "u".into(),
        uid: 1234,
        gid: 1234,
        home: PathBuf::from("/home/u"),
        mount: PathBuf::from(mount),
        socket: PathBuf::from("/run/user/1234/onedrive-hydration.sock"),
        client_id: "11111111-2222-3333-4444-555555555555".into(),
        bin_dir: bin_dir.to_path_buf(),
    }
}

/// A synthetic table in which the sync root is a healthy btrfs subvolume
/// mount with no second path to it.
const GOOD_TABLE: &str = "\
40 1 0:33 /@ / rw,noatime shared:1 - btrfs /dev/sda2 rw,subvolid=256,subvol=/@
58 40 0:33 /@home /home rw,noatime shared:2 - btrfs /dev/sda2 rw,subvolid=257,subvol=/@home
90 40 0:33 /@onedrive /home/u/OneDrive rw,noatime shared:3 - btrfs /dev/sda2 rw,subvolid=300,subvol=/@onedrive
";

const GOOD_FSTAB: &str =
    "UUID=aaaa /home/u/OneDrive btrfs subvol=/@onedrive,noatime,noauto,nofail 0 0\n";

fn good_observed() -> Observed {
    Observed {
        vantage: Vantage::Host,
        kernel: KernelSupport::Measured {
            release: "6.14.0".into(),
        },
        mountinfo: GOOD_TABLE.into(),
        fstab: GOOD_FSTAB.into(),
        secrets: SecretService::Owned,
        // The single-surface machine: no Plasma applet, so the tray unit is
        // the only tray and nothing has to be decided. Handed in rather than
        // probed so these tests cannot start depending on whether the
        // developer's own machine happens to have the applet installed.
        plasmoid: Plasmoid::Absent,
    }
}

fn payload_dir(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for b in [
        "hydrationd",
        "onedrive-hydration-daemon",
        "onedrive-hydrationctl",
        "onedrive-hydration-dbus",
        "onedrive-hydration-tray",
    ] {
        let p = bin.join(b);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

fn opts(prefix: &Path) -> Options {
    Options {
        prefix: prefix.to_path_buf(),
        dry_run: false,
        force: false,
        consent_fstab: false,
        tray: None,
    }
}

fn refusal<'a>(planned: &'a Planned, name: &str) -> &'a str {
    for c in &planned.checks {
        if c.name == name {
            if let Outcome::Refuse(m) = &c.outcome {
                return m;
            }
        }
    }
    panic!(
        "expected check {name:?} to refuse; checks were: {:#?}",
        planned.checks
    );
}

fn assert_no_files_under(dir: &Path) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else {
                panic!("refused install still wrote {}", e.path().display());
            }
        }
    }
}

// --- the user must exist -------------------------------------------------

#[test]
fn unknown_user_is_refused_by_fact_resolution() {
    let err = Facts::resolve(
        "no-such-user-e2ba7c",
        PathBuf::from("/x"),
        String::new(),
        PathBuf::from("/usr/local/bin"),
    )
    .unwrap_err();
    assert!(err.contains("does not exist"), "{err}");
    assert!(err.contains("uid"), "{err}");
}

// --- kernel support ------------------------------------------------------

#[test]
fn release_parsing_handles_real_suffixes() {
    assert_eq!(probes::parse_release("6.14.0-rc2-1-cachyos"), Some((6, 14)));
    assert_eq!(probes::parse_release("6.8.0-58-generic"), Some((6, 8)));
    assert_eq!(probes::parse_release("nonsense"), None);
}

#[test]
fn old_kernel_by_version_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let mut obs = good_observed();
    obs.kernel = probes::support_from_release(Some("6.8.0-58-generic".into()));
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let planned = install(
        &f,
        &Templates::default(),
        &obs,
        &opts(&tmp.path().join("p")),
    );
    let msg = refusal(&planned, "kernel");
    assert!(msg.contains("6.14"), "{msg}");
    assert!(msg.contains("zeros"), "{msg}");
    assert!(planned.actions.is_none());
}

#[test]
fn unparseable_kernel_is_a_refusal_not_a_guess() {
    let mut obs = good_observed();
    obs.kernel = probes::support_from_release(Some("surprise".into()));
    let tmp = tempfile::tempdir().unwrap();
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let planned = install(
        &f,
        &Templates::default(),
        &obs,
        &opts(&tmp.path().join("p")),
    );
    assert!(refusal(&planned, "kernel").contains("guess"));
}

#[test]
fn this_machines_kernel_probe_reaches_an_answer() {
    // Whatever the answer, the probe must land on a measurement or a version
    // inference — Unknown would mean the probe itself is broken.
    let support = probes::kernel_precontent_support();
    assert!(
        !matches!(support, KernelSupport::Unknown { .. }),
        "{support:?}"
    );
}

// --- the sync root must be its own mount, on an SB_I_ALLOW_HSM filesystem -

#[test]
fn plain_directory_is_refused_with_the_dirmark_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("OneDrive");
    std::fs::create_dir(&dir).unwrap();
    let mut obs = good_observed();
    // The real table: this directory genuinely is not a mount point on it.
    obs.mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap();
    let f = facts(dir.to_str().unwrap(), &payload_dir(tmp.path()));
    let planned = install(
        &f,
        &Templates::default(),
        &obs,
        &opts(&tmp.path().join("p")),
    );
    let msg = refusal(&planned, "sync-root-mount");
    assert!(msg.contains("not a mount point"), "{msg}");
    assert!(msg.contains("delivers nothing"), "{msg}");
    // The storage command is printed, never run.
    assert!(msg.contains("btrfs subvolume create"), "{msg}");
    assert!(planned.actions.is_none());
}

#[test]
fn tmpfs_sync_root_is_refused_by_name() {
    // Find a real tmpfs in the real table rather than hardcoding one.
    let table = std::fs::read_to_string("/proc/self/mountinfo").unwrap();
    let rows = probes::parse_mountinfo(&table);
    let tmpfs = rows
        .iter()
        .find(|r| r.fstype == "tmpfs")
        .expect("no tmpfs mounted anywhere?");
    let tmp = tempfile::tempdir().unwrap();
    let mut obs = good_observed();
    obs.mountinfo = table.clone();
    let f = facts(&tmpfs.point.clone(), &payload_dir(tmp.path()));
    let planned = install(
        &f,
        &Templates::default(),
        &obs,
        &opts(&tmp.path().join("p")),
    );
    let msg = refusal(&planned, "sync-root-fstype");
    assert!(msg.contains("tmpfs"), "{msg}");
    assert!(msg.contains("SB_I_ALLOW_HSM"), "{msg}");
}

// --- §6.4a: nothing else may mount those files ---------------------------

#[test]
fn mounted_filesystem_root_exposing_the_subvolume_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let mut obs = good_observed();
    // The §6.4a bypass measured in DESIGN.md: the btrfs top-level (subvol /)
    // mounted anywhere exposes every subvolume, including ours.
    obs.mountinfo = format!(
        "{GOOD_TABLE}99 40 0:33 / /mnt/top rw shared:9 - btrfs /dev/sda2 rw,subvolid=5,subvol=/\n"
    );
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let planned = install(
        &f,
        &Templates::default(),
        &obs,
        &opts(&tmp.path().join("p")),
    );
    let msg = refusal(&planned, "exposure");
    assert!(msg.contains("/mnt/top"), "{msg}");
    assert!(msg.contains("6.4a"), "{msg}");
    assert!(msg.contains("detected"), "{msg}");
}

#[test]
fn exposure_scan_is_symmetric_and_ignores_sibling_subvolumes() {
    let rows = probes::parse_mountinfo(&format!(
        "{GOOD_TABLE}99 40 0:33 /@onedrive/Documents /srv/docs rw shared:9 - btrfs /dev/sda2 rw,subvol=/@onedrive/Documents\n"
    ));
    let ours = probes::find_mount(&rows, Path::new("/home/u/OneDrive")).unwrap();
    // A bind of one of our subdirectories elsewhere is a bypass...
    assert_eq!(probes::exposures(&rows, ours), vec!["/srv/docs"]);
    // ...but sibling subvolumes (@, @home) sharing the device are not.
    let rows = probes::parse_mountinfo(GOOD_TABLE);
    let ours = probes::find_mount(&rows, Path::new("/home/u/OneDrive")).unwrap();
    assert!(probes::exposures(&rows, ours).is_empty());
}

// --- fstab: never automatic, never edited without consent ----------------

#[test]
fn fstab_entry_without_noauto_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let mut obs = good_observed();
    obs.fstab = "UUID=aaaa /home/u/OneDrive btrfs subvol=/@onedrive,noatime 0 0\n".into();
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let planned = install(
        &f,
        &Templates::default(),
        &obs,
        &opts(&tmp.path().join("p")),
    );
    let msg = refusal(&planned, "fstab");
    assert!(msg.contains("noauto"), "{msg}");
    assert!(msg.contains("boot"), "{msg}");
}

#[test]
fn missing_fstab_entry_is_refused_without_consent_and_appended_with_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mut obs = good_observed();
    obs.fstab = String::new();
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let planned = install(
        &f,
        &Templates::default(),
        &obs,
        &opts(&tmp.path().join("p")),
    );
    let msg = refusal(&planned, "fstab");
    assert!(msg.contains("--consent-fstab"), "{msg}");
    assert!(msg.contains("noauto"), "{msg}");

    let mut consent = opts(&tmp.path().join("p"));
    consent.consent_fstab = true;
    let planned = install(&f, &Templates::default(), &obs, &consent);
    let actions = planned.actions.expect("consent should clear the refusal");
    let line = actions
        .iter()
        .find_map(|a| match a {
            Action::AppendFstab { line, .. } => Some(line.clone()),
            _ => None,
        })
        .expect("consented install should plan the fstab append");
    // There is no code path that composes a line without noauto.
    assert!(line.contains("noauto"), "{line}");
    assert!(line.contains("nofail"), "{line}");
}

#[test]
fn fstab_suggestion_carries_subvolume_and_noauto() {
    let rows = probes::parse_mountinfo(GOOD_TABLE);
    let ours = probes::find_mount(&rows, Path::new("/home/u/OneDrive")).unwrap();
    let line = probes::fstab_suggestion(ours);
    assert!(line.contains("subvol=/@onedrive"), "{line}");
    assert!(line.contains("noauto"), "{line}");
}

// --- Secret Service ------------------------------------------------------

#[test]
fn dead_session_bus_is_refused_by_the_real_probe() {
    // No bus socket at all.
    let tmp = tempfile::tempdir().unwrap();
    let s = secret_service_at("u", 1234, &tmp.path().join("no-bus"));
    let SecretService::Unreachable { detail } = s else {
        panic!("expected Unreachable, got {s:?}");
    };
    assert!(detail.contains("no session bus"), "{detail}");

    // A path that exists but is not a bus: the actual busctl invocation runs
    // and fails. Using our own euid keeps the probe on its direct path.
    let dead = tmp.path().join("bus");
    std::fs::write(&dead, "").unwrap();
    let uid = unsafe { libc::geteuid() };
    let s = secret_service_at("whoever", uid, &dead);
    assert!(
        matches!(s, SecretService::Unreachable { .. }),
        "expected Unreachable, got {s:?}"
    );
}

#[test]
fn unreachable_secret_service_refuses_install() {
    let tmp = tempfile::tempdir().unwrap();
    let mut obs = good_observed();
    obs.secrets = SecretService::Unreachable {
        detail: "no session bus at /run/user/1234/bus".into(),
    };
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let planned = install(
        &f,
        &Templates::default(),
        &obs,
        &opts(&tmp.path().join("p")),
    );
    let msg = refusal(&planned, "secret-service");
    assert!(msg.contains("fails closed"), "{msg}");
    assert!(planned.actions.is_none());
}

// --- payload binaries ----------------------------------------------------

#[test]
fn missing_binaries_are_refused_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let empty = tmp.path().join("empty-bin");
    std::fs::create_dir(&empty).unwrap();
    let f = facts("/home/u/OneDrive", &empty);
    let planned = install(
        &f,
        &Templates::default(),
        &good_observed(),
        &opts(&tmp.path().join("p")),
    );
    let msg = refusal(&planned, "binaries");
    assert!(msg.contains("hydrationd"), "{msg}");
    assert!(msg.contains("ExecStart"), "{msg}");
}

// --- two tray surfaces: a decision, never a guess -------------------------

/// Stage a Plasma applet package where `kpackagetool6` would put it for this
/// user, under a rehearsal prefix.
fn stage_plasmoid(prefix: &Path) -> PathBuf {
    let pkg = prefix
        .join("home/u/.local/share/plasma/plasmoids")
        .join(probes::PLASMOID_ID);
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("metadata.json"), "{}\n").unwrap();
    pkg
}

fn present() -> Plasmoid {
    Plasmoid::Present {
        path: PathBuf::from("/home/u/.local/share/plasma/plasmoids").join(probes::PLASMOID_ID),
    }
}

fn tray_unit_path(prefix: &Path) -> PathBuf {
    prefix
        .join("home/u/.config/systemd/user")
        .join(units::TRAY_UNIT)
}

fn tray_link_path(prefix: &Path) -> PathBuf {
    prefix
        .join("home/u/.config/systemd/user/graphical-session.target.wants")
        .join(units::TRAY_UNIT)
}

#[test]
fn the_plasmoid_probe_reads_a_package_on_disk_not_a_running_shell() {
    let tmp = tempfile::tempdir().unwrap();
    let home = Path::new("/home/u");

    // The user's own data directory is searched before the system tree: that
    // is where install-plasmoid.sh puts it, and where kpackagetool6 --show
    // reported it on the measured machine.
    let dirs = probes::plasmoid_dirs(tmp.path(), home);
    assert!(
        dirs[0].ends_with("home/u/.local/share/plasma/plasmoids"),
        "{dirs:?}"
    );
    assert!(dirs[1].ends_with("usr/share/plasma/plasmoids"), "{dirs:?}");

    assert_eq!(probes::plasmoid_package(tmp.path(), home), Plasmoid::Absent);

    // A bare directory is not an installed applet — kpackagetool6 requires
    // metadata.json, and a leftover empty directory must not be able to cause
    // a refusal.
    let pkg = tmp
        .path()
        .join("home/u/.local/share/plasma/plasmoids")
        .join(probes::PLASMOID_ID);
    std::fs::create_dir_all(&pkg).unwrap();
    assert_eq!(probes::plasmoid_package(tmp.path(), home), Plasmoid::Absent);

    std::fs::write(pkg.join("metadata.json"), "{}\n").unwrap();
    assert_eq!(
        probes::plasmoid_package(tmp.path(), home),
        Plasmoid::Present { path: pkg }
    );

    // A distribution package installs the same tree system-wide; found too.
    let tmp2 = tempfile::tempdir().unwrap();
    let sys = tmp2
        .path()
        .join("usr/share/plasma/plasmoids")
        .join(probes::PLASMOID_ID);
    std::fs::create_dir_all(&sys).unwrap();
    std::fs::write(sys.join("metadata.json"), "{}\n").unwrap();
    assert_eq!(
        probes::plasmoid_package(tmp2.path(), home),
        Plasmoid::Present { path: sys }
    );
}

#[test]
fn both_surfaces_installed_is_refused_until_one_is_named() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("p");
    std::fs::create_dir_all(&prefix).unwrap();
    let mut obs = good_observed();
    obs.plasmoid = present();
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));

    let planned = install(&f, &Templates::default(), &obs, &opts(&prefix));
    let msg = refusal(&planned, "tray-surface");
    // The consequence, named.
    assert!(msg.contains("two identical icons"), "{msg}");
    // Both surfaces, and where the applet actually is.
    assert!(msg.contains(units::TRAY_UNIT), "{msg}");
    assert!(msg.contains(".local/share/plasma/plasmoids"), "{msg}");
    // Every way out, including the one that is not this tool's to take.
    assert!(msg.contains("--tray plasmoid"), "{msg}");
    assert!(msg.contains("--tray sni"), "{msg}");
    assert!(msg.contains("--tray none"), "{msg}");
    assert!(msg.contains("kpackagetool6"), "{msg}");
    // And why it is asking rather than deciding.
    assert!(msg.contains("not something this tool can know"), "{msg}");

    assert!(planned.actions.is_none());
    assert_no_files_under(&prefix);

    // And it is the *only* refusal: while the surface is undecided, no other
    // check may assert something that depends on the decision. With the tray
    // binary missing from the payload this would otherwise also refuse for a
    // binary belonging to a unit nobody has chosen to install.
    let bin = payload_dir(&tmp.path().join("no-tray"));
    std::fs::remove_file(bin.join("onedrive-hydration-tray")).unwrap();
    let f = facts("/home/u/OneDrive", &bin);
    let planned = install(&f, &Templates::default(), &obs, &opts(&prefix));
    assert_eq!(
        planned.refusals().len(),
        1,
        "only the open question should refuse: {:#?}",
        planned.checks
    );
}

#[test]
fn naming_the_applet_drops_the_tray_unit_and_retires_one_already_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("p");
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));

    // An ordinary single-surface install first: the tray unit and its
    // enablement link land in the prefix.
    let planned = install(&f, &Templates::default(), &good_observed(), &opts(&prefix));
    let (_, r) = execute(
        planned.actions.as_ref().unwrap(),
        ExecMode {
            write_files: true,
            run_commands: false,
        },
    );
    r.unwrap();
    assert!(tray_unit_path(&prefix).is_file());
    assert!(std::fs::symlink_metadata(tray_link_path(&prefix)).is_ok());

    // Then the applet is installed and the operator answers the refusal.
    let mut obs = good_observed();
    obs.plasmoid = present();
    let mut o = opts(&prefix);
    o.tray = Some(Tray::Plasmoid);
    let planned = install(&f, &Templates::default(), &obs, &o);
    assert_eq!(planned.refusals(), Vec::<&str>::new());
    let actions = planned
        .actions
        .expect("naming a surface clears the refusal");

    // Nothing writes the tray unit any more...
    assert!(
        !actions.iter().any(|a| matches!(
            a,
            Action::WriteFile { path, .. } if path.ends_with(units::TRAY_UNIT)
        )),
        "{actions:#?}"
    );
    // ...and the one already there is stopped before it is deleted, or the
    // second icon outlives its own unit file until the session ends.
    let pos = |pred: &dyn Fn(&Action) -> bool| actions.iter().position(pred);
    let disable = pos(&|a| {
        matches!(a, Action::Run { argv, .. }
            if argv.join(" ").contains("disable") && argv.join(" ").contains(units::TRAY_UNIT))
    })
    .expect("must stop the running tray");
    let remove =
        pos(&|a| matches!(a, Action::RemoveFile { path } if path.ends_with(units::TRAY_UNIT)))
            .expect("must remove the tray unit");
    assert!(disable < remove, "stop before delete: {actions:#?}");

    let (log, r) = execute(
        &actions,
        ExecMode {
            write_files: true,
            run_commands: false,
        },
    );
    r.unwrap();
    assert!(!tray_unit_path(&prefix).exists());
    assert!(std::fs::symlink_metadata(tray_link_path(&prefix)).is_err());
    // The next steps name the applet's own installer, not a unit this run
    // deliberately did not install.
    assert!(
        log.iter().any(|l| l.contains("install-plasmoid.sh")),
        "{log:#?}"
    );
    assert!(
        !log.iter()
            .any(|l| l.contains("systemctl --user start onedrive-hydration-tray")),
        "{log:#?}"
    );
}

#[test]
fn retiring_a_tray_unit_that_was_never_installed_runs_no_command() {
    // The gate that keeps --tray plasmoid from aborting on a machine that
    // never had the unit: `systemctl disable --now` on a missing unit fails,
    // and a failed Run stops the whole install.
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("p");
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let mut o = opts(&prefix);
    o.tray = Some(Tray::Plasmoid);
    let mut obs = good_observed();
    obs.plasmoid = present();

    let planned = install(&f, &Templates::default(), &obs, &o);
    let actions = planned.actions.unwrap();
    assert!(
        !actions.iter().any(|a| matches!(
            a,
            Action::Run { argv, .. } if argv.join(" ").contains(units::TRAY_UNIT)
        )),
        "nothing to disable, so no command: {actions:#?}"
    );
}

#[test]
fn asking_for_both_is_allowed_but_says_so_out_loud() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("p");
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let mut obs = good_observed();
    obs.plasmoid = present();
    let mut o = opts(&prefix);
    o.tray = Some(Tray::Sni);

    let planned = install(&f, &Templates::default(), &obs, &o);
    assert_eq!(planned.refusals(), Vec::<&str>::new());
    let note = planned
        .checks
        .iter()
        .find(|c| c.name == "tray-surface")
        .map(|c| c.outcome.clone())
        .unwrap();
    let Outcome::Caveat(msg) = note else {
        panic!("an explicit --tray sni next to the applet must still be a caveat: {note:?}");
    };
    assert!(msg.contains("both will show an icon"), "{msg}");
    // Deliberate is deliberate: the unit is still installed.
    assert!(planned
        .actions
        .unwrap()
        .iter()
        .any(|a| matches!(a, Action::WriteFile { path, .. } if path.ends_with(units::TRAY_UNIT))));
}

#[test]
fn choosing_the_applet_without_one_installed_warns_and_names_the_real_home() {
    let tmp = tempfile::tempdir().unwrap();
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let mut o = opts(&tmp.path().join("p"));
    o.tray = Some(Tray::Plasmoid);

    let planned = install(&f, &Templates::default(), &good_observed(), &o);
    let Some(Outcome::Caveat(msg)) = planned
        .checks
        .iter()
        .find(|c| c.name == "tray-surface")
        .map(|c| c.outcome.clone())
    else {
        panic!("expected a caveat: {:#?}", planned.checks);
    };
    assert!(msg.contains("no tray at all"), "{msg}");
    // The resolved home, never a guessed /home/<user>.
    assert!(
        msg.contains("/home/u/.local/share/plasma/plasmoids"),
        "{msg}"
    );
}

#[test]
fn tray_none_installs_no_tray_unit_and_says_what_is_lost() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("p");
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let mut o = opts(&prefix);
    o.tray = Some(Tray::None);

    let planned = install(&f, &Templates::default(), &good_observed(), &o);
    assert_eq!(planned.refusals(), Vec::<&str>::new());
    let Some(Outcome::Caveat(msg)) = planned
        .checks
        .iter()
        .find(|c| c.name == "tray-surface")
        .map(|c| c.outcome.clone())
    else {
        panic!("expected a caveat: {:#?}", planned.checks);
    };
    // The exposure warning is the one thing with no other surface (§6.4a).
    assert!(msg.contains("exposure"), "{msg}");

    let (_, r) = execute(
        planned.actions.as_ref().unwrap(),
        ExecMode {
            write_files: true,
            run_commands: false,
        },
    );
    r.unwrap();
    assert!(!tray_unit_path(&prefix).exists());
    // The rest of the deployment is untouched — including the state service's
    // activation file, which the applet and the CLI both still need.
    assert!(prefix
        .join("home/u/.config/systemd/user/onedrive-hydration.service")
        .is_file());
    assert!(prefix
        .join("home/u/.local/share/dbus-1/services/io.github.franzjeger.OneDriveHydration.service")
        .is_file());
}

#[test]
fn the_tray_binary_is_required_only_when_a_unit_points_at_it() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = payload_dir(tmp.path());
    std::fs::remove_file(bin.join("onedrive-hydration-tray")).unwrap();
    let f = facts("/home/u/OneDrive", &bin);

    // Default surface: a unit names it, so its absence is the usual refusal.
    let planned = install(
        &f,
        &Templates::default(),
        &good_observed(),
        &opts(&tmp.path().join("p")),
    );
    let msg = refusal(&planned, "binaries");
    assert!(msg.contains("onedrive-hydration-tray"), "{msg}");

    // Applet surface: nothing points at it, so demanding it would be a
    // refusal with no reason behind it.
    let mut o = opts(&tmp.path().join("p"));
    o.tray = Some(Tray::Plasmoid);
    let planned = install(&f, &Templates::default(), &good_observed(), &o);
    assert_eq!(planned.refusals(), Vec::<&str>::new());
}

#[test]
fn the_id_the_refusal_searches_for_is_the_package_that_actually_ships() {
    // A refusal keyed to a stale name is a refusal that can never fire, which
    // is the one thing this file exists to rule out. Rename the package tree
    // (or its Id) without renaming the constant and the installer looks for an
    // applet nobody installs, finds nothing, and cheerfully writes the second
    // icon. Pinned against the shipped tree rather than trusted.
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/plasmoid")
        .join(probes::PLASMOID_ID);
    let metadata = shipped.join("metadata.json");
    assert!(
        metadata.is_file(),
        "no shipped applet at {} — probes::PLASMOID_ID and packaging/plasmoid/ have \
         drifted apart, and the tray-surface refusal can no longer fire",
        shipped.display()
    );
    // kpackagetool6 installs under KPlugin.Id, not under the directory name,
    // so the directory agreeing is not enough on its own.
    let text = std::fs::read_to_string(&metadata).unwrap();
    assert!(
        text.contains(&format!("\"Id\": \"{}\"", probes::PLASMOID_ID)),
        "the applet's KPlugin.Id is what kpackagetool6 installs under, and it is not \
         {}:\n{text}",
        probes::PLASMOID_ID
    );
}

#[test]
fn uninstalling_a_deployment_that_has_no_tray_unit_names_none() {
    // Measured: `systemctl --user disable --now <absent unit>` exits 1, and a
    // failed Run aborts the plan — which here would happen *after* the
    // unmount. A deployment installed with --tray plasmoid has no tray unit,
    // so uninstall must not name one.
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("p");
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let mut o = opts(&prefix);
    o.tray = Some(Tray::Plasmoid);

    let planned = install(&f, &Templates::default(), &good_observed(), &o);
    let (_, r) = execute(
        planned.actions.as_ref().unwrap(),
        ExecMode {
            write_files: true,
            run_commands: false,
        },
    );
    r.unwrap();
    assert!(!tray_unit_path(&prefix).exists());

    let planned = uninstall(&f, false, false, &opts(&prefix));
    let disable = planned
        .actions
        .unwrap()
        .into_iter()
        .find_map(|a| match a {
            Action::Run { argv, .. } if argv.contains(&"disable".to_string()) => Some(argv),
            _ => None,
        })
        .expect("uninstall must disable the user half");
    assert!(
        !disable.contains(&units::TRAY_UNIT.to_string()),
        "no tray unit was installed, so naming it would abort the uninstall: {disable:?}"
    );
    // The units that are always there are still named.
    assert!(disable.contains(&"onedrive-hydration.service".to_string()));
    assert!(disable.contains(&"onedrive-hydration-dbus.service".to_string()));

    // And when one *is* installed, it is still named — the gate must not turn
    // into "never disable the tray".
    let sni = tmp.path().join("q");
    let planned = install(&f, &Templates::default(), &good_observed(), &opts(&sni));
    let (_, r) = execute(
        planned.actions.as_ref().unwrap(),
        ExecMode {
            write_files: true,
            run_commands: false,
        },
    );
    r.unwrap();
    let planned = uninstall(&f, false, false, &opts(&sni));
    let disable = planned
        .actions
        .unwrap()
        .into_iter()
        .find_map(|a| match a {
            Action::Run { argv, .. } if argv.contains(&"disable".to_string()) => Some(argv),
            _ => None,
        })
        .unwrap();
    assert!(
        disable.contains(&units::TRAY_UNIT.to_string()),
        "{disable:?}"
    );
}

#[test]
fn a_rehearsal_still_sees_an_applet_the_prefix_does_not_stage() {
    // The fallback `observe` applies: prefix first, then the machine. Made
    // testable by pointing `home` at a directory this test owns — searching
    // from "/" then lands inside it, so "the machine's real answer" is one the
    // test controls instead of one the developer's desktop decides.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("fakehome");
    let empty_prefix = tmp.path().join("empty");
    std::fs::create_dir_all(&empty_prefix).unwrap();

    // Staged on "the machine", nothing under the prefix: the rehearsal must
    // still find it, or an operator rehearsing before the real run is told
    // there is no collision and then hits the refusal with sudo already typed.
    let real = home
        .join(".local/share/plasma/plasmoids")
        .join(probes::PLASMOID_ID);
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("metadata.json"), "{}\n").unwrap();
    assert_eq!(
        probes::plasmoid_observed(&empty_prefix, &home),
        Plasmoid::Present { path: real }
    );

    // A prefix that stages its own answer wins over the machine's, so a test
    // rig can rehearse a machine it is not running on.
    let staged_prefix = tmp.path().join("staged");
    let staged = stage_plasmoid(&staged_prefix);
    assert_eq!(
        probes::plasmoid_observed(&staged_prefix, Path::new("/home/u")),
        Plasmoid::Present { path: staged }
    );
}

// --- the namespace-directive scan, against a deliberately bad template ---

#[test]
fn each_measured_directive_is_caught_even_disabled_or_indented() {
    for d in units::MEASURED_NAMESPACE_DIRECTIVES {
        for form in [format!("{d}=yes"), format!("{d}=no"), format!("  {d}=yes")] {
            let hits = units::namespace_directives(&form);
            assert_eq!(hits.len(), 1, "{form} should be caught");
            assert_eq!(hits[0].1, d);
        }
    }
    // Comments naming the directives are prose, not configuration.
    assert!(units::namespace_directives("# PrivateTmp=yes\n; PrivateNetwork=yes").is_empty());
    // The documented namespacing family is refused too.
    assert_eq!(units::namespace_directives("PrivateMounts=yes").len(), 1);
}

#[test]
fn bad_template_is_refused_by_the_full_flow_and_nothing_is_written() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("p");
    std::fs::create_dir(&prefix).unwrap();
    // The historical mistake, replayed on purpose: "harden" the helper unit.
    let base = Templates::default();
    let templates = Templates {
        hydrationd_service: base.hydrationd_service.replace(
            "NoNewPrivileges=yes",
            "NoNewPrivileges=yes\nPrivateNetwork=yes",
        ),
        ..base
    };
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let planned = install(&f, &templates, &good_observed(), &opts(&prefix));
    let msg = refusal(&planned, "unit-text");
    assert!(msg.contains("PrivateNetwork"), "{msg}");
    assert!(msg.contains("hydrationd.service"), "{msg}");
    assert!(msg.contains("private mount namespace"), "{msg}");
    assert!(planned.actions.is_none());
    assert_no_files_under(&prefix);
}

#[test]
fn shipped_templates_pass_their_own_scan_and_the_sync_unit_is_out_of_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let rendered = units::render(&Templates::default(), &f, Tray::Sni);
    for unit in rendered.system.iter() {
        assert!(
            units::namespace_directives(&unit.text).is_empty(),
            "{} carries a namespace directive",
            unit.name
        );
    }
    // The sync daemon's unit keeps its verified PrivateTmp= sandboxing and is
    // deliberately outside the helper scan's scope.
    let sync = &rendered.user[0];
    assert!(sync.text.contains("PrivateTmp=yes"));
    assert!(!units::must_share_host_namespace(&sync.name));
    assert!(units::must_share_host_namespace("hydrationd.service"));
}

// --- rendering: concrete facts, no leftovers -----------------------------

#[test]
fn rendered_units_carry_the_facts_and_no_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let rendered = units::render(&Templates::default(), &f, Tray::Sni);
    let helper = &rendered.system[0].text;
    assert!(helper.contains("--mount /home/u/OneDrive"));
    assert!(helper.contains("--socket /run/user/1234/onedrive-hydration.sock"));
    assert!(helper.contains("--peer-uid 1234"));
    let path_unit = &rendered.system[1].text;
    assert!(path_unit.contains("PathExists=/run/user/1234/onedrive-hydration.sock"));
    // User units use the manager's own specifiers when the facts sit where
    // the specifier points — matching the hand-verified reference.
    let sync = &rendered.user[0].text;
    assert!(sync.contains("--mount %h/OneDrive"));
    assert!(sync.contains("--socket %t/onedrive-hydration.sock"));
    assert!(sync.contains("--client-id 11111111-2222-3333-4444-555555555555"));
    for unit in rendered.all() {
        for token in [
            "@USER@",
            "@UID@",
            "@HOME@",
            "@MOUNT@",
            "@MOUNT_USER@",
            "@SOCKET@",
            "@SOCKET_USER@",
            "@CLIENT_ID@",
            "@BIN@",
        ] {
            assert!(
                !unit.text.contains(token),
                "{} still contains {token}",
                unit.name
            );
        }
    }
}

#[test]
fn mount_outside_home_is_rendered_literally() {
    let tmp = tempfile::tempdir().unwrap();
    let f = facts("/srv/onedrive", &payload_dir(tmp.path()));
    let rendered = units::render(&Templates::default(), &f, Tray::Sni);
    assert!(rendered.user[0].text.contains("--mount /srv/onedrive"));
}

// --- the state service: activated by the bus, not started eagerly ---------

/// One `Key=value` line of a KeyFile/unit body, comments skipped — the same
/// reading the consumers (the bus, systemd) apply.
fn keyed(text: &str, key: &str) -> Option<String> {
    text.lines()
        .map(str::trim_start)
        .filter(|l| !l.starts_with('#') && !l.starts_with(';'))
        .find_map(|l| l.strip_prefix(key).map(|v| v.trim().to_string()))
}

#[test]
fn dbus_activation_file_agrees_with_the_unit_it_starts() {
    let tmp = tempfile::tempdir().unwrap();
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let rendered = units::render(&Templates::default(), &f, Tray::Sni);
    let activation = &rendered.bus_services[0];
    assert_eq!(
        activation.name,
        "io.github.franzjeger.OneDriveHydration.service"
    );

    let unit = rendered
        .user
        .iter()
        .find(|u| u.name == "onedrive-hydration-dbus.service")
        .expect("the unit the activation file names must be generated too");

    // Same name, same binary, same unit — generated from the same facts, so
    // a template edit cannot leave the bus starting one thing while systemd
    // describes another.
    assert_eq!(
        keyed(&activation.text, "Name=").as_deref(),
        keyed(&unit.text, "BusName=").as_deref(),
        "the activation file and the unit must claim the same bus name"
    );
    assert_eq!(
        keyed(&activation.text, "SystemdService=").as_deref(),
        Some(unit.name.as_str())
    );
    assert_eq!(
        keyed(&activation.text, "Exec=").as_deref(),
        keyed(&unit.text, "ExecStart=").as_deref(),
        "bus-fallback Exec= and the unit's ExecStart= must run the same binary"
    );

    // Eager start is gone on purpose: no [Install], no WantedBy.
    assert!(
        keyed(&unit.text, "WantedBy=").is_none(),
        "the state service must not carry an eager enablement:\n{}",
        unit.text
    );
}

#[test]
fn a_drifted_activation_file_is_refused_without_force() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("p");
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let o = opts(&prefix);

    let planned = install(&f, &Templates::default(), &good_observed(), &o);
    let (_, r) = execute(
        planned.actions.as_ref().unwrap(),
        ExecMode {
            write_files: true,
            run_commands: false,
        },
    );
    r.unwrap();

    // A deployment whose activation file points somewhere else must not be
    // silently repointed: the collision refusal covers the bus's file the
    // same way it covers the units.
    let path = prefix
        .join("home/u/.local/share/dbus-1/services/io.github.franzjeger.OneDriveHydration.service");
    std::fs::write(
        &path,
        "[D-BUS Service]\nName=io.github.franzjeger.OneDriveHydration\nExec=/somewhere/else\n",
    )
    .unwrap();
    let planned = install(&f, &Templates::default(), &good_observed(), &o);
    let msg = refusal(&planned, "collision");
    assert!(msg.contains("dbus-1/services"), "{msg}");
    assert!(msg.contains("--force"), "{msg}");
    assert!(planned.actions.is_none());

    let mut forced = o.clone();
    forced.force = true;
    let planned = install(&f, &Templates::default(), &good_observed(), &forced);
    let (_, r) = execute(
        planned.actions.as_ref().unwrap(),
        ExecMode {
            write_files: true,
            run_commands: false,
        },
    );
    r.unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("SystemdService=onedrive-hydration-dbus.service"));
}

// --- installation, idempotence, drift -----------------------------------

#[test]
fn install_writes_units_links_and_is_idempotent_until_forced() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("p");
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let o = opts(&prefix);

    // First run: everything is written.
    let planned = install(&f, &Templates::default(), &good_observed(), &o);
    assert_eq!(planned.refusals(), Vec::<&str>::new());
    let (_, r) = execute(
        planned.actions.as_ref().unwrap(),
        ExecMode {
            write_files: true,
            run_commands: false,
        },
    );
    r.unwrap();
    let sys = prefix.join("etc/systemd/system");
    let usr = prefix.join("home/u/.config/systemd/user");
    assert!(sys.join("hydrationd.service").is_file());
    assert!(sys.join("hydrationd.path").is_file());
    // The helper service has no [Install] and therefore no .wants link — the
    // path unit is its only trigger; the path unit is the one enabled.
    assert!(!sys
        .join("multi-user.target.wants/hydrationd.service")
        .exists());
    let path_link = sys.join("multi-user.target.wants/hydrationd.path");
    assert_eq!(
        std::fs::read_link(&path_link).unwrap(),
        Path::new("/etc/systemd/system/hydrationd.path"),
        "enablement links point at runtime paths, never prefixed ones"
    );
    assert!(usr.join("onedrive-hydration.service").is_file());
    assert!(usr.join("onedrive-hydration-dbus.service").is_file());
    assert!(usr.join("onedrive-hydration-tray.service").is_file());
    assert!(usr
        .join("default.target.wants/onedrive-hydration.service")
        .read_link()
        .is_ok());
    assert!(usr
        .join("graphical-session.target.wants/onedrive-hydration-tray.service")
        .read_link()
        .is_ok());
    // The state service is D-Bus-activated: its activation file lands where
    // the session bus looks, and there is deliberately no enablement link —
    // an eager start would run it for sessions with no subscriber.
    let bus = prefix.join("home/u/.local/share/dbus-1/services");
    assert!(bus
        .join("io.github.franzjeger.OneDriveHydration.service")
        .is_file());
    assert!(
        !usr.join("default.target.wants/onedrive-hydration-dbus.service")
            .exists(),
        "the state service must not be started eagerly; activation is the trigger"
    );

    // Second run: same facts, no rewrites, no refusals.
    let planned = install(&f, &Templates::default(), &good_observed(), &o);
    let actions = planned.actions.expect("idempotent rerun must not refuse");
    assert!(
        actions
            .iter()
            .all(|a| !matches!(a, Action::WriteFile { .. } | Action::Symlink { .. })),
        "second run should find everything unchanged: {actions:#?}"
    );

    // A drifted file: refuse without --force, rewrite with it.
    std::fs::write(sys.join("hydrationd.service"), "[Unit]\n# drifted\n").unwrap();
    let planned = install(&f, &Templates::default(), &good_observed(), &o);
    let msg = refusal(&planned, "collision");
    assert!(msg.contains("--force"), "{msg}");
    assert!(planned.actions.is_none());

    let mut forced = o.clone();
    forced.force = true;
    let planned = install(&f, &Templates::default(), &good_observed(), &forced);
    let (_, r) = execute(
        planned.actions.as_ref().unwrap(),
        ExecMode {
            write_files: true,
            run_commands: false,
        },
    );
    r.unwrap();
    let text = std::fs::read_to_string(sys.join("hydrationd.service")).unwrap();
    assert!(text.contains("--peer-uid 1234"));
}

#[test]
fn dry_run_writes_nothing_at_all() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("p");
    std::fs::create_dir(&prefix).unwrap();
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let mut o = opts(&prefix);
    o.dry_run = true;
    let planned = install(&f, &Templates::default(), &good_observed(), &o);
    let (log, r) = execute(planned.actions.as_ref().unwrap(), ExecMode::from(&o));
    r.unwrap();
    assert!(log.iter().any(|l| l.starts_with("would write")));
    assert_no_files_under(&prefix);
}

// --- uninstall: the way back must not fail open --------------------------

#[test]
fn uninstall_refuses_while_mounted_without_explicit_unmount() {
    let tmp = tempfile::tempdir().unwrap();
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let planned = uninstall(&f, true, false, &opts(&tmp.path().join("p")));
    let msg = refusal(&planned, "mount-safety");
    assert!(msg.contains("--and-unmount"), "{msg}");
    assert!(msg.contains("zeros"), "{msg}");
    assert!(planned.actions.is_none());
}

#[test]
fn uninstall_with_unmount_never_stops_the_helper_under_a_live_mount() {
    let tmp = tempfile::tempdir().unwrap();
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));
    let planned = uninstall(&f, true, true, &opts(&tmp.path().join("p")));
    let actions = planned.actions.unwrap();

    let pos = |pred: &dyn Fn(&Action) -> bool| actions.iter().position(pred);
    let stop_path = pos(&|a| {
        matches!(a, Action::Run { argv, .. } if argv.join(" ") == "systemctl stop hydrationd.path")
    })
    .expect("must disarm the trigger");
    let umount = pos(&|a| matches!(a, Action::Run { argv, .. } if argv[0] == "umount"))
        .expect("must unmount");
    let verify = pos(&|a| matches!(a, Action::VerifyUnmounted { .. })).expect("must verify");
    let first_removal =
        pos(&|a| matches!(a, Action::RemoveFile { .. })).expect("must remove units");

    assert!(stop_path < umount, "trigger is disarmed before the unmount");
    assert!(umount < verify && verify < first_removal);
    // The one forbidden move: stopping the helper while its mount is up.
    assert!(
        !actions.iter().any(|a| matches!(
            a,
            Action::Run { argv, .. } if argv.join(" ").contains("stop hydrationd.service")
        )),
        "the umount's stop job reaches the helper via RequiresMountsFor=; \
         stopping it directly would open a fail-open window: {actions:#?}"
    );
}

#[test]
fn uninstall_when_unmounted_stops_the_helper_directly_and_removes_files() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = tmp.path().join("p");
    let f = facts("/home/u/OneDrive", &payload_dir(tmp.path()));

    // Install first, then take it back out, all inside the prefix.
    let planned = install(&f, &Templates::default(), &good_observed(), &opts(&prefix));
    let (_, r) = execute(
        planned.actions.as_ref().unwrap(),
        ExecMode {
            write_files: true,
            run_commands: false,
        },
    );
    r.unwrap();

    let planned = uninstall(&f, false, false, &opts(&prefix));
    let actions = planned.actions.unwrap();
    assert!(actions.iter().any(|a| matches!(
        a,
        Action::Run { argv, .. } if argv.join(" ").contains("stop hydrationd.service")
    )));
    let (log, r) = execute(
        &actions,
        ExecMode {
            write_files: true,
            run_commands: false,
        },
    );
    r.unwrap();
    // Commands were narrated, not run.
    assert!(log.iter().any(|l| l.starts_with("would run systemctl")));
    assert!(!prefix
        .join("etc/systemd/system/hydrationd.service")
        .exists());
    assert!(!prefix.join("etc/systemd/system/hydrationd.path").exists());
    assert!(!prefix
        .join("home/u/.config/systemd/user/onedrive-hydration.service")
        .exists());
    // The activation file goes with the units: a leftover would let the bus
    // keep starting a service whose binary uninstall just orphaned.
    assert!(!prefix
        .join("home/u/.local/share/dbus-1/services/io.github.franzjeger.OneDriveHydration.service")
        .exists());
    // What stays is stated, not silently skipped.
    assert!(log.iter().any(|l| l.contains("left in place on purpose")));
}

// --- the CLI surface ------------------------------------------------------

#[test]
fn cli_refuses_unknown_user_with_exit_code_one() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_onedrive-hydration-install"))
        .args([
            "uninstall",
            "--user",
            "no-such-user-e2ba7c",
            "--mount",
            "/nowhere",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("REFUSED [user]"), "{stdout}");
}

#[test]
fn cli_render_prints_the_whole_generated_set_without_writing() {
    let user = std::env::var("USER").unwrap_or_else(|_| "root".into());
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_onedrive-hydration-install"))
        .args([
            "render",
            "--user",
            &user,
            "--mount",
            "/nonexistent/on/purpose",
            "--client-id",
            "test-client-id",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    for name in [
        "hydrationd.service",
        "hydrationd.path",
        "onedrive-hydration.service",
        "onedrive-hydration-dbus.service",
        "onedrive-hydration-tray.service",
        // Not a unit: the D-Bus activation file, reviewed and diffed like one.
        "io.github.franzjeger.OneDriveHydration.service",
    ] {
        assert!(stdout.contains(&format!("# ==> {name} <==")), "{name}");
    }
    // render validates nothing about the machine: a nonsense mount is fine,
    // because nothing is installed from here.
    assert!(
        stdout.contains("--mount /nonexistent/on/purpose"),
        "{stdout}"
    );
}

#[test]
fn cli_rejects_an_unknown_tray_surface_and_says_why_there_is_no_auto() {
    let user = std::env::var("USER").unwrap_or_else(|_| "root".into());
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_onedrive-hydration-install"))
        .args([
            "render",
            "--user",
            &user,
            "--mount",
            "/nonexistent/on/purpose",
            "--client-id",
            "test-client-id",
            "--tray",
            "auto",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    // "auto" is the thing someone will reach for; the reason there isn't one
    // is the answer they need, so it is in the message and not just the docs.
    assert!(stderr.contains("no auto"), "{stderr}");
    assert!(
        stderr.contains("not a fact this installer can measure"),
        "{stderr}"
    );
    assert!(stderr.contains("sni, plasmoid or none"), "{stderr}");
}

#[test]
fn cli_render_omits_the_tray_unit_for_the_applet_surface() {
    let user = std::env::var("USER").unwrap_or_else(|_| "root".into());
    let run = |tray: &str| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_onedrive-hydration-install"))
            .args([
                "render",
                "--user",
                &user,
                "--mount",
                "/nonexistent/on/purpose",
                "--client-id",
                "test-client-id",
                "--tray",
                tray,
            ])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // A rendered set is diffed against a deployment's units; if it silently
    // rendered a surface the deployment does not have, the diff is noise.
    let sni = run("sni");
    assert!(sni.contains("tray surface: sni"), "{sni}");
    assert!(
        sni.contains(&format!("# ==> {} <==", units::TRAY_UNIT)),
        "{sni}"
    );

    let applet = run("plasmoid");
    assert!(applet.contains("tray surface: plasmoid"), "{applet}");
    assert!(!applet.contains(units::TRAY_UNIT), "{applet}");
    // Everything else is still rendered: the applet talks to the same state
    // service over the same bus name.
    assert!(
        applet.contains("# ==> onedrive-hydration-dbus.service <=="),
        "{applet}"
    );
    assert!(applet.contains("# ==> hydrationd.service <=="), "{applet}");
}

#[test]
fn cli_without_arguments_prints_usage_and_the_never_list() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_onedrive-hydration-install"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--consent-fstab"), "{stderr}");
    assert!(stderr.contains("never"), "{stderr}");
}
