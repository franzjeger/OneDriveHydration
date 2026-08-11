//! Session-bus control service: owns `io.github.franzjeger.OneDriveHydration`
//! and translates between D-Bus and the daemon's owner-only control socket.
//! See the `dbus` module for the interface and the reasoning.

use onedrive_hydration_daemon::dbus::{
    publish_state, watch_daemon, ControlSurface, BUS_NAME, OBJECT_PATH,
};
use onedrive_hydration_daemon::runtime_socket;
use std::io;
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!("usage:\n  onedrive-hydration-dbus [--socket <path>]");
    std::process::exit(2)
}

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut socket = None;
    while let Some(arg) = args.next() {
        if arg == "--socket" {
            socket = Some(PathBuf::from(args.next().unwrap_or_else(|| usage())));
        } else {
            usage();
        }
    }
    let socket = socket
        .map(Ok)
        .unwrap_or_else(|| runtime_socket("onedrive-hydration.ctl"))?;

    // The gate `Evict` holds callers to: the same uid that owns the control
    // socket, which is the uid this service runs as.
    let owner = rustix::process::geteuid().as_raw();
    let surface = ControlSurface::new(socket.clone(), Some(owner));
    let connection = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.serve_at(OBJECT_PATH, surface))
        .and_then(|b| b.build())
        .map_err(|e| io::Error::other(format!("could not join the session bus: {e}")))?;
    // Requested after the object is served so no caller can resolve the name
    // to a connection with nothing behind it — and requested with DoNotQueue,
    // *not* through the builder: the builder requests with no flags on this
    // zbus, so a second copy of this service would queue for the name and
    // then run forever believing it serves, while owning nothing. Measured,
    // not guessed. Two watchers and a ghost service is strictly worse than
    // telling the user the service is already running.
    connection
        .request_name_with_flags(BUS_NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into())
        .map_err(|e| match e {
            zbus::Error::NameTaken => io::Error::other(format!(
                "{BUS_NAME} is already owned on the session bus; \
                 is another onedrive-hydration-dbus running?"
            )),
            e => io::Error::other(format!("could not own {BUS_NAME}: {e}")),
        })?;
    let iface = connection
        .object_server()
        .interface::<_, ControlSurface>(OBJECT_PATH)
        .map_err(io::Error::other)?;
    eprintln!(
        "onedrive-hydration-dbus: serving {BUS_NAME} at {OBJECT_PATH}, watching {}",
        socket.display()
    );

    // The bus connection runs on its own thread inside zbus; this thread's
    // whole job is holding the watch connection to the daemon.
    watch_daemon(
        &socket,
        &mut |state| {
            if let Err(e) = publish_state(&iface, state) {
                eprintln!("onedrive-hydration-dbus: could not publish a state change: {e}");
            }
        },
        &mut std::thread::sleep,
        &mut || true,
    );
    Ok(())
}
