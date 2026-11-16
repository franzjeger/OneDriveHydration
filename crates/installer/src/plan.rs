//! Judgment and consequence: the checks, the refusals, and the actions.
//!
//! Everything here is computed as data first — a list of [`Check`]s and a list
//! of [`Action`]s — and only then, maybe, performed. That split is what makes
//! the refusals testable: `tests/refusals.rs` hands these functions
//! measurements this machine cannot produce and asserts that each refusal
//! actually refuses, which is the repository's bar for calling something a
//! check at all.

use crate::probes::{self, FstabEntry, KernelSupport, MountRow, Plasmoid, SecretService, Vantage};
use crate::units::{self, Rendered, Templates, Tray};
use crate::Facts;
use std::io;
use std::path::{Path, PathBuf};

/// Filesystems whose superblocks set `SB_I_ALLOW_HSM`. Exactly these three —
/// the kernel refuses pre-content marks anywhere else, so a deployment on
/// anything else is not "less supported", it is a helper that cannot mark.
const ALLOW_HSM: [&str; 3] = ["ext4", "btrfs", "xfs"];

/// The payload binaries every deployment's units point at.
const BINARIES: [&str; 4] = [
    "hydrationd",
    "onedrive-hydration-daemon",
    "onedrive-hydrationctl",
    "onedrive-hydration-dbus",
];

/// Required only when [`Tray::Sni`] is the surface. The binary check's whole
/// justification is that a unit's `ExecStart=` must not point at nothing; with
/// the applet as the surface no unit points here, so demanding it would be a
/// refusal with no reason behind it.
const TRAY_BINARY: &str = "onedrive-hydration-tray";

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
    /// Whether the Plasma applet — a tray entry in its own right — is already
    /// installed for this user. Not "is plasmashell running"; see [`Tray`].
    pub plasmoid: Plasmoid,
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
        // Same prefix-then-machine rule as fstab above.
        plasmoid: probes::plasmoid_observed(prefix, &facts.home),
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
    /// Which tray surface this deployment uses. `None` means the operator did
    /// not say — which is fine while there is only one surface installed, and
    /// a refusal the moment both are, because two identical icons is not a
    /// question this tool can answer for them.
    pub tray: Option<Tray>,
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

/// Which tray surface this deployment gets, and whether that could be decided
/// at all.
///
/// The collision is real and was measured: the applet is a system-tray entry
/// in its own right — a running plasmashell adopted it into the tray's
/// `knownItems` and instantiated it within seconds of `kpackagetool6`
/// finishing, no restart — so a user who runs this installer *and*
/// `install-plasmoid.sh` gets two identical icons. Neither surface is wrong;
/// they cover different desktops. The applet is better where it works (native
/// look, and it carries the flyout the binary cannot draw) and only works
/// under plasmashell; the binary covers every desktop with a
/// `StatusNotifierWatcher` and no plasmashell.
///
/// So this returns a decision, never a guess: the one thing measured is
/// whether the applet package exists, and when it does and nothing was said,
/// it refuses and asks. See [`Tray`] for why the running desktop is not an
/// input.
fn check_tray(plasmoid: &Plasmoid, chosen: Option<Tray>, facts: &Facts) -> (Check, Tray) {
    let user = &facts.user;
    let (outcome, tray) = match (plasmoid, chosen) {
        (Plasmoid::Present { path }, None) => (
            Outcome::Refuse(format!(
                "two tray surfaces would be installed, and both draw an icon. The Plasma \
                 applet is already installed for {user} at {p}, and it is a system-tray \
                 entry in its own right — plasmashell instantiates it without being asked \
                 — so enabling {unit} as well puts two identical icons in the tray. Which \
                 surface this deployment uses is a decision, not a measurement: the applet \
                 draws only under plasmashell, the binary draws wherever there is a \
                 StatusNotifierWatcher, and which desktop {user} logs into is not \
                 something this tool can know at install time. Say which:\n\
                 \x20   --tray plasmoid   the applet is the surface; the tray unit is not \
                 installed, and one left by an earlier install is removed\n\
                 \x20   --tray sni        install the tray unit as well — deliberate, and \
                 still two icons under plasmashell\n\
                 \x20   --tray none       no tray; onedrive-hydrationctl status is the \
                 surface\n\
                 or remove the applet first:\n\
                 \x20   kpackagetool6 --type Plasma/Applet --remove {id}",
                p = path.display(),
                unit = units::TRAY_UNIT,
                id = probes::PLASMOID_ID,
            )),
            // The plan is refused, so this decides nothing that gets written.
            // It does decide what the *rest of the checks* then assert, and
            // `None` is the only honest answer while the question is open: with
            // `Sni` here, an undecided surface makes the binary check demand
            // the tray binary and print a second refusal about a unit nobody
            // has chosen to install — a true-sounding diagnostic for a problem
            // the operator may not have.
            Tray::None,
        ),
        (Plasmoid::Present { path }, Some(Tray::Sni)) => (
            Outcome::Caveat(format!(
                "--tray sni with the Plasma applet installed at {}: under plasmashell both \
                 will show an icon, and they will look identical. Taken as deliberate — a \
                 user who logs into both Plasma and a desktop without it needs the binary \
                 — but it is the state this check exists to stop happening by accident",
                path.display()
            )),
            Tray::Sni,
        ),
        (Plasmoid::Present { path }, Some(Tray::Plasmoid)) => (
            Outcome::Pass(format!(
                "the Plasma applet at {} is this deployment's tray; {} is not installed, \
                 and one left by an earlier install is removed below",
                path.display(),
                units::TRAY_UNIT
            )),
            Tray::Plasmoid,
        ),
        (Plasmoid::Absent, None) => (
            Outcome::Pass(format!(
                "no Plasma applet is installed for {user}, so {unit} is this deployment's \
                 tray and there is nothing to collide with. On Plasma the applet is the \
                 better surface — packaging/plasmoid/install-plasmoid.sh, which this tool \
                 does not run — and installing it later means re-running this with \
                 --tray plasmoid, or there will be two icons",
                unit = units::TRAY_UNIT
            )),
            Tray::Sni,
        ),
        (Plasmoid::Absent, Some(Tray::Plasmoid)) => (
            Outcome::Caveat(format!(
                "--tray plasmoid, but no applet is installed for {user} at any of: {}. \
                 Nothing is wrong with installing in this order — but until \
                 packaging/plasmoid/install-plasmoid.sh is run as {user}, this deployment \
                 has no tray at all",
                // The real resolved home, never `/home/<user>`: this tool
                // binds a uid and a home it looked up, and a message that
                // guessed the path would send someone to the wrong directory.
                probes::plasmoid_dirs(Path::new("/"), &facts.home)
                    .iter()
                    .map(|d| d.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Tray::Plasmoid,
        ),
        (_, Some(Tray::Sni)) => (
            Outcome::Pass(format!(
                "{} is this deployment's tray; no Plasma applet is installed for {user}, \
                 so nothing collides with it",
                units::TRAY_UNIT
            )),
            Tray::Sni,
        ),
        (_, Some(Tray::None)) => (
            Outcome::Caveat(
                "--tray none: no tray icon is installed, and any left by an earlier \
                 install is removed below. onedrive-hydrationctl status is then the only \
                 place the exposure warning appears — the one thing in this product a \
                 person cannot discover any other way"
                    .into(),
            ),
            Tray::None,
        ),
    };
    (
        Check {
            name: "tray-surface",
            outcome,
        },
        tray,
    )
}

fn check_binaries(bin_dir: &Path, tray: Tray) -> Check {
    let mut wanted: Vec<&str> = BINARIES.to_vec();
    if tray == Tray::Sni {
        wanted.push(TRAY_BINARY);
    }
    let missing: Vec<String> = wanted
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

fn check_facts(facts: &Facts) -> Option<Check> {
    let mut bad = Vec::new();
    if facts.user.is_empty()
        || facts.user.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '@' | '"' | '\'' | '\\' | '%' | '$')
        })
    {
        bad.push(format!(
            "user {:?} contains unsafe unit/machine characters",
            facts.user
        ));
    }
    if facts.client_id.is_empty()
        || !facts
            .client_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bad.push(format!(
            "client_id {:?} must contain only ASCII alphanumerics, '-', '_' or '.'",
            facts.client_id
        ));
    }
    for (label, path) in [
        ("mount", &facts.mount),
        ("home", &facts.home),
        ("bin_dir", &facts.bin_dir),
        ("socket", &facts.socket),
    ] {
        if !path.is_absolute() {
            bad.push(format!(
                "{label} ({}) must be an absolute path",
                path.display()
            ));
            continue;
        }
        match path.to_str() {
            None => bad.push(format!("{label} ({}) must be valid UTF-8", path.display())),
            Some(s) if s.chars().any(|c| c.is_control() || matches!(c, '%' | '$')) => {
                bad.push(format!(
                    "{label} ({}) contains control characters or systemd '%'/ '$' specifiers",
                    path.display()
                ));
            }
            Some(_) => {}
        }
    }
    if bad.is_empty() {
        None
    } else {
        Some(Check {
            name: "facts",
            outcome: Outcome::Refuse(format!(
                "refusing unsafe deployment parameters that could corrupt systemd units or /etc/fstab: {}",
                bad.join("; ")
            )),
        })
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
    // D-Bus activation files carry no [Install] section, so `place` writes
    // them without enablement links — the session bus itself is what reads
    // this directory, and dropping the file there *is* the enablement.
    let bus_dir = units::dbus_service_dir(&opts.prefix, &facts.home);
    let bus_rt = units::dbus_service_dir(Path::new("/"), &facts.home);
    for unit in &rendered.bus_services {
        place(unit, &bus_dir, &bus_rt, owner);
    }
    actions
}

/// Take out a tray unit an earlier `--tray sni` install left behind, so that
/// choosing another surface actually removes the second icon rather than only
/// declining to add one.
///
/// Gated on the unit (or its enablement link) really being there, and that
/// gate is load-bearing: `systemctl disable --now` on a unit that does not
/// exist fails, [`Action::Run`] treats a failed command as fatal to the whole
/// plan, and an ungated cleanup would therefore abort every `--tray plasmoid`
/// install on a machine that never had the unit. The link is probed with
/// `symlink_metadata`, not `exists()` — it points at the *runtime* path, which
/// under a rehearsal prefix is dangling, and `exists()` follows the link and
/// would report a live enablement as absent.
fn retire_tray_unit(facts: &Facts, opts: &Options) -> Vec<Action> {
    let usr_dir = units::user_unit_dir(&opts.prefix, &facts.home);
    let unit = usr_dir.join(units::TRAY_UNIT);
    let link = usr_dir
        .join("graphical-session.target.wants")
        .join(units::TRAY_UNIT);
    if !unit.exists() && std::fs::symlink_metadata(&link).is_err() {
        return Vec::new();
    }
    vec![
        Action::Run {
            argv: vec![
                "systemctl".into(),
                "--user".into(),
                format!("--machine={}@.host", facts.user),
                "disable".into(),
                "--now".into(),
                units::TRAY_UNIT.into(),
            ],
            why: "another surface is this deployment's tray now; stop the second icon \
                  before removing its unit"
                .into(),
        },
        Action::RemoveFile { path: unit },
        Action::RemoveFile { path: link },
    ]
}

/// Idempotence: rewrite nothing that is already right, refuse to change what
/// is different, unless forced. Returns refusal messages.
fn collide(actions: &mut [Action], force: bool) -> Vec<String> {
    // SAFETY: `geteuid` has no preconditions and takes no arguments.
    let we_are_root = unsafe { libc::geteuid() } == 0;
    let mut refusals = Vec::new();
    for a in actions.iter_mut() {
        let replacement = match &*a {
            Action::WriteFile { path, text, .. } => {
                match read_regular_nofollow(path, we_are_root) {
                    Ok(Some(existing)) if existing == *text => {
                        Some(Action::Unchanged { path: path.clone() })
                    }
                    Ok(Some(_)) if !force => {
                        refusals.push(format!(
                            "{} exists and differs from what would be generated; refusing to \
                         change an installed deployment without --force (diff the file \
                         against a --dry-run first)",
                            path.display()
                        ));
                        None
                    }
                    Ok(None) | Ok(Some(_)) => None,
                    Err(e) => {
                        refusals.push(format!(
                            "{} cannot be safely inspected ({e}); refusing to follow symlinks or \
                         non-regular files",
                            path.display()
                        ));
                        None
                    }
                }
            }
            Action::Symlink { path, target, .. } => match read_link_secure(path, we_are_root) {
                Ok(Some(existing)) if existing == *target => {
                    Some(Action::Unchanged { path: path.clone() })
                }
                Ok(Some(other)) if !force => {
                    refusals.push(format!(
                        "{} is a symlink to {} (expected {}); refusing without --force",
                        path.display(),
                        other.display(),
                        target.display()
                    ));
                    None
                }
                Ok(None) | Ok(Some(_)) => None,
                Err(e) => {
                    refusals.push(format!(
                        "{} cannot be safely inspected ({e}); refusing to traverse untrusted symlinks",
                        path.display()
                    ));
                    None
                }
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

    if let Some(bad_facts) = check_facts(facts) {
        checks.push(bad_facts);
    }

    // SAFETY: `geteuid` has no preconditions and takes no arguments.
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

    // Decided before the binary check, which depends on it: with the applet as
    // the surface, no unit points at the tray binary.
    let (tray_check, tray) = check_tray(&observed.plasmoid, opts.tray, facts);
    checks.push(tray_check);
    checks.push(check_binaries(&facts.bin_dir, tray));

    let rendered = units::render(templates, facts, tray);
    checks.push(check_unit_text(&rendered));

    let mut actions = placement(&rendered, facts, opts);
    if tray != Tray::Sni {
        actions.extend(retire_tray_unit(facts, opts));
    }
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

    // The tray line says what this run actually decided, not what the product
    // can do in general: the whole point of --tray is that the choice stops
    // being silent, and next-steps that named a unit this run did not install
    // would put it straight back.
    let desktop_assets = facts
        .bin_dir
        .parent()
        .unwrap_or(Path::new("/usr/local"))
        .join("share/onedrive-hydration/packaging");
    let tray_next = match tray {
        Tray::Sni => format!(
            "the tray starts with the next graphical session, or now with: systemctl \
             --user start {unit} (icons: run {assets}/icons/install-icons.sh once \
             per user)",
            unit = units::TRAY_UNIT,
            assets = desktop_assets.display()
        ),
        Tray::Plasmoid => format!(
            "the tray is the Plasma applet, which this tool did not install and will \
             not — as {u}, and in this order: {assets}/icons/install-icons.sh, then \
             {assets}/plasmoid/install-plasmoid.sh; a running plasmashell picks up a \
             first install by itself",
            u = facts.user,
            assets = desktop_assets.display()
        ),
        Tray::None => "no tray was installed (--tray none); onedrive-hydrationctl \
                       status is the surface, and the only place the exposure warning \
                       appears"
            .to_string(),
    };
    actions.push(Action::Manual {
        text: format!(
            "next: sudo systemctl daemon-reload  (the path unit is enabled by its \
             .wants link and arms at next boot; to arm it now: sudo systemctl start \
             hydrationd.path)\n\
             then, as {u}: systemctl --user daemon-reload && systemctl --user start \
             onedrive-hydration.service\n\
             the state service needs no start or enablement: it is D-Bus-activated \
             the first time anything talks to its name (the tray, busctl, the \
             flyout); a session bus that predates this install may need one \
             log-out/log-in to notice the new activation file\n\
             {tray_next}\n\
             recommended for Plasma: as {u}, run {assets}/install-desktop.sh --mount {m}; \
             it installs and checks the panel, icons, Dolphin menus and overlays together. \
             Use --check on the same command to diagnose an existing desktop setup.\n\
             for Dolphin's \"Free Up Space\" and \"Keep on Device\" actions, as {u}: \
             {assets}/dolphin/install-servicemenu.sh --mount {m}\n\
             for cloud/on-device badges (compiled KF6 plugin): \
             {assets}/dolphin/overlay/install-overlay.sh --mount {m}\n\
             the same actions for GNOME Files, as {u}: \
             {assets}/nautilus/install-nautilus-scripts.sh --mount {m}\n\
             keep Dolphin previews off for this sync tree: previews read and hydrate \
             cloud-only files; this installer will not change a global desktop preference\n\
             not enrolled yet? as {u}: onedrive-hydration-daemon auth --state-dir \
             ~/.local/state/onedrive-hydration --client-id <the id you passed>\n\
             this tool did NOT create the subvolume, did NOT edit fstab beyond what \
             was shown, did NOT install the Plasma applet or Dolphin integration, \
             and did NOT touch credentials — those stay yours",
            assets = desktop_assets.display(),
            m = facts.mount.display(),
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
    let sys_dir = units::system_unit_dir(&opts.prefix);
    let usr_dir = units::user_unit_dir(&opts.prefix, &facts.home);

    // The tray unit is named only when it is really there. Not every
    // deployment has one: `--tray plasmoid` and `--tray none` install none.
    // Measured — `systemctl --user disable --now <absent unit>` exits 1
    // ("Unit ... does not exist"), and `Action::Run` treats a failed command
    // as fatal — so naming it unconditionally would abort the uninstall
    // *after* the unmount, leaving exactly the half-removed deployment the
    // ordering above exists to avoid. The `RemoveFile`s below need no such
    // gate; they tolerate absence by design.
    let mut user_units = vec![
        "onedrive-hydration.service".to_string(),
        "onedrive-hydration-dbus.service".to_string(),
    ];
    if usr_dir.join(units::TRAY_UNIT).exists() {
        user_units.push(units::TRAY_UNIT.to_string());
    }
    let mut argv = vec![
        "systemctl".to_string(),
        "--user".to_string(),
        format!("--machine={}@.host", facts.user),
        "disable".to_string(),
        "--now".to_string(),
    ];
    argv.extend(user_units);
    actions.push(Action::Run {
        argv,
        why: "stop and disable the user half".into(),
    });
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
        units::TRAY_UNIT,
    ] {
        actions.push(Action::RemoveFile {
            path: usr_dir.join(name),
        });
    }
    // The state service's D-Bus activation file. Removed with the units so
    // nothing can re-activate a service whose binary is about to be
    // unreachable; RemoveFile tolerates absence for deployments that predate
    // activation.
    actions.push(Action::RemoveFile {
        path: units::dbus_service_dir(&opts.prefix, &facts.home)
            .join("io.github.franzjeger.OneDriveHydration.service"),
    });
    for link in [
        "default.target.wants/onedrive-hydration.service".to_string(),
        // Written by installs that predate D-Bus activation of the state
        // service; current installs write no such link, but an uninstall has
        // to clean up the deployments that exist, not the ones this version
        // would make.
        "default.target.wants/onedrive-hydration-dbus.service".to_string(),
        format!("graphical-session.target.wants/{}", units::TRAY_UNIT),
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
             - the Plasma applet, if one is installed: this tool never installed it, \
             and a per-user applet is the user's own session data. Remove it \
             yourself, as {u}:  kpackagetool6 --type Plasma/Applet --remove {id}\n\
             - the fstab line for {mount} (inert with noauto; remove it yourself if \
             the volume is going away)\n\
             - the subvolume/volume itself; when — and only when — nothing unsent \
             remains in it: sudo btrfs subvolume delete <path>  (this tool will not \
             delete storage)\n\
             - the refresh token in {u}'s Secret Service and the state directory \
             ~{u}/.local/state/onedrive-hydration; remove them with your keyring tool \
             and rm -r if the account is being abandoned",
            u = facts.user,
            id = probes::PLASMOID_ID
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
    // SAFETY: `geteuid` has no preconditions and takes no arguments.
    let we_are_root = unsafe { libc::geteuid() } == 0;
    for action in actions {
        match action {
            Action::WriteFile { path, text, owner } => {
                if mode.write_files {
                    at(
                        write_file_secure(path, text.as_bytes(), *owner, we_are_root),
                        "writing",
                        path,
                    )?;
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
                    at(
                        write_symlink_secure(path, target, *owner, we_are_root),
                        "enabling symlink",
                        path,
                    )?;
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
                    at(
                        append_fstab_secure(path, line, we_are_root),
                        "appending to",
                        path,
                    )?;
                    log.push(format!("appended to {}: {line}", path.display()));
                } else {
                    log.push(format!("would append to {}: {line}", path.display()));
                }
            }
            Action::RemoveFile { path } => {
                if mode.write_files {
                    match at(remove_file_secure(path, we_are_root), "removing", path)? {
                        true => log.push(format!("removed {}", path.display())),
                        false => log.push(format!("already absent {}", path.display())),
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

fn cstring_os(s: &std::ffi::OsStr) -> io::Result<std::ffi::CString> {
    std::ffi::CString::new(s.as_encoded_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

fn leaf_name(path: &Path) -> io::Result<(&Path, std::ffi::CString)> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "target path has no parent directory",
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "target path has no leaf name")
    })?;
    Ok((parent, cstring_os(name)?))
}

/// Traverse `dir` from `/` component-by-component using `openat(..., O_NOFOLLOW)`
/// so an unprivileged user cannot plant symlinks under `~user/.config` (or `.local`)
/// to trick a `root` installer into creating, overwriting, chowning, or unlinking
/// arbitrary files outside their directory tree.
fn open_dir_secure(
    dir: &Path,
    create: bool,
    owner: Option<(u32, u32)>,
    we_are_root: bool,
) -> io::Result<Option<std::os::fd::OwnedFd>> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::path::Component;

    let abs = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()?.join(dir)
    };
    let root_c = std::ffi::CString::new("/").expect("static C literal");
    // SAFETY: `root_c` is a valid NUL-terminated string `"/"`.
    let root_raw = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if root_raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `root_raw >= 0` is a freshly opened descriptor owned exclusively here.
    let mut cur = unsafe { OwnedFd::from_raw_fd(root_raw) };
    let mut in_sensitive = false;

    for comp in abs.components() {
        let seg = match comp {
            Component::RootDir | Component::CurDir => continue,
            Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "refusing '..' component in installation path",
                ));
            }
            Component::Prefix(_) => continue,
            Component::Normal(s) => s,
        };
        let seg_bytes = seg.as_encoded_bytes();
        if matches!(seg_bytes, b".config" | b".local" | b"etc") || seg_bytes.ends_with(b".wants") {
            in_sensitive = true;
        }
        let seg_c = cstring_os(seg)?;
        // SAFETY: `cur` is a valid directory fd and `seg_c` is NUL-terminated.
        let next_raw = unsafe {
            libc::openat(
                cur.as_raw_fd(),
                seg_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if next_raw >= 0 {
            // SAFETY: `next_raw >= 0` is a newly opened directory descriptor.
            cur = unsafe { OwnedFd::from_raw_fd(next_raw) };
            continue;
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::ENOENT) => {
                if !create {
                    return Ok(None);
                }
                // SAFETY: `cur` is open and `seg_c` is a valid C string.
                let mk = unsafe { libc::mkdirat(cur.as_raw_fd(), seg_c.as_ptr(), 0o755) };
                if mk < 0 {
                    let mk_err = io::Error::last_os_error();
                    if mk_err.raw_os_error() != Some(libc::EEXIST) {
                        return Err(mk_err);
                    }
                }
                // SAFETY: Re-open with `O_NOFOLLOW` so a concurrent symlink race fails closed.
                let created_raw = unsafe {
                    libc::openat(
                        cur.as_raw_fd(),
                        seg_c.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if created_raw < 0 {
                    return Err(io::Error::last_os_error());
                }
                // SAFETY: `created_raw >= 0` is owned exclusively here.
                let created_fd = unsafe { OwnedFd::from_raw_fd(created_raw) };
                if mk == 0 {
                    fchown_fd(created_fd.as_raw_fd(), owner, we_are_root)?;
                }
                cur = created_fd;
            }
            Some(libc::ELOOP) | Some(libc::ENOTDIR) => {
                // Allow a root-owned top-level administrative symlink (e.g. `/home -> /mnt/home`),
                // never a user-owned symlink or any symlink inside `.config`, `.local`, or `etc`.
                let mut st: libc::stat = unsafe { std::mem::zeroed() };
                // SAFETY: `cur` is open, `seg_c` is valid, and `st` is writable `libc::stat`.
                let st_rc = unsafe {
                    libc::fstatat(
                        cur.as_raw_fd(),
                        seg_c.as_ptr(),
                        &mut st,
                        libc::AT_SYMLINK_NOFOLLOW,
                    )
                };
                let is_root_top_symlink = st_rc == 0
                    && (st.st_mode & libc::S_IFMT) == libc::S_IFLNK
                    && st.st_uid == 0
                    && !in_sensitive;
                if !is_root_top_symlink {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("refusing to follow untrusted symlink component {:?}", seg),
                    ));
                }
                // SAFETY: Verified root-owned top-level symlink; open directory target.
                let followed = unsafe {
                    libc::openat(
                        cur.as_raw_fd(),
                        seg_c.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
                    )
                };
                if followed < 0 {
                    return Err(io::Error::last_os_error());
                }
                // SAFETY: `followed >= 0` is owned exclusively here.
                cur = unsafe { OwnedFd::from_raw_fd(followed) };
            }
            _ => return Err(err),
        }
    }
    Ok(Some(cur))
}

fn fchown_fd(fd: libc::c_int, owner: Option<(u32, u32)>, we_are_root: bool) -> io::Result<()> {
    let Some((uid, gid)) = owner else {
        return Ok(());
    };
    if !we_are_root {
        return Ok(());
    }
    // SAFETY: `fd` is an open descriptor owned by the caller; `fchown` never follows paths.
    let rc = unsafe { libc::fchown(fd, uid, gid) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn read_regular_nofollow(path: &Path, we_are_root: bool) -> io::Result<Option<String>> {
    use std::io::Read as _;
    use std::os::fd::{AsRawFd, FromRawFd};

    let (parent, leaf) = leaf_name(path)?;
    let Some(dir_fd) = open_dir_secure(parent, false, None, we_are_root)? else {
        return Ok(None);
    };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `dir_fd` is open, `leaf` is valid C string, `st` is valid output storage.
    let st_rc = unsafe {
        libc::fstatat(
            dir_fd.as_raw_fd(),
            leaf.as_ptr(),
            &mut st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if st_rc < 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        return Err(e);
    }
    if (st.st_mode & libc::S_IFMT) != libc::S_IFREG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "existing entry is a symlink or non-regular file",
        ));
    }
    // SAFETY: `dir_fd` is open and `O_NOFOLLOW` prevents racing replacement with a symlink.
    let raw = unsafe {
        libc::openat(
            dir_fd.as_raw_fd(),
            leaf.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw >= 0` is a freshly opened regular-file descriptor.
    let mut file = unsafe { std::fs::File::from_raw_fd(raw) };
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(Some(text))
}

fn read_link_secure(path: &Path, we_are_root: bool) -> io::Result<Option<PathBuf>> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStringExt;

    let (parent, leaf) = leaf_name(path)?;
    let Some(dir_fd) = open_dir_secure(parent, false, None, we_are_root)? else {
        return Ok(None);
    };
    let mut buf = vec![0u8; 4096];
    // SAFETY: `dir_fd` is open, `leaf` is valid C string, `buf` has `buf.len()` writable bytes.
    let n = unsafe {
        libc::readlinkat(
            dir_fd.as_raw_fd(),
            leaf.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
        )
    };
    if n < 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        return Err(e);
    }
    buf.truncate(n as usize);
    Ok(Some(PathBuf::from(std::ffi::OsString::from_vec(buf))))
}

fn write_file_secure(
    path: &Path,
    bytes: &[u8],
    owner: Option<(u32, u32)>,
    we_are_root: bool,
) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::fd::{AsRawFd, FromRawFd};

    let (parent, leaf) = leaf_name(path)?;
    let dir_fd = open_dir_secure(parent, true, owner, we_are_root)?
        .expect("create=true always yields a directory fd");

    let mut tmp_os = path.file_name().unwrap_or_default().to_os_string();
    tmp_os.push(".tmp-install");
    let tmp_c = cstring_os(&tmp_os)?;

    // Remove any stale `.tmp-install` entry (including a user-planted symlink)
    // without following it before opening `O_CREAT | O_EXCL | O_NOFOLLOW`.
    // SAFETY: `dir_fd` is open and `tmp_c` is NUL-terminated; flags=0 unlinks non-directories.
    let _ = unsafe { libc::unlinkat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), 0) };

    // SAFETY: `dir_fd` is open; `O_CREAT | O_EXCL | O_NOFOLLOW` guarantees a brand-new inode.
    let raw = unsafe {
        libc::openat(
            dir_fd.as_raw_fd(),
            tmp_c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o644,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw >= 0` is owned exclusively here.
    let mut file = unsafe { std::fs::File::from_raw_fd(raw) };
    let write_res = (|| -> io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        fchown_fd(file.as_raw_fd(), owner, we_are_root)?;
        Ok(())
    })();
    drop(file);
    if let Err(e) = write_res {
        // SAFETY: Clean up partial temp file inside `dir_fd`.
        let _ = unsafe { libc::unlinkat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), 0) };
        return Err(e);
    }

    // SAFETY: `dir_fd` is open; `renameat` atomically replaces `leaf` without following symlinks.
    let ren = unsafe {
        libc::renameat(
            dir_fd.as_raw_fd(),
            tmp_c.as_ptr(),
            dir_fd.as_raw_fd(),
            leaf.as_ptr(),
        )
    };
    if ren != 0 {
        let e = io::Error::last_os_error();
        let _ = unsafe { libc::unlinkat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), 0) };
        return Err(e);
    }
    Ok(())
}

fn write_symlink_secure(
    path: &Path,
    target: &Path,
    owner: Option<(u32, u32)>,
    we_are_root: bool,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let (parent, leaf) = leaf_name(path)?;
    let target_c = cstring_os(target.as_os_str())?;
    let dir_fd = open_dir_secure(parent, true, owner, we_are_root)?
        .expect("create=true always yields a directory fd");

    if read_link_secure(path, we_are_root)?.as_deref() != Some(target) {
        // SAFETY: `dir_fd` is open and `leaf` is valid; removes stale symlink/file without following it.
        let ul = unsafe { libc::unlinkat(dir_fd.as_raw_fd(), leaf.as_ptr(), 0) };
        if ul != 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ENOENT) {
                return Err(e);
            }
        }
        // SAFETY: `target_c` and `leaf` are valid C strings; creates symlink inside `dir_fd`.
        let sl = unsafe { libc::symlinkat(target_c.as_ptr(), dir_fd.as_raw_fd(), leaf.as_ptr()) };
        if sl != 0 {
            return Err(io::Error::last_os_error());
        }
    }

    if let Some((uid, gid)) = owner {
        if we_are_root {
            // SAFETY: `AT_SYMLINK_NOFOLLOW` chowns the symlink itself, never its target.
            let rc = unsafe {
                libc::fchownat(
                    dir_fd.as_raw_fd(),
                    leaf.as_ptr(),
                    uid,
                    gid,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

fn append_fstab_secure(path: &Path, line: &str, we_are_root: bool) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::fd::{AsRawFd, FromRawFd};

    let (parent, leaf) = leaf_name(path)?;
    let dir_fd = open_dir_secure(parent, true, None, we_are_root)?
        .expect("create=true always yields a directory fd");
    // SAFETY: `O_NOFOLLOW` prevents appending to an arbitrary file via a symlinked fstab.
    let raw = unsafe {
        libc::openat(
            dir_fd.as_raw_fd(),
            leaf.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o644,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw >= 0` is a freshly opened descriptor.
    let mut f = unsafe { std::fs::File::from_raw_fd(raw) };
    writeln!(
        f,
        "# OneDrive hydration sync root. noauto on purpose: the mount \
         must not exist\n# before the helper that marks it (see \
         hydrationd.service).\n{line}"
    )?;
    f.sync_all()
}

fn remove_file_secure(path: &Path, we_are_root: bool) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    let (parent, leaf) = leaf_name(path)?;
    let Some(dir_fd) = open_dir_secure(parent, false, None, we_are_root)? else {
        return Ok(false);
    };
    // SAFETY: `dir_fd` is verified free of untrusted symlinks; `unlinkat` never follows `leaf`.
    let rc = unsafe { libc::unlinkat(dir_fd.as_raw_fd(), leaf.as_ptr(), 0) };
    if rc == 0 {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::ENOENT) {
        Ok(false)
    } else {
        Err(e)
    }
}
