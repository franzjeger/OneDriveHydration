//! Measurements of the machine, kept separate from the judgments in
//! [`crate::plan`] so that every judgment can be exercised in a test by
//! handing it a measurement that did not come from this machine.
//!
//! The mount-table functions mirror `crates/hydrationd/src/exposure.rs` and
//! `selfcheck.rs` in HydrationAPI deliberately: hydrationd is the runtime
//! authority on what counts as a bypass, and an installer that disagreed with
//! it would approve deployments the helper then spends its life warning about.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `FAN_CLASS_PRE_CONTENT` has existed since 2011; it is the *mark mask*
/// `FAN_PRE_ACCESS` that is new in Linux 6.14. Constants mirror
/// HydrationAPI's `crates/hydrationd/src/fanotify.rs`, the code whose
/// requirements this probe exists to predict.
const FAN_CLASS_PRE_CONTENT: libc::c_uint = 0x0000_0008;
const FAN_CLOEXEC: libc::c_uint = 0x0000_0001;
const FAN_MARK_ADD: libc::c_uint = 0x0000_0001;
const FAN_MARK_REMOVE: libc::c_uint = 0x0000_0002;
const FAN_PRE_ACCESS: u64 = 0x0010_0000;

/// What the kernel said, or what had to be inferred, about pre-content events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelSupport {
    /// `fanotify_mark(FAN_PRE_ACCESS)` was accepted (or refused with
    /// `EOPNOTSUPP`, which only a kernel that knows the flag can say).
    Measured { release: String },
    /// The syscall probe needs `CAP_SYS_ADMIN`; without it the answer is read
    /// off the release string. Weaker, and says so.
    Inferred { release: String },
    /// The kernel rejected the mask with `EINVAL`: it predates pre-content
    /// events. Measured, not guessed.
    TooOldMeasured { release: String },
    /// Release string parses below 6.14 and the syscall could not be tried.
    TooOldInferred { release: String },
    /// Neither a measurement nor a version could be had. Treated as a refusal:
    /// "could not tell" must never install.
    Unknown { detail: String },
}

impl KernelSupport {
    pub fn supported(&self) -> bool {
        matches!(
            self,
            KernelSupport::Measured { .. } | KernelSupport::Inferred { .. }
        )
    }
}

fn uname_release() -> Option<String> {
    let mut un: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut un) } != 0 {
        return None;
    }
    let bytes: Vec<u8> = un
        .release
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8(bytes).ok()
}

/// First two numeric components of a release string, tolerant of the suffixes
/// real kernels carry ("6.14.0-rc2-1-cachyos").
pub fn parse_release(release: &str) -> Option<(u32, u32)> {
    let mut parts = release.split(|c: char| !c.is_ascii_digit());
    let maj = parts.next()?.parse().ok()?;
    let min = parts.next()?.parse().ok()?;
    Some((maj, min))
}

/// Judgment on an already-obtained release string; split out so the version
/// path can be fed releases this machine will never have.
pub fn support_from_release(release: Option<String>) -> KernelSupport {
    let Some(release) = release else {
        return KernelSupport::Unknown {
            detail: "uname() failed and the syscall probe was unavailable".into(),
        };
    };
    match parse_release(&release) {
        Some((maj, min)) if (maj, min) >= (6, 14) => KernelSupport::Inferred { release },
        Some(_) => KernelSupport::TooOldInferred { release },
        None => KernelSupport::Unknown {
            detail: format!("could not parse kernel release {release:?}"),
        },
    }
}

/// Ask the kernel itself whether `FAN_PRE_ACCESS` exists.
///
/// The probe marks a file this process just created in the temp directory —
/// never the sync root. Marking a path on a live deployment would add a second
/// pre-content group to a mount that real readers are using; a probe must not
/// be able to stall them even for microseconds. A private file nobody else
/// reads carries no such risk, and the *kernel* answer does not depend on the
/// filesystem: a pre-6.14 kernel rejects the unknown mask bit with `EINVAL`
/// before it ever looks at the filesystem, while `EOPNOTSUPP` ("I know this
/// flag, this filesystem does not allow it") can only come from a kernel that
/// has the feature. Whether the *sync root's* filesystem qualifies is the
/// separate `SB_I_ALLOW_HSM` check against the mount table.
pub fn kernel_precontent_support() -> KernelSupport {
    let release = uname_release();
    let fd = unsafe {
        libc::fanotify_init(
            FAN_CLASS_PRE_CONTENT | FAN_CLOEXEC,
            (libc::O_RDONLY | libc::O_LARGEFILE) as libc::c_uint,
        )
    };
    if fd < 0 {
        let err = io::Error::last_os_error();
        return match err.raw_os_error() {
            // Needs CAP_SYS_ADMIN. Fall back to the version string and say so.
            Some(libc::EPERM) => support_from_release(release),
            _ => KernelSupport::Unknown {
                detail: format!("fanotify_init(FAN_CLASS_PRE_CONTENT): {err}"),
            },
        };
    }

    let probe = std::env::temp_dir().join(format!(
        "onedrive-hydration-install-probe-{}",
        std::process::id()
    ));
    let outcome = mark_probe_file(fd, &probe);
    unsafe { libc::close(fd) };
    let _ = std::fs::remove_file(&probe);

    let release_str = release.clone().unwrap_or_else(|| "unknown".into());
    match outcome {
        Ok(None) => KernelSupport::Measured {
            release: release_str,
        },
        Ok(Some(err)) => match err.raw_os_error() {
            // The flag itself was understood; the temp filesystem just refuses
            // HSM marks (tmpfs, for one). Still a measured "yes" for the kernel.
            Some(libc::EOPNOTSUPP) => KernelSupport::Measured {
                release: release_str,
            },
            Some(libc::EINVAL) => KernelSupport::TooOldMeasured {
                release: release_str,
            },
            _ => KernelSupport::Unknown {
                detail: format!("fanotify_mark(FAN_PRE_ACCESS) on a probe file: {err}"),
            },
        },
        Err(detail) => KernelSupport::Unknown { detail },
    }
}

/// Mark the freshly created `probe` file and report what the kernel said.
/// `Ok(None)` is acceptance; the mark is removed again immediately so nothing
/// outlives the probe, not even until the `close()` two lines later.
fn mark_probe_file(fd: libc::c_int, probe: &Path) -> Result<Option<io::Error>, String> {
    std::fs::write(probe, b"")
        .map_err(|e| format!("could not create probe file {}: {e}", probe.display()))?;
    let c = CString::new(probe.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let rc = unsafe {
        libc::fanotify_mark(fd, FAN_MARK_ADD, FAN_PRE_ACCESS, libc::AT_FDCWD, c.as_ptr())
    };
    if rc == 0 {
        unsafe {
            libc::fanotify_mark(
                fd,
                FAN_MARK_REMOVE,
                FAN_PRE_ACCESS,
                libc::AT_FDCWD,
                c.as_ptr(),
            )
        };
        return Ok(None);
    }
    Ok(Some(io::Error::last_os_error()))
}

/// One row of `/proc/self/mountinfo`, as far as the checks care.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountRow {
    pub devno: String,
    /// The subtree of the filesystem this mount exposes.
    pub root: String,
    pub point: String,
    pub fstype: String,
    /// `fs_spec`: the device, needed only to compose an fstab suggestion.
    pub source: String,
    /// Superblock options; carries `subvol=` on btrfs.
    pub super_options: String,
}

/// Parse the full table. Format: `id parent major:minor root point opts
/// [optional...] - fstype source super_opts`; the optional fields vary in
/// count, which is what the ` - ` separator exists for.
pub fn parse_mountinfo(text: &str) -> Vec<MountRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let l: Vec<&str> = left.split_whitespace().collect();
        let r: Vec<&str> = right.split_whitespace().collect();
        if l.len() < 5 || r.len() < 3 {
            continue;
        }
        rows.push(MountRow {
            devno: l[2].to_string(),
            root: unescape(l[3]),
            point: unescape(l[4]),
            fstype: r[0].to_string(),
            source: unescape(r[1]),
            super_options: r[2].to_string(),
        });
    }
    rows
}

/// Octal escapes in `/proc/self/mountinfo` and fstab: space, tab, newline,
/// backslash. Copied from HydrationAPI `selfcheck.rs` so the two sides read
/// the same table the same way.
pub fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() {
            if let Some(c) = std::str::from_utf8(&b[i + 1..i + 4])
                .ok()
                .and_then(|o| u8::from_str_radix(o, 8).ok())
            {
                out.push(c as char);
                i += 4;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// The row whose mount point is exactly `path`, or `None` — and `None` is the
/// "directory, not a mount" refusal. Keyed by mount point rather than device
/// on purpose: btrfs gives every subvolume its own anonymous `st_dev`, so a
/// device comparison finds nothing, ever (measured in HydrationAPI's
/// `exposure.rs`, where the same trap once made the warning silently never
/// fire).
pub fn find_mount<'a>(rows: &'a [MountRow], path: &Path) -> Option<&'a MountRow> {
    let want = path.to_string_lossy();
    rows.iter().find(|r| r.point == want)
}

/// Whether `outer` contains `inner` as a path prefix.
fn covers(outer: &str, inner: &str) -> bool {
    if outer == "/" || outer == inner {
        return true;
    }
    inner.starts_with(outer) && inner.as_bytes().get(outer.len()) == Some(&b'/')
}

/// Every mount point other than ours through which the sync files can be
/// reached. Empty is the healthy answer; anything else is DESIGN.md §6.4a.
///
/// Symmetric on purpose, like the runtime check it mirrors: a mount of an
/// ancestor subtree reaches our files, and so does a bind of one of our
/// subdirectories somewhere else. Both directions are real bypasses.
pub fn exposures(rows: &[MountRow], ours: &MountRow) -> Vec<String> {
    let mut out = Vec::new();
    for r in rows {
        if r.point == ours.point || r.devno != ours.devno {
            continue;
        }
        if covers(&r.root, &ours.root) || covers(&ours.root, &r.root) {
            out.push(r.point.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Where this *installer* is looking from.
///
/// An installer inside a sandbox validates the sandbox: every mount answer is
/// about a private table, and the deployment it approves runs somewhere it
/// never looked. `fanotify_mark(FAN_MARK_MOUNT)` marks the vfsmount in the
/// caller's namespace, so this is not hypothetical — it is the same mechanism
/// that makes the five systemd directives fatal to the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vantage {
    /// Same view as pid 1, either by namespace identity or — when the ns links
    /// need privilege we do not have — by the mount tables being identical.
    Host,
    /// Demonstrably not the machine's view.
    Sandboxed { detail: String },
    /// Could not be compared; the caller proceeds with a printed caveat rather
    /// than refusing, mirroring `selfcheck::Reach::Unknown`'s reasoning: an
    /// unreadable `/proc/1` must not become a new way to be down.
    Unknown { detail: String },
}

pub fn vantage() -> Vantage {
    // Identity first: the ns link text carries the namespace's inode.
    if let (Ok(ours), Ok(init)) = (
        std::fs::read_link("/proc/self/ns/mnt"),
        std::fs::read_link("/proc/1/ns/mnt"),
    ) {
        return if ours == init {
            Vantage::Host
        } else {
            Vantage::Sandboxed {
                detail: format!(
                    "mount namespace {} differs from pid 1's {}",
                    ours.to_string_lossy(),
                    init.to_string_lossy()
                ),
            }
        };
    }
    // The links need privilege; the tables do not. Equal tables are not an
    // identity proof, but a *differing* table is proof of sandboxing, and an
    // equal one means every mount judgment made here holds in pid 1's view
    // too — which is what the checks actually need.
    match (
        std::fs::read_to_string("/proc/self/mountinfo"),
        std::fs::read_to_string("/proc/1/mountinfo"),
    ) {
        (Ok(ours), Ok(init)) if ours == init => Vantage::Host,
        (Ok(_), Ok(_)) => Vantage::Sandboxed {
            detail: "this process's mount table differs from pid 1's".into(),
        },
        (_, Err(e)) | (Err(e), _) => Vantage::Unknown {
            detail: format!("could not compare mount tables with pid 1: {e}"),
        },
    }
}

/// Whether Secret Service can answer for `uid`'s session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretService {
    /// `org.freedesktop.secrets` currently has an owner.
    Owned,
    /// No owner, but the bus can activate one on first use — which is exactly
    /// what enrollment will do.
    Activatable,
    Unreachable {
        detail: String,
    },
    /// Cross-user check attempted without root. Refused in a real install
    /// (root can always check); reported as a caveat in a rehearsal.
    Unverifiable {
        detail: String,
    },
}

pub fn secret_service(user: &str, uid: u32) -> SecretService {
    secret_service_at(user, uid, Path::new(&format!("/run/user/{uid}/bus")))
}

/// Split from [`secret_service`] so a test can point the very same `busctl`
/// invocation at a socket that is not there and watch the refusal fire for
/// real, rather than faking the probe result.
pub fn secret_service_at(user: &str, uid: u32, bus: &Path) -> SecretService {
    if !bus.exists() {
        return SecretService::Unreachable {
            detail: format!(
                "no session bus at {}; is {user} logged in? Enrollment fails closed without \
                 Secret Service, so the deployment cannot start from here",
                bus.display()
            ),
        };
    }
    let euid = unsafe { libc::geteuid() };
    let run = |method: &str, arg: Option<&str>| -> Result<String, String> {
        let address = format!("unix:path={}", bus.display());
        let mut cmd;
        if euid == uid {
            cmd = Command::new("busctl");
        } else if euid == 0 {
            // Another user's session bus refuses foreign uids at the auth
            // layer; asking as the user, with runuser, is the honest transport.
            cmd = Command::new("runuser");
            cmd.arg("-u").arg(user).arg("--").arg("busctl");
        } else {
            return Err(format!(
                "cannot ask uid {uid}'s session bus as uid {euid}; run as root (or as {user}) \
                 to verify Secret Service"
            ));
        }
        cmd.env("DBUS_SESSION_BUS_ADDRESS", &address)
            .arg("--user")
            .arg("call")
            .arg("org.freedesktop.DBus")
            .arg("/org/freedesktop/DBus")
            .arg("org.freedesktop.DBus")
            .arg(method);
        if let Some(a) = arg {
            cmd.arg("s").arg(a);
        }
        let out = cmd
            .output()
            .map_err(|e| format!("could not run busctl: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "busctl {method} against {address} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };

    match run("NameHasOwner", Some("org.freedesktop.secrets")) {
        Ok(reply) if reply.contains("true") => SecretService::Owned,
        Ok(_) => match run("ListActivatableNames", None) {
            Ok(names) if names.contains("\"org.freedesktop.secrets\"") => {
                SecretService::Activatable
            }
            Ok(_) => SecretService::Unreachable {
                detail: format!(
                    "org.freedesktop.secrets is neither owned nor activatable on {user}'s \
                     session bus; install a Secret Service provider (gnome-keyring, or \
                     kwalletd6 with its Secret Service API enabled) and log in once"
                ),
            },
            Err(detail) => SecretService::Unreachable { detail },
        },
        Err(detail) => {
            if euid != uid && euid != 0 {
                SecretService::Unverifiable { detail }
            } else {
                SecretService::Unreachable { detail }
            }
        }
    }
}

/// The passwd fields the facts need.
pub struct Passwd {
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

/// `getpwnam_r`, so NSS answers (not just `/etc/passwd`) and other threads'
/// lookups cannot race ours.
pub fn resolve_user(name: &str) -> Result<Passwd, String> {
    let c = CString::new(name).map_err(|e| format!("user name {name:?}: {e}"))?;
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0u8; 4096];
    loop {
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwnam_r(
                c.as_ptr(),
                &mut pwd,
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 {
            return Err(format!(
                "user {name:?}: getpwnam_r: {}",
                io::Error::from_raw_os_error(rc)
            ));
        }
        if result.is_null() {
            return Err(format!(
                "user {name:?} does not exist on this system; the units bind a numeric uid \
                 and a home directory, and neither can be guessed"
            ));
        }
        let home = unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) };
        return Ok(Passwd {
            uid: pwd.pw_uid,
            gid: pwd.pw_gid,
            home: PathBuf::from(
                std::str::from_utf8(home.to_bytes())
                    .map_err(|e| format!("user {name:?} has a non-UTF-8 home directory: {e}"))?,
            ),
        });
    }
}

/// What fstab says about the sync root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FstabEntry {
    /// Present and `noauto`: the mount cannot exist before the helper that
    /// marks it. The healthy state.
    NoAuto,
    /// Present but mounted at boot: between boot and the helper's start every
    /// placeholder is readable and zero. A refusal, not a warning.
    Automatic {
        line: String,
    },
    Absent,
}

pub fn fstab_entry(text: &str, mount: &Path) -> FstabEntry {
    let want = mount.to_string_lossy();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 || unescape(f[1]) != want {
            continue;
        }
        if f[3].split(',').any(|o| o == "noauto") {
            return FstabEntry::NoAuto;
        }
        return FstabEntry::Automatic {
            line: line.to_string(),
        };
    }
    FstabEntry::Absent
}

/// Compose the fstab line this deployment needs, from the mount that is
/// actually there. `noauto` is not optional and there is no code path that
/// omits it; `nofail` keeps a missing disk from holding boot hostage.
pub fn fstab_suggestion(row: &MountRow) -> String {
    let what = stable_device_name(&row.source);
    let opts = match row.fstype.as_str() {
        "btrfs" => {
            let subvol = row
                .super_options
                .split(',')
                .find_map(|o| o.strip_prefix("subvol="))
                .unwrap_or("/");
            format!("subvol={subvol},noatime,noauto,nofail")
        }
        _ => "noauto,nofail".to_string(),
    };
    format!("{what} {} {} {opts} 0 0", row.point, row.fstype)
}

/// Prefer `UUID=` over a device node that can be renumbered. Resolved by
/// reading `/dev/disk/by-uuid`, which needs no privilege.
fn stable_device_name(source: &str) -> String {
    let by_uuid = Path::new("/dev/disk/by-uuid");
    let Ok(entries) = std::fs::read_dir(by_uuid) else {
        return source.to_string();
    };
    for e in entries.flatten() {
        if let Ok(target) = std::fs::canonicalize(e.path()) {
            if target == Path::new(source) {
                return format!("UUID={}", e.file_name().to_string_lossy());
            }
        }
    }
    source.to_string()
}

/// The Plasma applet's package id: the name `kpackagetool6 --type Plasma/Applet`
/// installs it under and the directory it creates. Identical to the D-Bus name
/// by construction — `packaging/plasmoid/` holds the tree under exactly this
/// name, and `crates/onedrive-daemon/tests/plasmoid_package.rs` pins it there.
pub const PLASMOID_ID: &str = "io.github.franzjeger.OneDriveHydration";

/// Whether the Plasma applet is installed for the user this deployment is for.
///
/// Deliberately a package on disk and *not* "is plasmashell running". A running
/// shell is a property of the session that happens to exist while `sudo` is
/// being typed; the applet package survives reboots and desktop switches, and
/// it is what makes a second tray icon possible at all. See [`crate::units::Tray`]
/// for why the distinction decides the whole design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plasmoid {
    /// Installed, at the package directory that holds it.
    Present {
        path: PathBuf,
    },
    Absent,
}

/// Where a Plasma applet package lands, in search order: the user's own data
/// directory first — `kpackagetool6` without `--global`, which is what
/// `packaging/plasmoid/install-plasmoid.sh` runs — then the system-wide tree a
/// distribution package would use.
///
/// Measured, not taken from documentation: with the applet installed for this
/// user, `kpackagetool6 --type Plasma/Applet --show io.github.franzjeger.OneDriveHydration`
/// reported `Path: /home/frank/.local/share/plasma/plasmoids/<id>/`, and
/// `/usr/share/plasma/plasmoids` holds the shipped applets on the same machine.
pub fn plasmoid_dirs(prefix: &Path, home: &Path) -> Vec<PathBuf> {
    let rel = home.strip_prefix("/").unwrap_or(home);
    vec![
        prefix.join(rel).join(".local/share/plasma/plasmoids"),
        prefix.join("usr/share/plasma/plasmoids"),
    ]
}

/// Look for the applet under `prefix`. `metadata.json` is what is checked
/// rather than the directory: `kpackagetool6` requires that file, an empty
/// leftover directory is not an installed applet, and a tray icon that is not
/// really there must not be able to cause a refusal.
pub fn plasmoid_package(prefix: &Path, home: &Path) -> Plasmoid {
    for dir in plasmoid_dirs(prefix, home) {
        let pkg = dir.join(PLASMOID_ID);
        if pkg.join("metadata.json").is_file() {
            return Plasmoid::Present { path: pkg };
        }
    }
    Plasmoid::Absent
}

/// The applet as an *observation*, with the rehearsal rule applied: look under
/// the prefix, and when a rehearsal stages nothing there, fall back to the
/// machine's real answer.
///
/// The same rule `observe` applies to fstab, for the same reason: `--prefix` is
/// meant to rehearse *this* deployment, and the run an operator makes before
/// the real one is exactly the run that has to be able to show them the
/// collision. Split out from [`plasmoid_package`] so the fallback itself can be
/// exercised — point `home` at a directory a test controls and the `/` search
/// lands inside it.
pub fn plasmoid_observed(prefix: &Path, home: &Path) -> Plasmoid {
    match plasmoid_package(prefix, home) {
        Plasmoid::Absent if prefix != Path::new("/") => plasmoid_package(Path::new("/"), home),
        found => found,
    }
}

/// A binary the units point at, and whether it can actually be executed.
pub fn binary_state(dir: &Path, name: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    match std::fs::metadata(&p) {
        Err(e) => Err(format!("{}: {e}", p.display())),
        Ok(m) if !m.is_file() => Err(format!("{}: not a regular file", p.display())),
        Ok(m) if m.permissions().mode() & 0o111 == 0 => {
            Err(format!("{}: not executable", p.display()))
        }
        Ok(_) => Ok(()),
    }
}
