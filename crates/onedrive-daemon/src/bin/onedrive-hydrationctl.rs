use onedrive_hydration_daemon::auth_state;
use onedrive_hydration_daemon::{control_request, runtime_socket};
use std::io;
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "usage:\n  \
         onedrive-hydrationctl [--socket <path>] status\n  \
         onedrive-hydrationctl [--socket <path>] evict <relative-path>\n  \
         onedrive-hydrationctl [--socket <path>] pin <relative-path>\n  \
         onedrive-hydrationctl [--socket <path>] unpin <relative-path>\n  \
         onedrive-hydrationctl [--socket <path>] pending <relative-dir>\n  \
         onedrive-hydrationctl hydrate <path>"
    );
    std::process::exit(2)
}

/// What the arguments asked for.
///
/// `status`/`evict`/`pin`/`unpin`/`pending` are lines the daemon answers — the
/// paths are relative to the sync root, which is where the daemon resolves and
/// confines them (`pending <dir>` lists the dehydrated files under a directory,
/// which a caller then hydrates one at a time). `hydrate` is the exception:
/// hydration happens by *reading* the file,
/// and that read must run in a process that is neither the daemon (which serves
/// the bytes) nor the helper (which answers the pre-content event) — §6a-ter. So
/// it never becomes a daemon line; this process does the read itself, and takes
/// a real filesystem path because it opens it directly.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Forward(String),
    Hydrate(String),
    Usage,
}

fn parse(positional: &[String]) -> Action {
    match positional {
        [verb] if verb == "status" => Action::Forward("status".to_owned()),
        [verb, path] if verb == "evict" && !path.is_empty() => {
            Action::Forward(format!("evict {path}"))
        }
        [verb, path]
            if (verb == "pin" || verb == "unpin" || verb == "pending") && !path.is_empty() =>
        {
            Action::Forward(format!("{verb} {path}"))
        }
        [verb, path] if verb == "hydrate" && !path.is_empty() => Action::Hydrate(path.clone()),
        _ => Action::Usage,
    }
}

/// Pull a placeholder's content down by reading it from 0 to EOF, and report the
/// disk it brought resident.
///
/// A plain sequential read is what fills a placeholder on this mount: the helper
/// answers each pre-content event from the sync daemon and widens the fetch by
/// `READAHEAD`, so reading to EOF covers the whole object and clears the mark.
/// Doing it here — in a process that is neither the daemon nor the helper — is
/// the deadlock-safe third party §6a-ter requires. Never `mmap`: that would
/// demand the whole object inside one held event.
///
/// The number reported is a measured `st_blocks` delta, never the read `count`:
/// §8d says a short read still hydrates the whole object, so the bytes that
/// became resident are the disk that appeared, not what any one `read` returned.
/// An already-resident file therefore reports `0`.
fn hydrate(path: &str) -> io::Result<u64> {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;

    let held = |p: &str| -> io::Result<u64> { Ok(std::fs::metadata(p)?.blocks() * 512) };
    let before = held(path)?;

    let mut file = std::fs::File::open(path)?;
    // READAHEAD-sized blocks: large enough not to make a syscall per page, and it
    // does not change how much the helper fetches per event either way.
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    loop {
        match file.read(&mut buf)? {
            0 => break,
            _ => continue,
        }
    }
    drop(file);

    let after = held(path)?;
    Ok(after.saturating_sub(before))
}

fn main() -> io::Result<()> {
    // The Rust runtime masks SIGPIPE, so `status | head -1` ends not in the
    // quiet death every other line-printing tool gets but in a panic from
    // inside println! ("failed printing to stdout: Broken pipe", measured
    // 2026-08-25). This binary's stdout is a pipe more often than a terminal —
    // the Dolphin wrappers read its replies — and a reader that closes early
    // is asking us to stop, not a bug to report. Restore the default before
    // the first print.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };

    let mut args = std::env::args().skip(1);
    let mut socket = None;
    let mut positional = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--socket" {
            socket = Some(PathBuf::from(args.next().unwrap_or_else(|| usage())));
        } else {
            positional.push(arg);
        }
    }

    let command = match parse(&positional) {
        Action::Forward(c) => c,
        Action::Hydrate(path) => {
            // Never forwarded — the read is the whole point, and it happens here.
            return match hydrate(&path) {
                Ok(bytes) => {
                    println!("hydrated {bytes} bytes");
                    Ok(())
                }
                Err(e) => {
                    println!("error: {e}");
                    std::process::exit(1);
                }
            };
        }
        Action::Usage => usage(),
    };

    let socket = socket
        .map(Ok)
        .unwrap_or_else(|| runtime_socket("onedrive-hydration.ctl"))?;
    let reply = control_request(&socket, &command)?;
    println!("{reply}");
    if command == "status" {
        // The sign-in state lives on the daemon's second socket, next to
        // the control socket, because the credential is product knowledge
        // the framework's status verb cannot answer. A daemon built before
        // that socket existed still answers everything above, so its
        // absence is reported with what actually happened rather than
        // being papered over or spelled as failure.
        match control_request(&auth_state::auth_socket(&socket), "status") {
            Ok(line) => println!("{line}"),
            Err(e) => println!(
                "sign-in: unknown — the sign-in state socket did not answer ({e}); \
                 the daemon is stopped or predates it"
            ),
        }
    }
    if reply.starts_with("error:") || reply.starts_with("unknown command:") {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// `pin`/`unpin` forward verbatim, exactly like `evict`, spaces and all — the
    /// daemon splits on the first space, so a path with spaces must arrive whole.
    #[test]
    fn pin_and_unpin_forward_verbatim() {
        assert_eq!(
            parse(&v(&["pin", "Photos/report final.raw"])),
            Action::Forward("pin Photos/report final.raw".to_owned())
        );
        assert_eq!(
            parse(&v(&["unpin", "Photos"])),
            Action::Forward("unpin Photos".to_owned())
        );
        assert_eq!(
            parse(&v(&["evict", "a/b.bin"])),
            Action::Forward("evict a/b.bin".to_owned())
        );
        // `pending` forwards too — it is a daemon-answered query, not a local read.
        assert_eq!(
            parse(&v(&["pending", "Photos"])),
            Action::Forward("pending Photos".to_owned())
        );
    }

    /// The crux: `hydrate` is handled *in this process*, not turned into a daemon
    /// line. A regression that forwarded it would put the triggering read on the
    /// daemon — the ninth disguise of §6a-ter.
    #[test]
    fn hydrate_is_handled_locally_never_forwarded() {
        assert_eq!(
            parse(&v(&["hydrate", "Photos/big.raw"])),
            Action::Hydrate("Photos/big.raw".to_owned())
        );
    }

    /// An empty path, an unknown verb, or the wrong arity is a usage error, not a
    /// half-formed command sent to the daemon.
    #[test]
    fn malformed_invocations_are_usage() {
        assert_eq!(parse(&v(&["pin"])), Action::Usage);
        assert_eq!(parse(&v(&["pin", ""])), Action::Usage);
        assert_eq!(parse(&v(&["hydrate", ""])), Action::Usage);
        assert_eq!(parse(&v(&["frobnicate", "x"])), Action::Usage);
        assert_eq!(parse(&v(&["status", "extra"])), Action::Usage);
    }

    /// Reading an already-resident file hydrates nothing and reports `0` — the
    /// st_blocks delta, not the bytes read.
    #[test]
    fn hydrating_a_plain_file_reports_zero() {
        let dir = std::env::temp_dir().join(format!("kod-hydrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("resident.bin");
        std::fs::write(&p, vec![7u8; 200 * 1024]).unwrap();
        let out = hydrate(p.to_str().unwrap()).unwrap();
        assert_eq!(out, 0, "a fully-resident file should report no new disk");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
