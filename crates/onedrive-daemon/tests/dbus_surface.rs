//! The D-Bus surface end to end: a scripted control socket plays the daemon
//! on one side, a generic D-Bus client sits on the other, and the two are
//! joined over a peer-to-peer zbus connection on a socketpair. Peer-to-peer
//! rather than a bus because CI has no session bus, and because a private
//! socketpair keeps the test from ever being visible to, or influenced by,
//! a real desktop session on the machine running it.

use onedrive_hydration_daemon::auth_state::CredentialState;
use onedrive_hydration_daemon::dbus::{
    publish_credential, publish_state, ControlSurface, DaemonState, BUS_NAME, INTERFACE,
    OBJECT_PATH,
};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread;

/// Serve `surface` over one end of a socketpair; return both connections,
/// server first. The server keeps no registered names, so zbus dispatches
/// calls addressed to the well-known name without a bus in the middle.
fn served_pair(
    surface: ControlSurface,
) -> (zbus::blocking::Connection, zbus::blocking::Connection) {
    let (server_stream, client_stream) = UnixStream::pair().unwrap();
    let guid = zbus::Guid::generate();
    // Both ends of the handshake have to make progress at once, so the
    // server builds on its own thread while this one builds the client.
    let server = thread::spawn(move || {
        zbus::blocking::connection::Builder::unix_stream(server_stream)
            .server(guid)
            .unwrap()
            .p2p()
            .serve_at(OBJECT_PATH, surface)
            .unwrap()
            .build()
            .unwrap()
    });
    let client = zbus::blocking::connection::Builder::unix_stream(client_stream)
        .p2p()
        .build()
        .unwrap();
    (server.join().unwrap(), client)
}

/// A client proxy the way a tray would build one, minus the bus. Property
/// caching is off so every read below is a real round trip rather than an
/// assertion about zbus's cache invalidation.
fn tray_proxy(client: &zbus::blocking::Connection) -> zbus::blocking::Proxy<'static> {
    zbus::blocking::proxy::Builder::new(client)
        .destination(BUS_NAME)
        .unwrap()
        .path(OBJECT_PATH)
        .unwrap()
        .interface(INTERFACE)
        .unwrap()
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .unwrap()
}

fn method_error(result: zbus::Result<u64>) -> (String, String) {
    match result {
        Err(zbus::Error::MethodError(name, detail, _)) => {
            (name.to_string(), detail.unwrap_or_default())
        }
        other => panic!("expected a method error, got {other:?}"),
    }
}

#[test]
fn properties_update_and_state_changed_fires_once_per_publish() {
    let (server, client) = served_pair(ControlSurface::new(PathBuf::from("/nonexistent"), None));
    let proxy = tray_proxy(&client);

    // Before any daemon contact the surface reports "not running", zeros.
    assert!(!proxy.get_property::<bool>("DaemonRunning").unwrap());
    assert_eq!(proxy.get_property::<u64>("Unsent").unwrap(), 0);

    // Subscribe first, publish second, so the signal cannot be missed.
    let mut signals = proxy.receive_signal("StateChanged").unwrap();
    let iface = server
        .object_server()
        .interface::<_, ControlSurface>(OBJECT_PATH)
        .unwrap();
    publish_state(
        &iface,
        DaemonState {
            daemon_running: true,
            unsent: 5,
            excluded: 2,
            exposures: 1,
        },
    )
    .unwrap();

    let msg = signals.next().unwrap();
    let body: (bool, u64, u64, u64) = msg.body().deserialize().unwrap();
    assert_eq!(body, (true, 5, 2, 1));

    assert!(proxy.get_property::<bool>("DaemonRunning").unwrap());
    assert_eq!(proxy.get_property::<u64>("Unsent").unwrap(), 5);
    assert_eq!(proxy.get_property::<u64>("Excluded").unwrap(), 2);
    assert_eq!(proxy.get_property::<u64>("Exposures").unwrap(), 1);
}

#[test]
fn credential_state_is_a_property_and_a_signal_of_its_own() {
    let (server, client) = served_pair(ControlSurface::new(PathBuf::from("/nonexistent"), None));
    let proxy = tray_proxy(&client);

    // Before any daemon has asserted anything, the property answers
    // "unknown" — the word for a cold read against a stopped daemon or one
    // that predates the auth-state socket.
    assert_eq!(
        proxy.get_property::<String>("CredentialState").unwrap(),
        "unknown"
    );

    // Subscribe first, publish second, so the signal cannot be missed —
    // and subscribe to StateChanged too, to pin that a credential publish
    // does not leak onto the counter signal existing subscribers decode
    // with a fixed signature.
    let mut credential_signals = proxy.receive_signal("CredentialStateChanged").unwrap();
    let mut state_signals = proxy.receive_signal("StateChanged").unwrap();
    let iface = server
        .object_server()
        .interface::<_, ControlSurface>(OBJECT_PATH)
        .unwrap();
    publish_credential(&iface, CredentialState::Rejected).unwrap();

    let msg = credential_signals.next().unwrap();
    let (value,): (String,) = msg.body().deserialize().unwrap();
    assert_eq!(value, "rejected");
    assert_eq!(
        proxy.get_property::<String>("CredentialState").unwrap(),
        "rejected"
    );

    // The counter signal fires only for counters: publish one state now and
    // assert it is the *first* StateChanged to arrive.
    publish_state(
        &iface,
        DaemonState {
            daemon_running: true,
            unsent: 1,
            excluded: 2,
            exposures: 0,
        },
    )
    .unwrap();
    let msg = state_signals.next().unwrap();
    let body: (bool, u64, u64, u64) = msg.body().deserialize().unwrap();
    assert_eq!(body, (true, 1, 2, 0));
}

#[test]
fn evict_forwards_the_exact_path_and_translates_each_reply() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("daemon.ctl");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let mut seen = Vec::new();
        for reply in [
            "reclaimed 4096 bytes\n",
            "kept: OpenByAnotherProcess\n",
            "error: no such placeholder\n",
        ] {
            let (mut conn, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(conn.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            seen.push(line);
            conn.write_all(reply.as_bytes()).unwrap();
        }
        seen
    });

    let (_server, client) = served_pair(ControlSurface::new(socket, None));
    let proxy = tray_proxy(&client);

    let bytes: u64 = proxy
        .call("Evict", &("Documents/report final.pdf",))
        .unwrap();
    assert_eq!(bytes, 4096);

    let (name, detail) = method_error(proxy.call::<_, _, u64>("Evict", &("Documents/kept.pdf",)));
    assert_eq!(name, "io.github.franzjeger.OneDriveHydration.Error.Kept");
    assert_eq!(detail, "OpenByAnotherProcess");

    let (name, detail) = method_error(proxy.call::<_, _, u64>("Evict", &("gone",)));
    assert_eq!(name, "io.github.franzjeger.OneDriveHydration.Error.Failed");
    assert_eq!(detail, "no such placeholder");

    // The daemon saw exactly the paths the D-Bus caller sent — spaces kept,
    // nothing rewritten, one line per request.
    assert_eq!(
        daemon.join().unwrap(),
        [
            "evict Documents/report final.pdf\n",
            "evict Documents/kept.pdf\n",
            "evict gone\n",
        ]
    );
}

#[test]
fn evict_reports_an_absent_daemon_as_daemon_unavailable() {
    let dir = tempfile::tempdir().unwrap();

    // No socket file at all.
    let missing = dir.path().join("never-bound.ctl");
    let (_server, client) = served_pair(ControlSurface::new(missing, None));
    let (name, _) = method_error(tray_proxy(&client).call::<_, _, u64>("Evict", &("x",)));
    assert_eq!(
        name,
        "io.github.franzjeger.OneDriveHydration.Error.DaemonUnavailable"
    );

    // A stale socket file whose daemon is gone: connection refused.
    let stale = dir.path().join("stale.ctl");
    drop(UnixListener::bind(&stale).unwrap());
    let (_server, client) = served_pair(ControlSurface::new(stale, None));
    let (name, _) = method_error(tray_proxy(&client).call::<_, _, u64>("Evict", &("x",)));
    assert_eq!(
        name,
        "io.github.franzjeger.OneDriveHydration.Error.DaemonUnavailable"
    );
}

#[test]
fn evict_refuses_paths_the_line_protocol_cannot_carry() {
    let (_server, client) = served_pair(ControlSurface::new(PathBuf::from("/nonexistent"), None));
    let proxy = tray_proxy(&client);
    for path in ["", "a\nstatus", "a\r"] {
        let (name, _) = method_error(proxy.call::<_, _, u64>("Evict", &(path,)));
        assert_eq!(
            name,
            "io.github.franzjeger.OneDriveHydration.Error.InvalidPath"
        );
    }
    // A NUL never even reaches the gate: D-Bus strings cannot carry it, so
    // the object server rejects the message at deserialization. Any error is
    // acceptable here — the invariant that matters is that no command line
    // was fabricated, and the socket for this surface does not even exist.
    let (name, _) = method_error(proxy.call::<_, _, u64>("Evict", &("a\0b",)));
    assert!(!name.is_empty());
}

#[test]
fn evict_fails_closed_when_the_caller_cannot_be_attributed() {
    // Enforcement on: over peer-to-peer there is no bus driver to identify
    // the caller, so the surface must deny rather than shrug — the same
    // decision it would make on a bus that cannot report a uid.
    let (_server, client) = served_pair(ControlSurface::new(
        PathBuf::from("/nonexistent"),
        Some(12345),
    ));
    let (name, detail) = method_error(tray_proxy(&client).call::<_, _, u64>("Evict", &("x",)));
    assert_eq!(name, "io.github.franzjeger.OneDriveHydration.Error.Denied");
    assert!(detail.contains("no bus identity"), "detail: {detail}");
}
