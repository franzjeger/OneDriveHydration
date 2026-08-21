//! The tray end to end against a real message bus: a private `dbus-daemon`
//! per test, a scripted StatusNotifierWatcher, and the *actual*
//! [`ControlSurface`] playing the state service, driven through
//! `publish_state` exactly as the shipping binary drives it.
//!
//! A real bus daemon rather than the peer-to-peer harness `dbus_surface.rs`
//! uses, because everything under test here is bus behaviour: well-known
//! names, `NameOwnerChanged`, match rules that survive an owner change, and
//! registration with a watcher. None of that exists over a socketpair, and a
//! fake of it would test the fake. The bus is private per test (its address
//! is passed explicitly, never through the environment), so parallel tests
//! and the desktop session hosting the developer cannot see each other.
//!
//! `dbus-daemon` missing is a hard failure with its name in the message, not
//! a skip: a silently skipped suite reads as green while measuring nothing.

use onedrive_hydration_daemon::dbus::{
    publish_state, ControlSurface, DaemonState, BUS_NAME, OBJECT_PATH,
};
use onedrive_hydration_daemon::tray::{
    self, ToolTip, TrayOptions, ICON_EXPOSED, ICON_STOPPED, ICON_SYNCED, ICON_UNSENT, ITEM_PATH,
    MENU_PATH, WATCHER_NAME, WATCHER_PATH,
};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use zbus::zvariant::{OwnedValue, Value};

const WAIT: Duration = Duration::from_secs(10);

/// One private session bus, killed with the test.
struct PrivateBus {
    daemon: Child,
    address: String,
}

impl PrivateBus {
    fn start() -> Self {
        let mut daemon = Command::new("dbus-daemon")
            .args(["--session", "--print-address=1", "--nofork"])
            .stdout(Stdio::piped())
            .spawn()
            .expect(
                "could not run dbus-daemon — this test needs a real bus daemon on the PATH \
                 to host names and NameOwnerChanged",
            );
        let stdout = daemon.stdout.take().expect("stdout was piped");
        let mut address = String::new();
        BufReader::new(stdout)
            .read_line(&mut address)
            .expect("dbus-daemon prints its address as the first stdout line");
        Self {
            daemon,
            address: address.trim().to_owned(),
        }
    }

    fn connect(&self) -> zbus::blocking::Connection {
        zbus::blocking::connection::Builder::address(self.address.as_str())
            .expect("dbus-daemon printed a parseable address")
            .build()
            .expect("the private bus accepts connections")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// A watcher that records who registers. `IsStatusNotifierHostRegistered`
/// answers true so the tray has nothing to warn about.
struct Watcher {
    registrations: mpsc::Sender<String>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    fn register_status_notifier_item(&self, service: String) {
        self.registrations
            .send(service)
            .expect("the test holds the receiver for the whole run");
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }
}

fn start_watcher(bus: &PrivateBus) -> (zbus::blocking::Connection, mpsc::Receiver<String>) {
    let (registrations, seen) = mpsc::channel();
    let connection = bus.connect();
    connection
        .object_server()
        .at(WATCHER_PATH, Watcher { registrations })
        .unwrap();
    connection.request_name(WATCHER_NAME).unwrap();
    (connection, seen)
}

/// The real D-Bus surface from the shipping service, minus its socket: tests
/// feed it states through `publish_state`, which is exactly what the
/// binary's watch loop does.
struct StateService {
    connection: zbus::blocking::Connection,
}

impl StateService {
    fn start(bus: &PrivateBus) -> Self {
        let connection = bus.connect();
        connection
            .object_server()
            .at(
                OBJECT_PATH,
                ControlSurface::new(PathBuf::from("/nonexistent"), None),
            )
            .unwrap();
        connection.request_name(BUS_NAME).unwrap();
        Self { connection }
    }

    fn publish(&self, daemon_running: bool, unsent: u64, excluded: u64, exposures: u64) {
        let iface = self
            .connection
            .object_server()
            .interface::<_, ControlSurface>(OBJECT_PATH)
            .unwrap();
        publish_state(
            &iface,
            DaemonState {
                daemon_running,
                unsent,
                excluded,
                exposures,
                downloading: 0,
                indexing: false,
                uploading: Vec::new(),
            },
        )
        .unwrap();
    }
}

struct Tray {
    thread: thread::JoinHandle<std::io::Result<()>>,
    unique_name: String,
    opened: mpsc::Receiver<PathBuf>,
}

fn start_tray(bus: &PrivateBus, mount: Option<PathBuf>) -> Tray {
    let connection = bus.connect();
    let unique_name = connection.unique_name().unwrap().to_string();
    let (opened_tx, opened) = mpsc::channel();
    let thread = thread::spawn(move || {
        tray::run(
            connection,
            TrayOptions {
                mount,
                open: Box::new(move |path| {
                    opened_tx
                        .send(path.to_owned())
                        .expect("the test holds the receiver");
                }),
            },
        )
    });
    Tray {
        thread,
        unique_name,
        opened,
    }
}

/// A proxy onto the tray's item, from a bystander connection, with caching
/// off so every read is a real round trip.
fn item_proxy<'a>(
    connection: &zbus::blocking::Connection,
    tray: &Tray,
    path: &'a str,
    interface: &'a str,
) -> zbus::blocking::Proxy<'a> {
    zbus::blocking::proxy::Builder::new(connection)
        .destination(tray.unique_name.clone())
        .unwrap()
        .path(path)
        .unwrap()
        .interface(interface)
        .unwrap()
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .unwrap()
}

fn quit(observer: &zbus::blocking::Connection, tray: Tray) {
    let menu = item_proxy(observer, &tray, MENU_PATH, "com.canonical.dbusmenu");
    // Quit is id 5; the ids are part of the published shape under test.
    menu.call::<_, _, ()>("Event", &(5i32, "clicked", Value::from(0i32), 0u32))
        .unwrap();
    tray.thread
        .join()
        .expect("the tray thread does not panic")
        .expect("quit is a clean exit");
}

/// A `GetLayout` reply as the wire carries it: revision, then the
/// `(ia{sv}av)` root node.
type RawLayout = (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>));

fn get_layout(menu: &zbus::blocking::Proxy<'_>) -> RawLayout {
    menu.call("GetLayout", &(0i32, -1i32, Vec::<String>::new()))
        .unwrap()
}

/// Decode one `(ia{sv}av)` child out of a `GetLayout` reply.
fn decode_child(child: &OwnedValue) -> (i32, HashMap<String, OwnedValue>) {
    let structure: zbus::zvariant::Structure = child.downcast_ref().unwrap();
    let fields = structure.fields();
    let id = i32::try_from(&fields[0]).unwrap();
    let props: HashMap<String, OwnedValue> = fields[1].try_clone().unwrap().try_into().unwrap();
    (id, props)
}

fn label(props: &HashMap<String, OwnedValue>) -> Option<String> {
    props
        .get("label")
        .map(|v| String::try_from(v.try_clone().unwrap()).unwrap())
}

#[test]
fn registers_reflects_every_state_and_serves_the_menu() {
    let bus = PrivateBus::start();
    let (_watcher, registrations) = start_watcher(&bus);
    let service = StateService::start(&bus);
    let mount = tempfile::tempdir().unwrap();
    let tray = start_tray(&bus, Some(mount.path().to_owned()));

    // Registration happens only after the item is served and holds its
    // initial state, so from here every read sees a finished object.
    assert_eq!(registrations.recv_timeout(WAIT).unwrap(), tray.unique_name);

    let observer = bus.connect();
    let item = item_proxy(&observer, &tray, ITEM_PATH, "org.kde.StatusNotifierItem");

    // The service is reachable and has seen no daemon: that is "daemon not
    // running", not "state service not running" — the initial cold read has
    // to make that distinction, no signal ever fired.
    assert_eq!(
        item.get_property::<String>("IconName").unwrap(),
        ICON_STOPPED
    );
    assert_eq!(
        item.get_property::<ToolTip>("ToolTip").unwrap().title,
        "Sync daemon not running"
    );

    // From here on, everything arrives by signal.
    let mut new_icons = item.receive_signal("NewIcon").unwrap();
    let mut new_statuses = item.receive_signal("NewStatus").unwrap();

    service.publish(true, 0, 7, 0);
    new_icons.next().unwrap();
    assert_eq!(
        item.get_property::<String>("IconName").unwrap(),
        ICON_SYNCED
    );
    assert_eq!(item.get_property::<String>("Status").unwrap(), "Active");
    let tip = item.get_property::<ToolTip>("ToolTip").unwrap();
    assert_eq!(tip.title, "Up to date");
    assert!(tip.text.contains("7 files are cloud-only placeholders"));

    service.publish(true, 3, 7, 0);
    new_icons.next().unwrap();
    assert_eq!(
        item.get_property::<String>("IconName").unwrap(),
        ICON_UNSENT
    );
    assert_eq!(
        item.get_property::<ToolTip>("ToolTip").unwrap().title,
        "3 changes to upload"
    );

    // Exposures outrank the unsent work and demand attention.
    service.publish(true, 3, 7, 2);
    new_icons.next().unwrap();
    let status_signal = new_statuses.next().unwrap();
    assert_eq!(
        status_signal.body().deserialize::<(String,)>().unwrap().0,
        "NeedsAttention"
    );
    assert_eq!(
        item.get_property::<String>("IconName").unwrap(),
        ICON_EXPOSED
    );
    assert_eq!(
        item.get_property::<String>("Status").unwrap(),
        "NeedsAttention"
    );
    let tip = item.get_property::<ToolTip>("ToolTip").unwrap();
    assert_eq!(tip.title, "2 mounts bypass hydration");
    assert!(tip.text.contains("3 changes are still waiting to upload"));

    // The menu: full shape from the root, the way the measured host asks.
    let menu = item_proxy(&observer, &tray, MENU_PATH, "com.canonical.dbusmenu");
    let (_, (root_id, _, children)) = get_layout(&menu);
    assert_eq!(root_id, 0);
    let decoded: Vec<_> = children.iter().map(decode_child).collect();
    assert_eq!(
        decoded.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    assert_eq!(
        label(&decoded[0].1).as_deref(),
        Some("2 mounts bypass hydration"),
        "the status entry mirrors the current headline"
    );
    assert_eq!(
        label(&decoded[2].1).as_deref(),
        Some("Open OneDrive Folder")
    );
    assert_eq!(label(&decoded[4].1).as_deref(), Some("Quit"));

    // A folder click opens exactly the configured mount.
    menu.call::<_, _, ()>("Event", &(3i32, "clicked", Value::from(0i32), 0u32))
        .unwrap();
    assert_eq!(tray.opened.recv_timeout(WAIT).unwrap(), mount.path());

    quit(&observer, tray);
}

#[test]
fn a_watcher_restart_is_answered_with_a_fresh_registration() {
    let bus = PrivateBus::start();
    let (watcher, registrations) = start_watcher(&bus);
    let _service = StateService::start(&bus);
    let tray = start_tray(&bus, None);
    assert_eq!(registrations.recv_timeout(WAIT).unwrap(), tray.unique_name);

    // kded6 restarts: the name goes away and returns with empty state.
    drop(watcher);
    drop(registrations);
    let (_watcher, registrations) = start_watcher(&bus);
    assert_eq!(
        registrations.recv_timeout(WAIT).unwrap(),
        tray.unique_name,
        "the tray re-registers with a watcher that lost every item it held"
    );

    let observer = bus.connect();
    quit(&observer, tray);
}

#[test]
fn losing_the_state_service_is_shown_as_its_own_honest_state() {
    let bus = PrivateBus::start();
    let (_watcher, registrations) = start_watcher(&bus);
    let service = StateService::start(&bus);
    let tray = start_tray(&bus, None);
    assert_eq!(registrations.recv_timeout(WAIT).unwrap(), tray.unique_name);

    let observer = bus.connect();
    let item = item_proxy(&observer, &tray, ITEM_PATH, "org.kde.StatusNotifierItem");
    let mut new_icons = item.receive_signal("NewIcon").unwrap();

    service.publish(true, 0, 0, 0);
    new_icons.next().unwrap();
    assert_eq!(
        item.get_property::<String>("IconName").unwrap(),
        ICON_SYNCED
    );

    // The service dies. The tray must not keep showing "up to date" on a
    // feed that no longer exists.
    drop(service);
    new_icons.next().unwrap();
    assert_eq!(
        item.get_property::<String>("IconName").unwrap(),
        ICON_STOPPED
    );
    assert_eq!(
        item.get_property::<ToolTip>("ToolTip").unwrap().title,
        "State service not running"
    );

    // It returns with a running daemon and the tray recovers, signal-driven.
    let service = StateService::start(&bus);
    service.publish(true, 0, 0, 0);
    new_icons.next().unwrap();
    assert_eq!(
        item.get_property::<String>("IconName").unwrap(),
        ICON_SYNCED
    );

    // Without a mount the menu carries no folder entry: ids 1, 2, 5.
    let menu = item_proxy(&observer, &tray, MENU_PATH, "com.canonical.dbusmenu");
    let (_, (_, _, children)) = get_layout(&menu);
    let ids: Vec<i32> = children.iter().map(|c| decode_child(c).0).collect();
    assert_eq!(ids, [1, 2, 5]);

    quit(&observer, tray);
}

#[test]
fn a_desktop_without_a_watcher_is_a_named_startup_error_not_a_retry_loop() {
    let bus = PrivateBus::start();
    let _service = StateService::start(&bus);
    let tray = start_tray(&bus, None);
    let error = tray
        .thread
        .join()
        .expect("the tray thread does not panic")
        .expect_err("with no watcher there is nothing to register with");
    assert!(
        error.to_string().contains(WATCHER_NAME),
        "the error names what was missing: {error}"
    );
}
