//! Judgment and consequence: the checks, the refusals, and the actions.
//!
//! Everything here is computed as data first — a list of [`Check`]s and a list
//! of [`Action`]s — and only then, maybe, performed. That split is what makes
//! the refusals testable: `tests/refusals.rs` hands these functions
//! measurements this machine cannot produce and asserts that each refusal
//! actually refuses, which is the repository's bar for calling something a
//! check at all.

use crate::probes::{self, FstabEntry, KernelSupport, MountRow, SecretService, Vantage};
use crate::units::{self, Rendered, Templates};
use crate::Facts;
use std::io;
use std::path::{Path, PathBuf};

/// Filesystems whose superblocks set `SB_I_ALLOW_HSM`. Exactly these three —
/// the kernel refuses pre-content marks anywhere else, so a deployment on
/// anything else is not "less supported", it is a helper that cannot mark.
const ALLOW_HSM: [&str; 3] = ["ext4", "btrfs", "xfs"];

/// The payload binaries the generated units point at.
const BINARIES: [&str; 5] = [
    "hydrationd",
    "onedrive-hydration-daemon",
    "onedrive-hydrationctl",
    "onedrive-hydration-dbus",
    "onedrive-hydration-tray",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass(String),
    /// True but not blocking; printed so nobody can say they were not told.
    Caveat(String),
    Refuse(String),
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: &'static str,
    pub outcome: Outcome,
}

impl Check {
    pub fn refused(&self) -> Option<&str> {
        match &self.outcome {
            Outcome::Refuse(m) => Some(m),
            _ => None,
        }
    }
}

/// Everything measured about the machine, gathered once. Tests construct this
/// by hand to make any refusal fire; the binary fills it from
/// [`observe`].
#[derive(Debug, Clone)]
pub struct Observed {
    pub vantage: Vantage,
    pub kernel: KernelSupport,
    pub mountinfo: String,
    pub fstab: String,
    pub secrets: SecretService,
}

/// Gather the real measurements. `prefix` matters only for fstab: a rehearsal
/// against a scratch prefix reads `{prefix}/etc/fstab` when present, so the
/// fstab checks can be exercised without ever looking at the machine's own.
pub fn observe(facts: &Facts, prefix: &Path) -> io::Result<Observed> {
    let staged = prefix.join("etc/fstab");
    let fstab = if staged.exists() {
        std::fs::read_to_string(&staged)?
    } else {
        std::fs::read_to_string("/etc/fstab").unwrap_or_default()
    };
    Ok(Observed {
        vantage: probes::vantage(),
        kernel: probes::kernel_precontent_support(),
        mountinfo: std::fs::read_to_string("/proc/self/mountinfo")?,
        fstab,
        secrets: probes::secret_service(&facts.user, facts.uid),
    })
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Where files are written. `/` is a real installation; anything else is a
    /// rehearsal, in which no command is ever executed.
    pub prefix: PathBuf,
    /// Print everything, write nothing.
    pub dry_run: bool,
    /// Overwrite unit files that exist and differ. Without it, a differing
    /// file is a refusal: a working deployment must not silently become a
    /// different one.
    pub force: bool,
    /// Explicit consent to append the (always-`noauto`) fstab line.
    pub consent_fstab: bool,
}

impl Options {
    pub fn real(&self) -> bool {
        self.prefix == Path::new("/") && !self.dry_run
    }
}

/// What `install` decided.
#[derive(Debug)]
pub struct Planned {
    pub checks: Vec<Check>,
    /// `None` when any check refused; nothing may be written from a refused
    /// plan, and there is deliberately no way to get actions out of one.
    pub actions: Option<Vec<Action>>,
}

impl Planned {
    pub fn refusals(&self) -> Vec<&str> {
        self.checks.iter().filter_map(Check::refused).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    WriteFile {
        path: PathBuf,
        text: String,
        owner: Option<(u32, u32)>,
    },
    /// Enablement, done the way `systemctl enable` does it: a `.wants` symlink
    /// pointing at the unit's *runtime* path — never the prefixed one, since
    /// the prefix exists only in rehearsals.
    Symlink {
        path: PathBuf,
        target: PathBuf,
        owner: Option<(u32, u32)>,
    },
    /// Destination already holds exactly this content; idempotent second runs
    /// say so instead of rewriting.
    Unchanged {
        path: PathBuf,
    },
    AppendFstab {
        path: PathBuf,
        line: String,
    },
    RemoveFile {
        path: PathBuf,
    },
    /// A command that only a real run (`prefix == /`) may execute; rehearsals
    /// print it.
    Run {
        argv: Vec<String>,
        why: String,
    },
    /// Re-read the mount table after the `umount` and refuse to continue while
    /// the sync root is still mounted — the one ordering promise uninstall
    /// makes: `hydrationd` is never left stopped under a mounted sync root.
    VerifyUnmounted {
        mount: PathBuf,
    },
    /// Something this tool refuses to do for the user; printed where they are
    /// already looking.
    Manual {
        text: String,
    },
}

fn check_kernel(kernel: &KernelSupport) -> Check {
    let outcome = match kernel {
        KernelSupport::Measured { release } => Outcome::Pass(format!(
            "kernel {release} accepts FAN_PRE_ACCESS marks (measured)"
        )),
        KernelSupport::Inferred { release } => Outcome::Pass(format!(
            "kernel {release} is at least 6.14, so pre-content events should exist \
             (inferred from the version; run as root for the syscall measurement)"
        )),
        KernelSupport::TooOldMeasured { release } => Outcome::Refuse(format!(
            "kernel {release} rejected FAN_PRE_ACCESS with EINVAL: fanotify pre-content \
             events do not exist before Linux 6.14, and without them a placeholder is \
             just a sparse file full of zeros. Nothing was installed"
        )),
        KernelSupport::TooOldInferred { release } => Outcome::Refuse(format!(
            "kernel {release} is older than 6.14, which introduced fanotify pre-content \
             events; without them hydration cannot intercept reads and every \
             placeholder reads as zeros. Nothing was installed"
        )),
        KernelSupport::Unknown { detail } => Outcome::Refuse(format!(
            "could not determine pre-content support ({detail}); refusing to install on \
             a guess, because the failure being guessed about is silent zeros"
        )),
    };
    Check {
        name: "kernel",
        outcome,
    }
}

fn check_vantage(v: &Vantage) -> Check {
    let outcome = match v {
        Vantage::Host => Outcome::Pass(
            "this process sees pid 1's mount table, so the mount checks below are about \
             the machine, not a sandbox"
                .into(),
        ),
        Vantage::Sandboxed { detail } => Outcome::Refuse(format!(
            "this installer is running in a private mount namespace ({detail}); every \
             mount judgment it made would be about the sandbox, not the machine — the \
             same mechanism that makes a sandboxed hydrationd mark a private copy and \
             fail open. Run it from the host"
        )),
        Vantage::Unknown { detail } => Outcome::Caveat(format!(
            "could not verify this process shares pid 1's mount view ({detail}); \
             proceeding, but if this shell is sandboxed the mount checks are about the \
             sandbox"
        )),
    };
    Check {
        name: "vantage",
        outcome,
    }
}

fn check_mount<'a>(rows: &'a [MountRow], mount: &Path) -> (Check, Option<&'a MountRow>) {
    match probes::find_mount(rows, mount) {
        Some(row) => (
            Check {
                name: "sync-root-mount",
                outcome: Outcome::Pass(format!(
                    "{} is its own mount ({} on {})",
                    mount.display(),
                    row.fstype,
                    row.source
                )),
            },
            Some(row),
        ),
        None => (
            Check {
                name: "sync-root-mount",
                outcome: Outcome::Refuse(format!(
                    "{m} is not a mount point. A directory cannot be protected: a fanotify \
                     directory mark is accepted and delivers nothing (measured, \
                     HydrationAPI probes/dirmark.c), so every placeholder under it would \
                     read as zeros. Give the sync root its own volume — on btrfs this \
                     tool will NOT create it for you, because that is your storage \
                     layout; do it yourself:\n\
                     \x20   sudo btrfs subvolume create <top-level>/@onedrive\n\
                     \x20   (then the noauto fstab entry, then: sudo mount {m})",
                    m = mount.display()
                )),
            },
            None,
        ),
    }
}

fn check_fstype(row: &MountRow) -> Check {
    let outcome = if ALLOW_HSM.contains(&row.fstype.as_str()) {
        Outcome::Pass(format!(
            "{} sets SB_I_ALLOW_HSM, so the kernel accepts pre-content marks on it",
            row.fstype
        ))
    } else if row.fstype == "tmpfs" {
        Outcome::Refuse(
            "the sync root is tmpfs, which does not set SB_I_ALLOW_HSM: the kernel \
             refuses pre-content marks there, and a tmpfs sync root would silently \
             vanish at reboot besides. Use ext4, btrfs or xfs"
                .into(),
        )
    } else {
        Outcome::Refuse(format!(
            "the sync root is {}, which does not set SB_I_ALLOW_HSM; the kernel accepts \
             pre-content marks on exactly ext4, btrfs and xfs, so hydration cannot work \
             here",
            row.fstype
        ))
    };
    Check {
        name: "sync-root-fstype",
        outcome,
    }
}

fn check_exposure(rows: &[MountRow], ours: &MountRow) -> Check {
    let exp = probes::exposures(rows, ours);
    let outcome = if exp.is_empty() {
        Outcome::Pass("no other mount in this namespace exposes the sync files".into())
    } else {
        Outcome::Refuse(format!(
            "other mounts expose the same files (DESIGN.md §6.4a): {} — a read through \
             any of them bypasses hydration and returns zeros. Unmount them or choose \
             storage nothing else reaches. Note this can only be detected, never \
             prevented: anyone can add such a mount later, which is why hydrationd \
             watches for it at runtime and reports it",
            exp.join(", ")
        ))
    };
    Check {
        name: "exposure",
        outcome,
    }
}

fn check_fstab(
    entry: Option<FstabEntry>,
    suggestion: Option<String>,
    consent: bool,
) -> (Check, Option<String>) {
    let (outcome, append) = match entry {
        None => (
            Outcome::Caveat(
                "fstab not checked: the sync root is not mounted, so there is no mount to \
                 describe yet"
                    .into(),
            ),
            None,
        ),
        Some(FstabEntry::NoAuto) => (
            Outcome::Pass(
                "fstab has the sync root with noauto, so the mount never exists without \
                 the helper that marks it"
                    .into(),
            ),
            None,
        ),
        Some(FstabEntry::Automatic { line }) => (
            Outcome::Refuse(format!(
                "the fstab entry for the sync root lacks noauto:\n    {line}\n\
                 At boot the mount would exist before anything can mark it, and every \
                 placeholder would be readable — and zero — from boot until login. Add \
                 noauto (and nofail); this tool will not edit the line for you"
            )),
            None,
        ),
        Some(FstabEntry::Absent) => {
            let line = suggestion.unwrap_or_default();
            if consent {
                (
                    Outcome::Pass(format!(
                        "fstab has no entry for the sync root; with --consent-fstab this \
                         line will be appended:\n    {line}"
                    )),
                    Some(line),
                )
            } else {
                (
                    Outcome::Refuse(format!(
                        "fstab has no entry for the sync root, so RequiresMountsFor= has \
                         no mount unit to pull up and the deployment dies at the first \
                         reboot. This tool does not touch /etc/fstab without \
                         --consent-fstab; either re-run with it, or add the line \
                         yourself:\n    {line}"
                    )),
                    None,
                )
            }
        }
    };
    (
        Check {
            name: "fstab",
            outcome,
        },
        append,
    )
}

fn check_secrets(s: &SecretService, real: bool) -> Check {
    let outcome = match s {
        SecretService::Owned => {
            Outcome::Pass("org.freedesktop.secrets has an owner on the user's session bus".into())
        }
        SecretService::Activatable => Outcome::Pass(
            "org.freedesktop.secrets is activatable on the user's session bus; enrollment \
             will start it"
                .into(),
        ),
        SecretService::Unreachable { detail } => Outcome::Refuse(format!(
            "Secret Service is not reachable: {detail}. Enrollment fails closed without \
             it — there is no plaintext-token fallback, on purpose — so this deployment \
             could never authenticate"
        )),
        SecretService::Unverifiable { detail } if real => Outcome::Refuse(format!(
            "Secret Service could not be verified ({detail}); a real install refuses to \
             assume it"
        )),
        SecretService::Unverifiable { detail } => Outcome::Caveat(format!(
            "Secret Service not verified in this rehearsal: {detail}"
        )),
    };
    Check {
        name: "secret-service",
        outcome,
    }
}

fn check_binaries(bin_dir: &Path) -> Check {
    let missing: Vec<String> = BINARIES
        .iter()
        .filter_map(|b| probes::binary_state(bin_dir, b).err())
        .collect();
    let outcome = if missing.is_empty() {
        Outcome::Pass(format!(
            "all payload binaries present and executable in {}",
            bin_dir.display()
        ))
    } else {
        Outcome::Refuse(format!(
            "the units would point at binaries that are not there — a unit whose \
             ExecStart= is missing fails only at start, which is too late to find out: \
             {}. Install the payload first",
            missing.join("; ")
        ))
    };
    Check {
        name: "binaries",
        outcome,
    }
}

fn check_unit_text(rendered: &Rendered) -> Check {
    let mut hits = Vec::new();
    for unit in rendered.all() {
        if !units::must_share_host_namespace(&unit.name) {
            continue;
        }
        for (line, directive) in units::namespace_directives(&unit.text) {
            hits.push(format!("{}:{line}: {directive}=", unit.name));
        }
    }
    let outcome = if hits.is_empty() {
        Outcome::Pass(
            "the generated helper units contain none of the namespace-creating \
             directives (checked in the generated text, not assumed from the templates)"
                .into(),
        )
    } else {
        Outcome::Refuse(format!(
            "the generated helper unit carries a directive that would give it a private \
             mount namespace: {}. Each of PrivateTmp=, PrivateNetwork=, \
             ProtectKernelTunables=, ProtectControlGroups= and ProtectKernelModules= \
             alone was measured to do this, and a helper in its own namespace marks a \
             private copy of the sync mount while every real read comes back zeros with \
             all units green. Refusing to write it",
            hits.join(", ")
        ))
    };
    Check {
        name: "unit-text",
        outcome,
    }
}

/// Where every rendered unit lands, plus its enablement links, as actions.
fn placement(rendered: &Rendered, facts: &Facts, opts: &Options) -> Vec<Action> {
    let mut actions = Vec::new();
    let sys_dir = units::system_unit_dir(&opts.prefix);
    let usr_dir = units::user_unit_dir(&opts.prefix, &facts.home);
    // Runtime (unprefixed) paths, for symlink targets.
    let sys_rt = units::system_unit_dir(Path::new("/"));
    let usr_rt = units::user_unit_dir(Path::new("/"), &facts.home);
    let owner = Some((facts.uid, facts.gid));

    let mut place = |unit: &units::UnitFile, dir: &Path, rt: &Path, own: Option<(u32, u32)>| {
        actions.push(Action::WriteFile {
            path: dir.join(&unit.name),
            text: unit.text.clone(),
            owner: own,
        });
        // Enablement is read off the generated text so a template edit cannot
        // desynchronize the link from the unit's own [Install] section.
        for line in unit.text.lines() {
            let line = line.trim();
            if let Some(targets) = line.strip_prefix("WantedBy=") {
                for target in targets.split_whitespace() {
                    actions.push(Action::Symlink {
                        path: dir.join(format!("{target}.wants")).join(&unit.name),
                        target: rt.join(&unit.name),
                        owner: own,
                    });
                }
            }
        }
    };

    for unit in &rendered.system {
        place(unit, &sys_dir, &sys_rt, None);
    }
    for unit in &rendered.user {
        place(unit, &usr_dir, &usr_rt, owner);
    }
    actions
}

/// Idempotence: rewrite nothing that is already right, refuse to change what
/// is different, unless forced. Returns refusal messages.
fn collide(actions: &mut [Action], force: bool) -> Vec<String> {
    let mut refusals = Vec::new();
    for a in actions.iter_mut() {
        let replacement = match &*a {
            Action::WriteFile { path, text, .. } => match std::fs::read_to_string(path) {
                Ok(existing) if existing == *text => Some(Action::Unchanged { path: path.clone() }),
                Ok(_) if !force => {
                    refusals.push(format!(
                        "{} exists and differs from what would be generated; refusing to \
                         change an installed deployment without --force (diff the file \
                         against a --dry-run first)",
                        path.display()
                    ));
                    None
                }
                _ => None,
            },
            Action::Symlink { path, target, .. } => match std::fs::read_link(path) {
                Ok(existing) if existing == *target => {
                    Some(Action::Unchanged { path: path.clone() })
                }
                Ok(other) if !force => {
                    refusals.push(format!(
                        "{} is a symlink to {} (expected {}); refusing without --force",
                        path.display(),
                        other.display(),
                        target.display()
                    ));
                    None
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(r) = replacement {
            *a = r;
        }
    }
    refusals
}

/// The whole judgment, as data.
pub fn install(
    facts: &Facts,
    templates: &Templates,
    observed: &Observed,
    opts: &Options,
) -> Planned {
    let mut checks = Vec::new();

    if opts.real() && unsafe { libc::geteuid() } != 0 {
        checks.push(Check {
            name: "root",
            outcome: Outcome::Refuse(
                "a real install writes under /etc and needs root; re-run with sudo, or \
                 rehearse with --prefix <dir> / --dry-run"
                    .into(),
            ),
        });
    }

    checks.push(check_vantage(&observed.vantage));
    checks.push(check_kernel(&observed.kernel));

    let rows = probes::parse_mountinfo(&observed.mountinfo);
    let (mount_check, row) = check_mount(&rows, &facts.mount);
    checks.push(mount_check);

    let mut fstab_append = None;
    if let Some(row) = row {
        checks.push(check_fstype(row));
        checks.push(check_exposure(&rows, row));
        let (c, append) = check_fstab(
            Some(probes::fstab_entry(&observed.fstab, &facts.mount)),
            Some(probes::fstab_suggestion(row)),
            opts.consent_fstab,
        );
        checks.push(c);
        fstab_append = append;
    } else {
        let (c, _) = check_fstab(None, None, opts.consent_fstab);
        checks.push(c);
    }

    checks.push(check_secrets(&observed.secrets, opts.real()));
    checks.push(check_binaries(&facts.bin_dir));

    let rendered = units::render(templates, facts);
    checks.push(check_unit_text(&rendered));

    let mut actions = placement(&rendered, facts, opts);
    for msg in collide(&mut actions, opts.force) {
        checks.push(Check {
            name: "collision",
            outcome: Outcome::Refuse(msg),
        });
    }

    if let Some(line) = fstab_append {
        actions.push(Action::AppendFstab {
            path: opts.prefix.join("etc/fstab"),
            line,
        });
    }

    actions.push(Action::Manual {
        text: format!(
            "next: sudo systemctl daemon-reload  (the path unit is enabled by its \
             .wants link and arms at next boot; to arm it now: sudo systemctl start \
             hydrationd.path)\n\
             then, as {u}: systemctl --user daemon-reload && systemctl --user start \
             onedrive-hydration.service onedrive-hydration-dbus.service\n\
             the tray starts with the next graphical session, or now with: systemctl \
             --user start onedrive-hydration-tray.service (icons: run \
             packaging/icons/install-icons.sh once per user)\n\
             not enrolled yet? as {u}: onedrive-hydration-daemon auth --state-dir \
             ~/.local/state/onedrive-hydration --client-id <the id you passed>\n\
             this tool did NOT create the subvolume, did NOT edit fstab beyond what \
             was shown, and did NOT touch credentials — those stay yours",
            u = facts.user
        ),
    });

    let refused = checks.iter().any(|c| c.refused().is_some());
    Planned {
        checks,
        actions: (!refused).then_some(actions),
    }
}

/// Uninstall: the mirror image, with one promise that outranks tidiness —
/// `hydrationd` is never left stopped (or removed) while the sync root is
/// still mounted, because a marked mount with nobody answering, or a fresh
/// unmarked mount, is the fail-open state this project exists to prevent.
pub fn uninstall(facts: &Facts, mounted: bool, and_unmount: bool, opts: &Options) -> Planned {
    let mut checks = Vec::new();
    let mount = facts.mount.display();

    if opts.real() && unsafe { libc::geteuid() } != 0 {
        checks.push(Check {
            name: "root",
            outcome: Outcome::Refuse(
                "a real uninstall stops system units and removes files under /etc; \
                 re-run with sudo, or rehearse with --prefix <dir>"
                    .into(),
            ),
        });
    }

    if mounted && !and_unmount {
        checks.push(Check {
            name: "mount-safety",
            outcome: Outcome::Refuse(format!(
                "{mount} is currently mounted. Removing the units or stopping the \
                 helper while it stays mounted would leave every placeholder readable \
                 as zeros — the fail-open state — so this tool refuses. Re-run with \
                 --and-unmount to take the deployment down in the safe order (stop \
                 hydrationd.path, then umount, which stops the helper with its mount), \
                 or unmount yourself first"
            )),
        });
    } else {
        checks.push(Check {
            name: "mount-safety",
            outcome: Outcome::Pass(if mounted {
                format!(
                    "{mount} is mounted; it will be unmounted before anything is removed, \
                     and removal aborts if it will not come down"
                )
            } else {
                format!("{mount} is not mounted; nothing can fail open during removal")
            }),
        });
    }

    let refused = checks.iter().any(|c| c.refused().is_some());
    if refused {
        return Planned {
            checks,
            actions: None,
        };
    }

    let mut actions = Vec::new();
    // Stop the trigger before anything else: the socket still exists, and a
    // path unit that stays armed would restart the helper the moment it is
    // stopped — each restart pulling the mount back up underneath us.
    actions.push(Action::Run {
        argv: vec!["systemctl".into(), "stop".into(), "hydrationd.path".into()],
        why: "disarm the trigger so nothing restarts the helper mid-removal".into(),
    });
    if mounted {
        // Deliberately NOT `systemctl stop hydrationd.service`: stopping the
        // helper first would leave a window — and possibly a permanent state,
        // if anything below fails — with the mount up and unanswered. The
        // umount's stop job reaches the helper through RequiresMountsFor=, so
        // the mount and its helper go down together, in that order.
        actions.push(Action::Run {
            argv: vec!["umount".into(), facts.mount.display().to_string()],
            why: "take the mount down; RequiresMountsFor= stops the helper with it".into(),
        });
        actions.push(Action::VerifyUnmounted {
            mount: facts.mount.clone(),
        });
    } else {
        actions.push(Action::Run {
            argv: vec![
                "systemctl".into(),
                "stop".into(),
                "hydrationd.service".into(),
            ],
            why: "with no mount there is nothing to fail open; stop the helper if it \
                  is somehow still running"
                .into(),
        });
    }
    actions.push(Action::Run {
        argv: vec![
            "systemctl".into(),
            "--user".into(),
            format!("--machine={}@.host", facts.user),
            "disable".into(),
            "--now".into(),
            "onedrive-hydration.service".into(),
            "onedrive-hydration-dbus.service".into(),
            "onedrive-hydration-tray.service".into(),
        ],
        why: "stop and disable the user half".into(),
    });

    let sys_dir = units::system_unit_dir(&opts.prefix);
    let usr_dir = units::user_unit_dir(&opts.prefix, &facts.home);
    for name in ["hydrationd.service", "hydrationd.path"] {
        actions.push(Action::RemoveFile {
            path: sys_dir.join(name),
        });
    }
    actions.push(Action::RemoveFile {
        path: sys_dir.join("multi-user.target.wants/hydrationd.path"),
    });
    for name in [
        "onedrive-hydration.service",
        "onedrive-hydration-dbus.service",
        "onedrive-hydration-tray.service",
    ] {
        actions.push(Action::RemoveFile {
            path: usr_dir.join(name),
        });
    }
    for link in [
        "default.target.wants/onedrive-hydration.service",
        "default.target.wants/onedrive-hydration-dbus.service",
        "graphical-session.target.wants/onedrive-hydration-tray.service",
    ] {
        actions.push(Action::RemoveFile {
            path: usr_dir.join(link),
        });
    }
    actions.push(Action::Run {
        argv: vec!["systemctl".into(), "daemon-reload".into()],
        why: "forget the removed system units".into(),
    });
    actions.push(Action::Manual {
        text: format!(
            "left in place on purpose:\n\
             - the fstab line for {mount} (inert with noauto; remove it yourself if \
             the volume is going away)\n\
             - the subvolume/volume itself; when — and only when — nothing unsent \
             remains in it: sudo btrfs subvolume delete <path>  (this tool will not \
             delete storage)\n\
             - the refresh token in {u}'s Secret Service and the state directory \
             ~{u}/.local/state/onedrive-hydration; remove them with your keyring tool \
             and rm -r if the account is being abandoned",
            u = facts.user
        ),
    });

    Planned {
        checks,
        actions: Some(actions),
    }
}

/// How much of a plan may touch the world.
#[derive(Debug, Clone, Copy)]
pub struct ExecMode {
    pub write_files: bool,
    /// Commands run only in a real (`prefix == /`, not dry-run) invocation;
    /// a rehearsal writes files into the prefix but executes nothing.
    pub run_commands: bool,
}

impl ExecMode {
    pub fn from(opts: &Options) -> Self {
        ExecMode {
            write_files: !opts.dry_run,
            run_commands: opts.real(),
        }
    }
}

/// Perform (or narrate) the plan.
///
/// The transcript is returned even when execution fails partway: by then some
/// of the actions have already happened, and a tool that swallows the record
/// of what it just did to a machine is not one an operator can trust. The
/// error itself names the path or command that failed.
pub fn execute(actions: &[Action], mode: ExecMode) -> (Vec<String>, io::Result<()>) {
    let mut log = Vec::new();
    let result = execute_inner(actions, mode, &mut log);
    (log, result)
}

/// Attach the path to an io error; a bare "Permission denied" from a plan
/// with a dozen paths in it is a puzzle, not a message.
fn at<T>(r: io::Result<T>, what: &str, path: &Path) -> io::Result<T> {
    r.map_err(|e| io::Error::new(e.kind(), format!("{what} {}: {e}", path.display())))
}

fn execute_inner(actions: &[Action], mode: ExecMode, log: &mut Vec<String>) -> io::Result<()> {
    let we_are_root = unsafe { libc::geteuid() } == 0;
    for action in actions {
        match action {
            Action::WriteFile { path, text, owner } => {
                if mode.write_files {
                    at(
                        create_dirs_owned(path.parent().unwrap(), *owner, we_are_root),
                        "creating directories for",
                        path,
                    )?;
                    // Write-then-rename so a crash cannot leave a half unit
                    // for systemd to half-load. The temporary keeps the real
                    // name plus a suffix systemd does not load.
                    let mut name = path.file_name().unwrap_or_default().to_os_string();
                    name.push(".tmp-install");
                    let tmp = path.with_file_name(name);
                    at(std::fs::write(&tmp, text), "writing", &tmp)?;
                    at(chown_if(&tmp, *owner, we_are_root), "chowning", &tmp)?;
                    at(std::fs::rename(&tmp, path), "renaming into place", path)?;
                    log.push(format!("wrote {}", path.display()));
                } else {
                    log.push(format!("would write {}", path.display()));
                }
            }
            Action::Symlink {
                path,
                target,
                owner,
            } => {
                if mode.write_files {
                    create_dirs_owned(path.parent().unwrap(), *owner, we_are_root)?;
                    match std::fs::read_link(path) {
                        Ok(existing) if existing == *target => {}
                        Ok(_) => {
                            std::fs::remove_file(path)?;
                            std::os::unix::fs::symlink(target, path)?;
                        }
                        Err(_) => std::os::unix::fs::symlink(target, path)?,
                    }
                    lchown_if(path, *owner, we_are_root)?;
                    log.push(format!(
                        "enabled {} -> {}",
                        path.display(),
                        target.display()
                    ));
                } else {
                    log.push(format!(
                        "would enable {} -> {}",
                        path.display(),
                        target.display()
                    ));
                }
            }
            Action::Unchanged { path } => {
                log.push(format!("unchanged {}", path.display()));
            }
            Action::AppendFstab { path, line } => {
                if mode.write_files {
                    use std::io::Write as _;
                    at(
                        create_dirs_owned(path.parent().unwrap(), None, we_are_root),
                        "creating directories for",
                        path,
                    )?;
                    let mut f = at(
                        std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path),
                        "opening for append",
                        path,
                    )?;
                    writeln!(
                        f,
                        "# OneDrive hydration sync root. noauto on purpose: the mount \
                         must not exist\n# before the helper that marks it (see \
                         hydrationd.service).\n{line}"
                    )?;
                    log.push(format!("appended to {}: {line}", path.display()));
                } else {
                    log.push(format!("would append to {}: {line}", path.display()));
                }
            }
            Action::RemoveFile { path } => {
                if mode.write_files {
                    match std::fs::remove_file(path) {
                        Ok(()) => log.push(format!("removed {}", path.display())),
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {
                            log.push(format!("already absent {}", path.display()))
                        }
                        Err(e) => return at(Err(e), "removing", path),
                    }
                } else {
                    log.push(format!("would remove {}", path.display()));
                }
            }
            Action::Run { argv, why } => {
                if mode.run_commands {
                    let status = std::process::Command::new(&argv[0])
                        .args(&argv[1..])
                        .status()?;
                    if !status.success() {
                        return Err(io::Error::other(format!(
                            "{} failed ({status}); stopping here — {why}",
                            argv.join(" ")
                        )));
                    }
                    log.push(format!("ran {} ({why})", argv.join(" ")));
                } else {
                    log.push(format!("would run {} ({why})", argv.join(" ")));
                }
            }
            Action::VerifyUnmounted { mount } => {
                if mode.run_commands {
                    let table = std::fs::read_to_string("/proc/self/mountinfo")?;
                    let rows = probes::parse_mountinfo(&table);
                    if probes::find_mount(&rows, mount).is_some() {
                        return Err(io::Error::other(format!(
                            "{} is still mounted after umount; refusing to remove \
                             anything — a deployment must never be half-removed with \
                             its mount up",
                            mount.display()
                        )));
                    }
                    log.push(format!("verified {} is no longer mounted", mount.display()));
                } else {
                    log.push(format!(
                        "would verify {} is no longer mounted before removing anything",
                        mount.display()
                    ));
                }
            }
            Action::Manual { text } => {
                log.push(text.clone());
            }
        }
    }
    Ok(())
}

fn create_dirs_owned(dir: &Path, owner: Option<(u32, u32)>, root: bool) -> io::Result<()> {
    // Walk down creating one level at a time so each *newly created* directory
    // can be chowned; pre-existing ones are left alone — an installer that
    // re-owns ~/.config wholesale would be a different kind of hazard.
    let mut stack = Vec::new();
    let mut cur = dir;
    while !cur.exists() {
        stack.push(cur.to_path_buf());
        match cur.parent() {
            Some(p) => cur = p,
            None => break,
        }
    }
    for d in stack.into_iter().rev() {
        std::fs::create_dir(&d)?;
        chown_if(&d, owner, root)?;
    }
    Ok(())
}

fn chown_if(path: &Path, owner: Option<(u32, u32)>, root: bool) -> io::Result<()> {
    chown_impl(path, owner, root, false)
}

/// Symlinks need `lchown`: following the link would chown its target, which
/// in a rehearsal prefix is a dangling runtime path.
fn lchown_if(path: &Path, owner: Option<(u32, u32)>, root: bool) -> io::Result<()> {
    chown_impl(path, owner, root, true)
}

fn chown_impl(path: &Path, owner: Option<(u32, u32)>, root: bool, link: bool) -> io::Result<()> {
    // Without root, chown would fail and the files already belong to us —
    // which in a prefix rehearsal is exactly right.
    let Some((uid, gid)) = owner else {
        return Ok(());
    };
    if !root {
        return Ok(());
    }
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let rc = if link {
        unsafe { libc::lchown(c.as_ptr(), uid, gid) }
    } else {
        unsafe { libc::chown(c.as_ptr(), uid, gid) }
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
