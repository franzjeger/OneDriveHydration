//! Tray icon: StatusNotifierItem plus DBusMenu on the session bus, driven by
//! the state service's `StateChanged` signal. See the `tray` module for the
//! interfaces and the reasoning; this binary only parses arguments and wires
//! `xdg-open`.

use onedrive_hydration_daemon::tray::{run, TrayOptions};
use std::io;
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!("usage:\n  onedrive-hydration-tray [--mount <path>]");
    std::process::exit(2)
}

fn spawn_xdg_open(arg: &std::ffi::OsStr, label: &str) {
    match std::process::Command::new("xdg-open").arg(arg).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => {
            eprintln!("onedrive-hydration-tray: could not run xdg-open {label}: {e}");
        }
    }
}

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut mount = None;
    while let Some(arg) = args.next() {
        if arg == "--mount" {
            mount = Some(PathBuf::from(args.next().unwrap_or_else(|| usage())));
        } else {
            usage();
        }
    }
    if mount.is_none() {
        // Not an error — the menu simply has no folder entry — but silence
        // would look like a defect the first time someone reaches for it.
        eprintln!(
            "onedrive-hydration-tray: no --mount given; the menu will not offer \
             \"Open OneDrive Folder\""
        );
    }

    let connection = zbus::blocking::Connection::session()
        .map_err(|e| io::Error::other(format!("could not join the session bus: {e}")))?;
    run(
        connection,
        TrayOptions {
            mount,
            open: Box::new(|path| {
                // Spawn and reap on a helper thread so the tray never blocks
                // and never accumulates `<defunct>` children.
                spawn_xdg_open(path.as_os_str(), &path.display().to_string());
            }),
            open_url: Box::new(|url| {
                spawn_xdg_open(std::ffi::OsStr::new(url), url);
            }),
        },
    )
}
