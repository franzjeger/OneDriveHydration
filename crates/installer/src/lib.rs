//! A systemd installer whose real product is its refusals.
//!
//! `packaging/systemd/README.md` rejects shipping generic units: a unit for
//! this deployment must bind one specific user's mount, numeric uid and runtime
//! socket, and a deployment whose facts are wrong does not fail — it goes
//! *green* while every read returns the zeros a placeholder is made of. That is
//! the failure mode this whole stack exists to prevent, and it is reachable
//! from an installer in at least six distinct ways (wrong kernel, wrong
//! filesystem, a directory instead of a mount, a second mount over the same
//! files, a sandboxed helper unit, an unreachable Secret Service).
//!
//! So the installer's job is split exactly like the daemon's own self-checks:
//! measure first, refuse loudly, and only then write. Every check here either
//! fires in a test in `tests/refusals.rs` or does not ship — the repository
//! standard is that a refusal which has never been seen to fire is not a check.
//!
//! What this tool never does, by design (each is printed, not performed):
//!
//! * create or delete the btrfs subvolume — that is the user's storage layout
//!   and a destructive operation; the exact command is printed instead;
//! * touch `/etc/fstab` without `--consent-fstab`, and never without `noauto`;
//! * enroll credentials or invent a client id — `--client-id` is required and
//!   is public configuration, never a secret.

pub mod plan;
pub mod probes;
pub mod units;

use std::path::PathBuf;

/// The installation-time facts a concrete deployment binds.
///
/// These are the values `packaging/systemd/README.md` says a generic unit
/// cannot carry: one user, that user's numeric uid, their sync root and their
/// runtime socket. Everything else is derived from them.
#[derive(Debug, Clone)]
pub struct Facts {
    pub user: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
    /// The sync root. Must be its own mount on ext4, btrfs or xfs; validated,
    /// never assumed.
    pub mount: PathBuf,
    /// Derived from the uid — `/run/user/{uid}/onedrive-hydration.sock` — and
    /// deliberately not an input. A socket path that disagrees with the uid it
    /// serves is one of the silent fail-open shapes.
    pub socket: PathBuf,
    /// Public configuration, not a secret; required so this tool never embeds
    /// an id it invented.
    pub client_id: String,
    /// Where the payload binaries live at runtime. Checked, since a unit whose
    /// `ExecStart=` points at nothing fails only at boot.
    pub bin_dir: PathBuf,
}

impl Facts {
    /// Resolve the facts from a user name. Refuses (with the `getpwnam`
    /// failure) rather than guessing: every generated path hangs off the
    /// uid and home directory, so an unresolved user has nothing to install.
    pub fn resolve(
        user: &str,
        mount: PathBuf,
        client_id: String,
        bin_dir: PathBuf,
    ) -> Result<Self, String> {
        let pw = probes::resolve_user(user)?;
        let socket = PathBuf::from(format!("/run/user/{}/onedrive-hydration.sock", pw.uid));
        Ok(Facts {
            user: user.to_string(),
            uid: pw.uid,
            gid: pw.gid,
            home: pw.home,
            mount,
            socket,
            client_id,
            bin_dir,
        })
    }
}
