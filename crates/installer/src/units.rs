//! The unit texts, their rendering, and the scan that refuses to write a
//! helper unit that would sandbox itself into uselessness.
//!
//! The templates are copies of the hand-written units that were verified on a
//! real deployment across a real reboot, with the installation-time facts
//! turned into tokens and the machine-specific asides generalized. Their
//! comments are kept: each one records a measurement or a failure that was
//! paid for once already, and a generated file that silently dropped them
//! would invite exactly the edits they exist to prevent.

use crate::Facts;
use std::path::Path;

/// The five namespace-creating directives, measured one at a time: each alone
/// gives the unit its own mount namespace (verified by comparing
/// `/proc/self/ns/mnt` against the host), after which the helper marks a
/// private copy of the sync mount, systemd reports the unit active, and every
/// read from the user's session returns the zeros a placeholder is made of.
/// It happened twice on a real deployment before it was understood.
pub const MEASURED_NAMESPACE_DIRECTIVES: [&str; 5] = [
    "PrivateTmp",
    "PrivateNetwork",
    "ProtectKernelTunables",
    "ProtectControlGroups",
    "ProtectKernelModules",
];

/// Directives systemd.exec documents as implemented with file system
/// namespacing. Not measured here one by one — the five above were — but a
/// scan that stopped at the measured list would wave through `PrivateMounts=`,
/// whose entire purpose is the namespace. Erring toward refusal is the right
/// direction for a fail-open hazard.
pub const DOCUMENTED_NAMESPACE_DIRECTIVES: [&str; 20] = [
    "PrivateMounts",
    "PrivateDevices",
    "PrivateUsers",
    "ProtectHome",
    "ProtectSystem",
    "ProtectProc",
    "ProcSubset",
    "ProtectKernelLogs",
    "ReadWritePaths",
    "ReadOnlyPaths",
    "InaccessiblePaths",
    "ExecPaths",
    "NoExecPaths",
    "BindPaths",
    "BindReadOnlyPaths",
    "TemporaryFileSystem",
    "MountAPIVFS",
    "MountFlags",
    "RootDirectory",
    "RootImage",
];

/// One rendered unit file, by its final basename.
#[derive(Debug, Clone)]
pub struct UnitFile {
    pub name: String,
    pub text: String,
}

/// The templates, overridable so a test can push a deliberately bad one
/// through the full install flow and watch it be refused. The binary always
/// uses [`Templates::default`]; there is intentionally no CLI override — a
/// template supplied at run time is exactly the drift the scan guards against.
#[derive(Debug, Clone)]
pub struct Templates {
    pub hydrationd_service: String,
    pub hydrationd_path: String,
    pub sync_service: String,
    pub dbus_service: String,
    pub tray_service: String,
}

impl Default for Templates {
    fn default() -> Self {
        Templates {
            hydrationd_service: HYDRATIOND_SERVICE.into(),
            hydrationd_path: HYDRATIOND_PATH.into(),
            sync_service: SYNC_SERVICE.into(),
            dbus_service: DBUS_SERVICE.into(),
            tray_service: TRAY_SERVICE.into(),
        }
    }
}

/// Everything the installer writes, split by which manager loads it.
#[derive(Debug, Clone)]
pub struct Rendered {
    /// Installed under `/etc/systemd/system`; these are the units that must
    /// never acquire a mount namespace, and the scan runs on exactly these.
    pub system: Vec<UnitFile>,
    /// Installed under `~user/.config/systemd/user`.
    pub user: Vec<UnitFile>,
}

impl Rendered {
    pub fn all(&self) -> impl Iterator<Item = &UnitFile> {
        self.system.iter().chain(self.user.iter())
    }
}

/// Substitute the facts into the templates.
///
/// User units get `%h`/`%t` forms when the fact lies where the specifier
/// points: the user manager expands them to the same values, the unit stays
/// correct if the home moves with the user, and it matches the hand-written
/// reference. System units never use specifiers — `%t` means `/run` there,
/// which is the kind of quiet wrongness concrete units exist to rule out.
pub fn render(t: &Templates, f: &Facts) -> Rendered {
    let mount = f.mount.to_string_lossy().into_owned();
    let socket = f.socket.to_string_lossy().into_owned();
    let home = f.home.to_string_lossy().into_owned();
    let bin = f.bin_dir.to_string_lossy().into_owned();

    let mount_user = f
        .mount
        .strip_prefix(&f.home)
        .map(|rest| format!("%h/{}", rest.to_string_lossy()))
        .unwrap_or_else(|_| mount.clone());
    let socket_user = f
        .socket
        .strip_prefix(format!("/run/user/{}", f.uid))
        .map(|rest| format!("%t/{}", rest.to_string_lossy()))
        .unwrap_or_else(|_| socket.clone());

    let sub = |template: &str| -> String {
        template
            .replace("@USER@", &f.user)
            .replace("@UID@", &f.uid.to_string())
            .replace("@HOME@", &home)
            .replace("@MOUNT_USER@", &mount_user)
            .replace("@MOUNT@", &mount)
            .replace("@SOCKET_USER@", &socket_user)
            .replace("@SOCKET@", &socket)
            .replace("@CLIENT_ID@", &f.client_id)
            .replace("@BIN@", &bin)
    };

    Rendered {
        system: vec![
            UnitFile {
                name: "hydrationd.service".into(),
                text: sub(&t.hydrationd_service),
            },
            UnitFile {
                name: "hydrationd.path".into(),
                text: sub(&t.hydrationd_path),
            },
        ],
        user: vec![
            UnitFile {
                name: "onedrive-hydration.service".into(),
                text: sub(&t.sync_service),
            },
            UnitFile {
                name: "onedrive-hydration-dbus.service".into(),
                text: sub(&t.dbus_service),
            },
            UnitFile {
                name: "onedrive-hydration-tray.service".into(),
                text: sub(&t.tray_service),
            },
        ],
    }
}

/// Every namespace-creating directive present in `text`, as
/// `(line_number, directive)`.
///
/// Runs on the generated text, not on anyone's intent: a template edit, a
/// merge, or a "helpful" hardening pass all land here before they can land in
/// `/etc`. Comment lines are skipped the way systemd itself skips them —
/// stripped of leading whitespace, then `#` or `;` — because the templates
/// legitimately *name* the forbidden directives in prose. Any value counts,
/// including `=no`: a disabled copy of the directive is a loaded gun on the
/// table, and there is no reason for one to appear.
pub fn namespace_directives(text: &str) -> Vec<(usize, String)> {
    let mut hits = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim_start();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let forbidden = MEASURED_NAMESPACE_DIRECTIVES
            .iter()
            .chain(DOCUMENTED_NAMESPACE_DIRECTIVES.iter())
            .any(|d| d.eq_ignore_ascii_case(key));
        if forbidden {
            hits.push((idx + 1, key.to_string()));
        }
    }
    hits
}

/// Whether `name` names one of the units the scan applies to: the ones that
/// run `fanotify_mark`. The sync daemon's user unit is deliberately outside
/// the scope — it marks nothing, and its `PrivateTmp=`/`ProtectSystem=` are
/// part of the verified reference (a zero read through its private copy of
/// the mount cannot become an upload, because a placeholder is never unsent
/// content — DESIGN.md §8e).
pub fn must_share_host_namespace(name: &str) -> bool {
    name.starts_with("hydrationd.")
}

/// Where each generated unit lands, relative to a prefix.
pub fn system_unit_dir(prefix: &Path) -> std::path::PathBuf {
    prefix.join("etc/systemd/system")
}

pub fn user_unit_dir(prefix: &Path, home: &Path) -> std::path::PathBuf {
    let rel = home.strip_prefix("/").unwrap_or(home);
    prefix.join(rel).join(".config/systemd/user")
}

// ---------------------------------------------------------------------------
// The templates live next to this crate as `templates/*.in`, copied from the
// hand-written units that were verified on a real deployment across a real
// reboot, with the installation-time facts turned into @TOKENS@ and the
// machine-specific asides generalized. Keeping them as files keeps them
// diffable against a deployed set (`render` exists for exactly that), and
// keeps their comments — each records a measurement or a failure that was
// paid for once — where an editor will actually see them.
// ---------------------------------------------------------------------------

const HYDRATIOND_SERVICE: &str = include_str!("../templates/hydrationd.service.in");
const HYDRATIOND_PATH: &str = include_str!("../templates/hydrationd.path.in");
const SYNC_SERVICE: &str = include_str!("../templates/onedrive-hydration.service.in");
const DBUS_SERVICE: &str = include_str!("../templates/onedrive-hydration-dbus.service.in");
const TRAY_SERVICE: &str = include_str!("../templates/onedrive-hydration-tray.service.in");
