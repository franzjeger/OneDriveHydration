//! The tray icon: a StatusNotifierItem plus a DBusMenu, spoken directly over
//! zbus. No GUI toolkit — the panel draws everything, we only publish state.
//!
//! This is the subscriber the D-Bus surface in [`crate::dbus`] exists for. It
//! holds no state of its own beyond the last presentation it showed: one
//! initial property read at startup (and again when the state service returns
//! to the bus, because a freshly started service does not signal a state it
//! considers unchanged), then `StateChanged` signals only. It never polls.
//!
//! The interface shapes below — property set, method set, the `(ia{sv}av)`
//! menu layout — were taken from a working item on this machine's Plasma
//! 6.7.4 session (`busctl --user introspect` of an existing tray item and a
//! `GetLayout` call against its menu), not from reading the specification
//! alone. Where the spec and the measured host disagree, the measurement won.
//!
//! Lifecycle, degrading honestly rather than quietly:
//!
//! * No `org.kde.StatusNotifierWatcher` on the bus at startup means this
//!   desktop has no StatusNotifier tray. [`run`] returns an error naming
//!   exactly that, and the binary exits; retrying forever against a desktop
//!   that will never show the icon would be silent failure with extra steps.
//! * The watcher dying *later* is different: plasmashell and kded6 restart
//!   routinely, and an item that exited with them would never be seen again.
//!   We watch `NameOwnerChanged` for the watcher's name and re-register when
//!   a new owner appears. No busy loop — the signal is the retry.
//! * A watcher present but with no host registered displays nothing. That is
//!   warned about once at startup; the watcher announces hosts when they
//!   arrive and our registration stays valid, so there is nothing to redo.
//! * The state service vanishing from the bus is shown as its own state
//!   ("state service not running") distinct from "daemon not running" —
//!   collapsing the two would hide which process needs starting.
//!
//! Eviction is deliberately absent from the menu. `Evict` needs a path, a
//! path needs a file picker, and a file picker needs a GUI toolkit this
//! binary intentionally does not link. It belongs to the flyout, which owns
//! that decision.

use crate::auth_state::CredentialState;
use crate::dbus::{DaemonState, BUS_NAME, INTERFACE, OBJECT_PATH};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::thread;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{self, ObjectPath, OwnedValue, Value};

/// The watcher every StatusNotifier desktop owns, and the object path it
/// serves. Fixed by the StatusNotifierItem specification.
pub const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
/// See [`WATCHER_NAME`].
pub const WATCHER_PATH: &str = "/StatusNotifierWatcher";
/// Where our item lives. Hosts hard-code this path when an item registers by
/// bus name alone, which is how we register.
pub const ITEM_PATH: &str = "/StatusNotifierItem";
/// Where our menu lives; the item's `Menu` property points here.
pub const MENU_PATH: &str = "/MenuBar";

/// Icon names, resolved by the host through the hicolor icon theme.
/// `packaging/icons/` carries the SVGs and the script that installs them.
pub const ICON_SYNCED: &str = "onedrive-hydration-synced";
/// See [`ICON_SYNCED`].
pub const ICON_UNSENT: &str = "onedrive-hydration-unsent";
/// See [`ICON_SYNCED`].
pub const ICON_EXPOSED: &str = "onedrive-hydration-exposed";
/// See [`ICON_SYNCED`].
pub const ICON_STOPPED: &str = "onedrive-hydration-stopped";
/// The launcher icon, used for the tooltip.
pub const ICON_APP: &str = "onedrive-hydration";

/// Everything the panel shows, derived from one [`DaemonState`] (or from the
/// state service being absent). Pure, so the mapping is testable without a
/// bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presentation {
    /// Theme icon name for the item.
    pub icon: &'static str,
    /// StatusNotifierItem `Status`: `"Active"`, or `"NeedsAttention"` for the
    /// exposure hazard so the host renders it prominently.
    pub sni_status: &'static str,
    /// One line: the menu's status entry and the tooltip title.
    pub headline: String,
    /// A sentence or two of tooltip body.
    pub detail: String,
}

fn count(n: u64, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {plural}")
    }
}

/// The placeholders line shown while things are healthy. `excluded` counts
/// files excluded from backup because they are placeholders — from the
/// user's side, the files that live in the cloud and hydrate on first read.
fn placeholders_line(excluded: u64) -> String {
    match excluded {
        0 => String::new(),
        1 => " 1 file is a cloud-only placeholder.".to_owned(),
        n => format!(" {n} files are cloud-only placeholders."),
    }
}

/// The caveat appended to every running-state detail while the daemon
/// reports it cannot persist the rotated credential. A caveat and not a
/// state of its own: syncing still works, so the headline stays about the
/// work, and the future cost travels in the sentence that names the fix.
fn store_caveat(credential: CredentialState) -> &'static str {
    match credential {
        CredentialState::Unsaved => {
            " Warning: the sign-in works but its rotation could not be saved to Linux Secret \
             Service — unlock the keyring, or the next daemon start may require signing in \
             again."
        }
        _ => "",
    }
}

/// Map what we know to what the panel shows. `None` means the state service
/// itself is not on the bus, which is worth distinguishing from a reachable
/// service reporting a stopped daemon: they name different processes to
/// start.
///
/// Precedence, most urgent knowledge first:
///
/// 1. Service absent — nothing else is knowable.
/// 2. Daemon not running — the counters are last-seen values, so they are
///    only quoted ("before it stopped"), never presented as current. The
///    credential state is not even quoted: a stopped daemon cannot tell a
///    missing credential from a locked keyring, and a sign-in instruction
///    backed by a dead process would send someone to re-enroll over a
///    keyring that merely has not unlocked yet.
/// 3. Exposures — a warning state that outranks progress: another mount
///    exposes the sync files, reads through it bypass hydration entirely and
///    can silently return placeholder zeros instead of content (HydrationAPI
///    DESIGN.md §6.4a). The framework can detect this but not prevent it,
///    which is exactly why the tray must not sit on it. It also outranks the
///    sign-in state: exposure corrupts reads happening now, a dead sign-in
///    merely stops sync loudly.
/// 4. Sign-in required — the service has conclusively refused the stored
///    credential (measured semantics: `MAX_REJECTIONS` consecutive
///    `invalid_grant`s, nothing less). Only the *running* daemon asserts
///    this, so showing it never contradicts rule 2.
/// 5. Unsent changes — ordinary work in flight.
/// 6. Synced.
///
/// Wording rule for the stopped states: the files are *unreachable*, not
/// lost, and the text says so explicitly rather than leaving a scary blank.
/// The signed-out state follows the same rule — a signed-out client has
/// lost nothing either — and names the tool that actually works on this
/// deployment (`tools/pkce-enroll.py`; Conditional Access blocks the
/// daemon's own device-code flow). There is deliberately no sign-in button
/// anywhere: the surface cannot run a browser flow, and a button that
/// cannot do the thing it names is worse than a sentence that can be
/// followed.
pub fn present(state: Option<DaemonState>, credential: CredentialState) -> Presentation {
    let Some(state) = state else {
        return Presentation {
            icon: ICON_STOPPED,
            sni_status: "Active",
            headline: "State service not running".to_owned(),
            detail: "onedrive-hydration-dbus is not on the session bus, so the daemon's state \
                     is unknown. Files stay in OneDrive either way; nothing is lost."
                .to_owned(),
        };
    };
    if !state.daemon_running {
        let mut detail = "Cloud-only files cannot be opened until the daemon starts. Nothing is \
                          lost: every synced file is still in OneDrive."
            .to_owned();
        if state.exposures > 0 {
            // Held, last-seen knowledge — quoted as such, not shown as live.
            detail.push_str(&format!(
                " Before it stopped, {} exposed the sync folder.",
                count(state.exposures, "other mount", "other mounts")
            ));
        }
        return Presentation {
            icon: ICON_STOPPED,
            sni_status: "Active",
            headline: "Sync daemon not running".to_owned(),
            detail,
        };
    }
    if state.exposures > 0 {
        let headline = if state.exposures == 1 {
            "1 mount bypasses hydration".to_owned()
        } else {
            format!("{} mounts bypass hydration", state.exposures)
        };
        let mut detail = if state.exposures == 1 {
            "Another mount exposes the OneDrive files, and reads through it bypass hydration: \
             they can silently return empty placeholder content. Unmount the extra path to \
             close the bypass."
                .to_owned()
        } else {
            "Other mounts expose the OneDrive files, and reads through them bypass hydration: \
             they can silently return empty placeholder content. Unmount the extra paths to \
             close the bypass."
                .to_owned()
        };
        if state.unsent > 0 {
            detail.push_str(&format!(
                " {} still waiting to upload.",
                count(state.unsent, "change is", "changes are")
            ));
        }
        detail.push_str(store_caveat(credential));
        return Presentation {
            icon: ICON_EXPOSED,
            sni_status: "NeedsAttention",
            headline,
            detail,
        };
    }
    if credential == CredentialState::Rejected {
        let mut detail = "OneDrive no longer accepts this machine's saved sign-in — it was \
                          revoked, expired, or invalidated by a password change or policy. \
                          Nothing is lost: every synced file is still in OneDrive, but nothing \
                          syncs and cloud-only files cannot be opened until you sign in again. \
                          Sign in from a terminal with tools/pkce-enroll.py (Conditional Access \
                          blocks the built-in device-code sign-in here); the daemon adopts it \
                          and restarts by itself."
            .to_owned();
        if state.unsent > 0 {
            detail.push_str(&format!(
                " {} still waiting to upload.",
                count(state.unsent, "change is", "changes are")
            ));
        }
        return Presentation {
            icon: ICON_STOPPED,
            sni_status: "NeedsAttention",
            headline: "Sign-in required".to_owned(),
            detail,
        };
    }
    if state.unsent > 0 {
        return Presentation {
            icon: ICON_UNSENT,
            sni_status: "Active",
            headline: format!("{} to upload", count(state.unsent, "change", "changes")),
            detail: format!(
                "{} not reached OneDrive yet.{}{}",
                count(state.unsent, "local change has", "local changes have"),
                placeholders_line(state.excluded),
                store_caveat(credential)
            ),
        };
    }
    Presentation {
        icon: ICON_SYNCED,
        sni_status: "Active",
        headline: "Up to date".to_owned(),
        detail: format!(
            "All local changes are in OneDrive.{}{}",
            placeholders_line(state.excluded),
            store_caveat(credential)
        ),
    }
}

/// What a left click (and the menu's folder entry) does. The mount path is
/// installation-time knowledge — the tray is told with `--mount`, the same
/// way every other piece of this product receives its paths, rather than
/// guessing one. Without it there is simply no folder entry.
///
/// The action itself is injected so tests can record the open instead of
/// launching a file manager on whatever machine runs them; the binary passes
/// `xdg-open`.
pub struct Opener {
    mount: Option<PathBuf>,
    open: Box<dyn Fn(&Path) + Send + Sync>,
}

impl Opener {
    fn open_mount(&self) {
        if let Some(mount) = &self.mount {
            (self.open)(mount);
        }
    }
}

/// A pixmap as StatusNotifierItem carries them: width, height, ARGB32 bytes.
/// Always empty here — icons go by theme name — but the properties must
/// still answer with the right signature, because the measured host reads
/// them all.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    serde::Serialize,
    zvariant::Type,
    zvariant::Value,
    zvariant::OwnedValue,
)]
pub struct Pixmap {
    pub width: i32,
    pub height: i32,
    pub bytes: Vec<u8>,
}

/// The `ToolTip` property's `(sa(iiay)ss)` structure.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    serde::Serialize,
    zvariant::Type,
    zvariant::Value,
    zvariant::OwnedValue,
)]
pub struct ToolTip {
    pub icon_name: String,
    pub icon_pixmap: Vec<Pixmap>,
    pub title: String,
    pub text: String,
}

/// The object served at [`ITEM_PATH`].
pub struct StatusNotifierItem {
    presentation: Presentation,
    opener: Arc<Opener>,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl StatusNotifierItem {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        "onedrive-hydration"
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        "OneDrive Hydration"
    }

    /// `"NeedsAttention"` for the two states a person must act on — the
    /// exposure hazard and a sign-in the service has refused; see
    /// [`present`].
    #[zbus(property)]
    fn status(&self) -> &str {
        self.presentation.sni_status
    }

    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }

    /// Empty: icons come from the hicolor theme by name, not from a private
    /// directory.
    #[zbus(property)]
    fn icon_theme_path(&self) -> &str {
        ""
    }

    /// With no mount to open, a left click has no defined action, so let the
    /// host treat every click as "show the menu" instead of doing nothing.
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        self.opener.mount.is_none()
    }

    #[zbus(property)]
    fn menu(&self) -> ObjectPath<'_> {
        ObjectPath::from_static_str_unchecked(MENU_PATH)
    }

    #[zbus(property)]
    fn icon_name(&self) -> &str {
        self.presentation.icon
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<Pixmap> {
        Vec::new()
    }

    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Vec<Pixmap> {
        Vec::new()
    }

    /// The attention states keep their own icons — exposed for the exposure
    /// hazard, stopped for a refused sign-in — so this follows the item
    /// icon rather than pinning one hazard's artwork on every alarm. Hosts
    /// only consult it while `Status` is `"NeedsAttention"`.
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        self.presentation.icon
    }

    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Vec<Pixmap> {
        Vec::new()
    }

    #[zbus(property)]
    fn attention_movie_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: ICON_APP.to_owned(),
            icon_pixmap: Vec::new(),
            title: self.presentation.headline.clone(),
            text: self.presentation.detail.clone(),
        }
    }

    /// Left click: open the sync folder, when we know where it is.
    fn activate(&self, _x: i32, _y: i32) {
        self.opener.open_mount();
    }

    /// Middle click. No defined action.
    fn secondary_activate(&self, _x: i32, _y: i32) {}

    /// Only called by hosts that cannot render our `Menu` property
    /// themselves; Plasma never calls it. We cannot draw a menu without a
    /// toolkit, so there is nothing honest to do here.
    fn context_menu(&self, _x: i32, _y: i32) {}

    fn scroll(&self, _delta: i32, _orientation: &str) {}

    #[zbus(signal)]
    pub async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn new_attention_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn new_overlay_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn new_title(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Menu item ids. Stable whether or not the folder entry exists, so a host
/// that cached ids across a layout change could never activate the wrong
/// entry.
const MENU_ROOT: i32 = 0;
const MENU_STATUS: i32 = 1;
const MENU_SEPARATOR_A: i32 = 2;
const MENU_FOLDER: i32 = 3;
const MENU_SEPARATOR_B: i32 = 4;
const MENU_QUIT: i32 = 5;

/// Events the interfaces push at the run loop.
enum TrayEvent {
    /// A `StateChanged` signal arrived from the state service.
    State(DaemonState),
    /// A `CredentialStateChanged` signal arrived from the state service.
    Credential(CredentialState),
    /// The state service left the bus.
    ServiceGone,
    /// The state service (re)appeared on the bus; re-read its properties.
    ServiceReturned,
    /// A new watcher owns [`WATCHER_NAME`]; register with it.
    WatcherReturned,
    /// The menu's Quit entry was clicked.
    Quit,
    /// A signal stream ended, which only happens when our own bus connection
    /// died. There is nothing left to serve.
    BusLost,
}

/// The object served at [`MENU_PATH`], speaking `com.canonical.dbusmenu`.
///
/// The layout is fixed at startup — status line, optional folder entry,
/// quit — and only the status line's label ever changes, announced through
/// `ItemsPropertiesUpdated`. `Evict` is deliberately not here; see the
/// module docs.
pub struct DBusMenu {
    headline: String,
    opener: Arc<Opener>,
    events: mpsc::Sender<TrayEvent>,
    revision: u32,
}

impl DBusMenu {
    /// The ids present in this menu, in display order.
    fn item_ids(&self) -> Vec<i32> {
        if self.opener.mount.is_some() {
            vec![
                MENU_STATUS,
                MENU_SEPARATOR_A,
                MENU_FOLDER,
                MENU_SEPARATOR_B,
                MENU_QUIT,
            ]
        } else {
            vec![MENU_STATUS, MENU_SEPARATOR_A, MENU_QUIT]
        }
    }

    /// The dbusmenu properties of one item. Defaults (`enabled`, `visible`,
    /// `type=standard`) are omitted, matching what measured menus send.
    fn item_properties(&self, id: i32) -> Option<Vec<(&'static str, Value<'static>)>> {
        match id {
            MENU_ROOT => Some(vec![("children-display", Value::from("submenu"))]),
            // The menu leads with the state, as the donor client's did. Not
            // activatable — it is a statement, not an action.
            MENU_STATUS => Some(vec![
                ("label", Value::from(self.headline.clone())),
                ("enabled", Value::from(false)),
            ]),
            MENU_SEPARATOR_A => Some(vec![("type", Value::from("separator"))]),
            MENU_FOLDER if self.opener.mount.is_some() => Some(vec![
                ("label", Value::from("Open OneDrive Folder")),
                ("icon-name", Value::from("folder")),
            ]),
            MENU_SEPARATOR_B if self.opener.mount.is_some() => {
                Some(vec![("type", Value::from("separator"))])
            }
            MENU_QUIT => Some(vec![
                ("label", Value::from("Quit")),
                ("icon-name", Value::from("application-exit")),
            ]),
            _ => None,
        }
    }
}

/// One `(ia{sv}av)` node of a `GetLayout` reply.
#[derive(Debug, serde::Serialize, zvariant::Type)]
pub struct MenuLayout(
    pub i32,
    pub HashMap<String, OwnedValue>,
    pub Vec<OwnedValue>,
);

fn owned_props(
    props: Vec<(&'static str, Value<'static>)>,
) -> zbus::fdo::Result<HashMap<String, OwnedValue>> {
    props
        .into_iter()
        .map(|(k, v)| {
            v.try_to_owned()
                .map(|v| (k.to_owned(), v))
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
        })
        .collect()
}

/// Wrap a childless node as the variant element `GetLayout`'s `av` carries.
fn leaf_value(
    id: i32,
    props: Vec<(&'static str, Value<'static>)>,
) -> zbus::fdo::Result<OwnedValue> {
    let dict_source: HashMap<String, Value> =
        props.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    let structure = zvariant::StructureBuilder::new()
        .add_field(id)
        .append_field(Value::Dict(zvariant::Dict::from(dict_source)))
        .append_field(Value::Array(zvariant::Array::from(Vec::<Value>::new())))
        .build()
        .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
    Value::from(structure)
        .try_to_owned()
        .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl DBusMenu {
    /// 3 is what the measured host itself serves; there is nothing from
    /// later revisions this menu needs.
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    /// The layout is one level deep, so recursion handling reduces to
    /// "children or not": depth 0 omits them, anything else includes them.
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, MenuLayout)> {
        let props = self.item_properties(parent_id).ok_or_else(|| {
            zbus::fdo::Error::InvalidArgs(format!("no menu item has id {parent_id}"))
        })?;
        let mut children = Vec::new();
        if parent_id == MENU_ROOT && recursion_depth != 0 {
            for id in self.item_ids() {
                let props = self
                    .item_properties(id)
                    .expect("item_ids only lists ids item_properties knows");
                children.push(leaf_value(id, props)?);
            }
        }
        Ok((
            self.revision,
            MenuLayout(parent_id, owned_props(props)?, children),
        ))
    }

    /// An empty `ids` means every item, per the dbusmenu specification.
    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> zbus::fdo::Result<Vec<(i32, HashMap<String, OwnedValue>)>> {
        let ids = if ids.is_empty() {
            let mut all = vec![MENU_ROOT];
            all.extend(self.item_ids());
            all
        } else {
            ids
        };
        ids.into_iter()
            .filter_map(|id| self.item_properties(id).map(|props| (id, props)))
            .map(|(id, props)| Ok((id, owned_props(props)?)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        self.item_properties(id)
            .and_then(|props| props.into_iter().find(|(k, _)| *k == name))
            .map(|(_, v)| {
                v.try_to_owned()
                    .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
            })
            .ok_or_else(|| {
                zbus::fdo::Error::InvalidArgs(format!("menu item {id} has no property {name}"))
            })?
    }

    /// Clicks. Hovers and open/close notifications arrive here too and mean
    /// nothing to a menu without submenus, so they are ignored rather than
    /// answered with errors the host would log.
    fn event(&self, id: i32, event_id: String, _data: Value<'_>, _timestamp: u32) {
        if event_id != "clicked" {
            return;
        }
        match id {
            MENU_FOLDER => self.opener.open_mount(),
            // The receiver only disappears while the run loop is already
            // returning, so a failed send needs no second announcement.
            MENU_QUIT => drop(self.events.send(TrayEvent::Quit)),
            _ => {}
        }
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        let mut unknown = Vec::new();
        for (id, event_id, _data, _timestamp) in events {
            if self.item_properties(id).is_none() {
                unknown.push(id);
                continue;
            }
            self.event(id, event_id, Value::from(0i32), 0);
        }
        unknown
    }

    /// The layout never changes shape and the label is pushed eagerly, so a
    /// host about to show the menu never needs a refresh first.
    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(signal)]
    pub async fn items_properties_updated(
        emitter: &SignalEmitter<'_>,
        updated_props: Vec<(i32, HashMap<String, OwnedValue>)>,
        removed_props: Vec<(i32, Vec<String>)>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

/// Push a new presentation into the served objects: set the changed SNI
/// properties and emit the `New*` signals hosts actually listen for, then
/// update the menu's status label via `ItemsPropertiesUpdated`. Emits
/// nothing when nothing changed, so a replayed state after a reconnect is
/// silent, mirroring the dedup rule of the surface underneath.
fn apply_presentation(
    sni: &zbus::blocking::object_server::InterfaceRef<StatusNotifierItem>,
    menu: &zbus::blocking::object_server::InterfaceRef<DBusMenu>,
    presentation: &Presentation,
) -> zbus::Result<()> {
    let mut item = sni.get_mut();
    let previous = std::mem::replace(&mut item.presentation, presentation.clone());
    let emitter = sni.signal_emitter();
    zbus::block_on(async {
        if previous.icon != presentation.icon {
            StatusNotifierItem::new_icon(emitter).await?;
            // The attention icon follows the item icon (see
            // `attention_icon_name`), so it changed with it.
            StatusNotifierItem::new_attention_icon(emitter).await?;
        }
        if previous.sni_status != presentation.sni_status {
            StatusNotifierItem::new_status(emitter, presentation.sni_status).await?;
        }
        if previous.headline != presentation.headline || previous.detail != presentation.detail {
            StatusNotifierItem::new_tool_tip(emitter).await?;
        }
        Ok::<(), zbus::Error>(())
    })?;
    drop(item);

    let mut menu_state = menu.get_mut();
    if menu_state.headline != presentation.headline {
        menu_state.headline = presentation.headline.clone();
        menu_state.revision += 1;
        let updated = vec![(
            MENU_STATUS,
            owned_props(vec![("label", Value::from(presentation.headline.clone()))])
                .map_err(|e| zbus::Error::Failure(e.to_string()))?,
        )];
        let emitter = menu.signal_emitter();
        zbus::block_on(DBusMenu::items_properties_updated(
            emitter,
            updated,
            Vec::new(),
        ))?;
    }
    Ok(())
}

/// Ask the state service for its current properties, or `None` when it is
/// not on the bus. One cold read — at startup and when the service returns —
/// is the documented complement to the signal, not polling: a service that
/// starts while its daemon is down publishes nothing, and only a read can
/// distinguish that from the service being absent entirely.
fn read_service_state(connection: &zbus::blocking::Connection) -> Option<DaemonState> {
    let proxy: zbus::blocking::Proxy<'_> = zbus::blocking::proxy::Builder::new(connection)
        .destination(BUS_NAME)
        .ok()?
        .path(OBJECT_PATH)
        .ok()?
        .interface(INTERFACE)
        .ok()?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .ok()?;
    Some(DaemonState {
        daemon_running: proxy.get_property::<bool>("DaemonRunning").ok()?,
        unsent: proxy.get_property::<u64>("Unsent").ok()?,
        excluded: proxy.get_property::<u64>("Excluded").ok()?,
        exposures: proxy.get_property::<u64>("Exposures").ok()?,
    })
}

/// The credential half of the cold read. Separate from
/// [`read_service_state`] and infallible on purpose: a state service built
/// before `CredentialState` existed still serves the four properties above,
/// and this tray must keep working against it — a missing property is
/// exactly "nobody has asserted anything", which already has a word.
fn read_credential_state(connection: &zbus::blocking::Connection) -> CredentialState {
    let proxy: Option<zbus::blocking::Proxy<'_>> = zbus::blocking::proxy::Builder::new(connection)
        .destination(BUS_NAME)
        .ok()
        .and_then(|b| b.path(OBJECT_PATH).ok())
        .and_then(|b| b.interface(INTERFACE).ok())
        .map(|b| b.cache_properties(zbus::proxy::CacheProperties::No))
        .and_then(|b| b.build().ok());
    proxy
        .and_then(|p| p.get_property::<String>("CredentialState").ok())
        .map(|value| CredentialState::from_wire(&value))
        .unwrap_or(CredentialState::Unknown)
}

/// Hand the watcher our unique name; it looks for the item at [`ITEM_PATH`].
/// Registering by unique name (rather than claiming a well-known
/// `org.kde.StatusNotifierItem-<pid>-1`) is the form an existing item on
/// this machine's session uses, so the host demonstrably supports it.
fn register_with_watcher(connection: &zbus::blocking::Connection) -> io::Result<()> {
    let unique = connection
        .unique_name()
        .ok_or_else(|| io::Error::other("the bus connection has no unique name"))?
        .to_string();
    let proxy = zbus::blocking::Proxy::new(connection, WATCHER_NAME, WATCHER_PATH, WATCHER_NAME)
        .map_err(io::Error::other)?;
    proxy
        .call::<_, _, ()>("RegisterStatusNotifierItem", &(unique.as_str(),))
        .map_err(|e| io::Error::other(format!("could not register with {WATCHER_NAME}: {e}")))
}

/// What the binary passes to [`run`].
pub struct TrayOptions {
    /// The sync root, for "Open OneDrive Folder" and left click. `None`
    /// drops the entry instead of guessing a path.
    pub mount: Option<PathBuf>,
    /// How to open it; the binary passes `xdg-open`.
    pub open: Box<dyn Fn(&Path) + Send + Sync>,
}

/// Serve the tray until Quit is clicked. Returns an error naming the missing
/// piece when this desktop cannot show StatusNotifier items at all, and when
/// the session bus connection is lost.
pub fn run(connection: zbus::blocking::Connection, options: TrayOptions) -> io::Result<()> {
    let fdo = zbus::blocking::fdo::DBusProxy::new(&connection).map_err(io::Error::other)?;
    let watcher_busname = BusName::try_from(WATCHER_NAME).map_err(io::Error::other)?;
    if !fdo
        .name_has_owner(watcher_busname)
        .map_err(io::Error::other)?
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no {WATCHER_NAME} on the session bus — this desktop has no StatusNotifier \
                 tray to register with, so the icon cannot be shown"
            ),
        ));
    }

    let (events, event_queue) = mpsc::channel();
    let opener = Arc::new(Opener {
        mount: options.mount,
        open: options.open,
    });
    let object_server = connection.object_server();
    object_server
        .at(
            ITEM_PATH,
            StatusNotifierItem {
                presentation: present(None, CredentialState::Unknown),
                opener: Arc::clone(&opener),
            },
        )
        .map_err(io::Error::other)?;
    object_server
        .at(
            MENU_PATH,
            DBusMenu {
                headline: present(None, CredentialState::Unknown).headline,
                opener,
                events: events.clone(),
                revision: 1,
            },
        )
        .map_err(io::Error::other)?;
    let sni = object_server
        .interface::<_, StatusNotifierItem>(ITEM_PATH)
        .map_err(io::Error::other)?;
    let menu = object_server
        .interface::<_, DBusMenu>(MENU_PATH)
        .map_err(io::Error::other)?;

    // Subscriptions are created here, before the initial read, so a state
    // change in the gap lands in the queue instead of being missed; the
    // iterators are only *drained* on their own threads. Each stream ends
    // when the connection dies, and reports that instead of going quiet.
    let state_proxy: zbus::blocking::Proxy<'static> =
        zbus::blocking::proxy::Builder::new(&connection)
            .destination(BUS_NAME)
            .and_then(|b| b.path(OBJECT_PATH))
            .and_then(|b| b.interface(INTERFACE))
            .map(|b| b.cache_properties(zbus::proxy::CacheProperties::No))
            .and_then(|b| b.build())
            .map_err(io::Error::other)?;
    let state_signals = state_proxy
        .receive_signal("StateChanged")
        .map_err(io::Error::other)?;
    let credential_signals = state_proxy
        .receive_signal("CredentialStateChanged")
        .map_err(io::Error::other)?;
    let service_owner_changes = fdo
        .receive_name_owner_changed_with_args(&[(0, BUS_NAME)])
        .map_err(io::Error::other)?;
    let watcher_owner_changes = fdo
        .receive_name_owner_changed_with_args(&[(0, WATCHER_NAME)])
        .map_err(io::Error::other)?;

    let state_events = events.clone();
    thread::spawn(move || {
        for message in state_signals {
            if let Ok(state) = message.body().deserialize::<(bool, u64, u64, u64)>() {
                let (daemon_running, unsent, excluded, exposures) = state;
                if state_events
                    .send(TrayEvent::State(DaemonState {
                        daemon_running,
                        unsent,
                        excluded,
                        exposures,
                    }))
                    .is_err()
                {
                    return;
                }
            }
        }
        drop(state_events.send(TrayEvent::BusLost));
    });
    let credential_events = events.clone();
    thread::spawn(move || {
        for message in credential_signals {
            if let Ok((value,)) = message.body().deserialize::<(String,)>() {
                if credential_events
                    .send(TrayEvent::Credential(CredentialState::from_wire(&value)))
                    .is_err()
                {
                    return;
                }
            }
        }
        drop(credential_events.send(TrayEvent::BusLost));
    });
    let service_events = events.clone();
    thread::spawn(move || {
        for change in service_owner_changes {
            let Ok(args) = change.args() else { continue };
            let event = if args.new_owner().is_some() {
                TrayEvent::ServiceReturned
            } else {
                TrayEvent::ServiceGone
            };
            if service_events.send(event).is_err() {
                return;
            }
        }
        drop(service_events.send(TrayEvent::BusLost));
    });
    let watcher_events = events.clone();
    thread::spawn(move || {
        for change in watcher_owner_changes {
            let Ok(args) = change.args() else { continue };
            if args.new_owner().is_some() {
                if watcher_events.send(TrayEvent::WatcherReturned).is_err() {
                    return;
                }
            } else {
                eprintln!(
                    "onedrive-hydration-tray: {WATCHER_NAME} left the bus; the icon is not \
                     shown until a watcher returns"
                );
            }
        }
        drop(watcher_events.send(TrayEvent::BusLost));
    });

    // What the loop below renders: the last state and credential the
    // service told us, updated by signals and re-read when the service
    // returns to the bus. Two variables rather than one struct because they
    // arrive on two signals and go stale together only when the service
    // itself goes away.
    let mut daemon_state = read_service_state(&connection);
    let mut credential = read_credential_state(&connection);
    apply_presentation(&sni, &menu, &present(daemon_state, credential))
        .map_err(io::Error::other)?;

    // Register only now, with the objects served and current: a watcher that
    // looks the moment we register must find the real item, not a half-built
    // one.
    register_with_watcher(&connection)?;
    match zbus::blocking::Proxy::new(&connection, WATCHER_NAME, WATCHER_PATH, WATCHER_NAME)
        .and_then(|watcher| watcher.get_property::<bool>("IsStatusNotifierHostRegistered"))
    {
        Ok(true) => {}
        Ok(false) => eprintln!(
            "onedrive-hydration-tray: a watcher is present but no host is registered with it; \
             the icon stays registered and will appear when a host arrives"
        ),
        Err(e) => eprintln!(
            "onedrive-hydration-tray: could not ask the watcher whether a host is registered: {e}"
        ),
    }

    loop {
        match event_queue.recv() {
            Ok(TrayEvent::State(state)) => {
                daemon_state = Some(state);
                apply_presentation(&sni, &menu, &present(daemon_state, credential))
                    .map_err(io::Error::other)?;
            }
            Ok(TrayEvent::Credential(state)) => {
                credential = state;
                apply_presentation(&sni, &menu, &present(daemon_state, credential))
                    .map_err(io::Error::other)?;
            }
            Ok(TrayEvent::ServiceGone) => {
                // Nothing the service asserted survives it leaving the bus.
                daemon_state = None;
                credential = CredentialState::Unknown;
                apply_presentation(&sni, &menu, &present(daemon_state, credential))
                    .map_err(io::Error::other)?;
            }
            Ok(TrayEvent::ServiceReturned) => {
                daemon_state = read_service_state(&connection);
                credential = read_credential_state(&connection);
                apply_presentation(&sni, &menu, &present(daemon_state, credential))
                    .map_err(io::Error::other)?;
            }
            Ok(TrayEvent::WatcherReturned) => {
                // A restarted watcher (kded6) lost every registration it
                // held. A restarted host (plasmashell) alone keeps the
                // watcher's list and needs nothing from us; re-registering
                // is harmless in that case and required in the first, so
                // always re-register.
                if let Err(e) = register_with_watcher(&connection) {
                    eprintln!(
                        "onedrive-hydration-tray: re-registration failed, waiting for the \
                         watcher to return again: {e}"
                    );
                }
            }
            Ok(TrayEvent::Quit) => return Ok(()),
            Ok(TrayEvent::BusLost) | Err(mpsc::RecvError) => {
                return Err(io::Error::other(
                    "the session bus connection was lost; nothing can be shown without it",
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(daemon_running: bool, unsent: u64, excluded: u64, exposures: u64) -> DaemonState {
        DaemonState {
            daemon_running,
            unsent,
            excluded,
            exposures,
        }
    }

    /// Most states do not depend on the credential; spell that out once.
    fn shown(state: Option<DaemonState>) -> Presentation {
        present(state, CredentialState::Unknown)
    }

    #[test]
    fn a_missing_service_and_a_stopped_daemon_are_different_states() {
        let service_gone = shown(None);
        let daemon_stopped = shown(Some(state(false, 0, 0, 0)));
        assert_eq!(service_gone.icon, ICON_STOPPED);
        assert_eq!(daemon_stopped.icon, ICON_STOPPED);
        assert_ne!(service_gone.headline, daemon_stopped.headline);
        assert!(service_gone.detail.contains("onedrive-hydration-dbus"));
        assert!(daemon_stopped.detail.contains("daemon"));
    }

    #[test]
    fn stopped_states_say_files_are_unreachable_not_lost() {
        for p in [shown(None), shown(Some(state(false, 3, 10, 0)))] {
            assert!(
                p.detail.contains("nothing is lost") || p.detail.contains("Nothing is lost"),
                "stopped detail must say nothing is lost: {:?}",
                p.detail
            );
        }
    }

    #[test]
    fn a_stopped_daemon_quotes_held_exposures_as_past_not_current() {
        let p = shown(Some(state(false, 0, 0, 2)));
        assert_eq!(p.icon, ICON_STOPPED);
        assert_eq!(p.sni_status, "Active");
        assert!(p.detail.contains("Before it stopped, 2 other mounts"));
        // But a clean stop mentions no exposures at all.
        assert!(!shown(Some(state(false, 0, 0, 0)))
            .detail
            .contains("Before it stopped"));
    }

    #[test]
    fn exposures_outrank_unsent_and_demand_attention() {
        let p = shown(Some(state(true, 5, 100, 1)));
        assert_eq!(p.icon, ICON_EXPOSED);
        assert_eq!(p.sni_status, "NeedsAttention");
        assert_eq!(p.headline, "1 mount bypasses hydration");
        assert!(p.detail.contains("bypass hydration"));
        // The unsent work is still reported, just not as the headline.
        assert!(p.detail.contains("5 changes are still waiting to upload"));

        let plural = shown(Some(state(true, 0, 0, 3)));
        assert_eq!(plural.headline, "3 mounts bypass hydration");
        assert!(!plural.detail.contains("waiting to upload"));
    }

    #[test]
    fn attention_is_for_exposures_and_a_refused_sign_in_only() {
        for p in [
            shown(None),
            shown(Some(state(false, 0, 0, 1))),
            shown(Some(state(true, 4, 2, 0))),
            shown(Some(state(true, 0, 2, 0))),
            present(Some(state(true, 0, 2, 0)), CredentialState::Unsaved),
        ] {
            assert_eq!(p.sni_status, "Active", "{:?}", p.headline);
        }
        for p in [
            present(Some(state(true, 0, 0, 1)), CredentialState::Healthy),
            present(Some(state(true, 0, 0, 0)), CredentialState::Rejected),
        ] {
            assert_eq!(p.sni_status, "NeedsAttention", "{:?}", p.headline);
        }
    }

    #[test]
    fn unsent_counts_read_naturally_in_both_numbers() {
        let one = shown(Some(state(true, 1, 0, 0)));
        assert_eq!(one.icon, ICON_UNSENT);
        assert_eq!(one.headline, "1 change to upload");
        assert!(one.detail.contains("1 local change has not reached"));

        let many = shown(Some(state(true, 12, 1, 0)));
        assert_eq!(many.headline, "12 changes to upload");
        assert!(many.detail.contains("12 local changes have not reached"));
        assert!(many.detail.contains("1 file is a cloud-only placeholder."));
    }

    #[test]
    fn synced_reports_up_to_date_and_the_placeholder_population() {
        let p = shown(Some(state(true, 0, 146820, 0)));
        assert_eq!(p.icon, ICON_SYNCED);
        assert_eq!(p.headline, "Up to date");
        assert!(p
            .detail
            .contains("146820 files are cloud-only placeholders."));
        // A drive with nothing dehydrated gets no placeholder line.
        assert!(!shown(Some(state(true, 0, 0, 0)))
            .detail
            .contains("placeholder"));
    }

    #[test]
    fn sign_in_required_says_nothing_is_lost_and_names_the_tool_that_works() {
        let p = present(Some(state(true, 0, 7, 0)), CredentialState::Rejected);
        assert_eq!(p.headline, "Sign-in required");
        assert_eq!(p.icon, ICON_STOPPED);
        assert_eq!(p.sni_status, "NeedsAttention");
        // The register the stopped states established: unreachable, not lost.
        assert!(p.detail.contains("Nothing is lost"), "{}", p.detail);
        // The instruction must be one that works on this deployment —
        // Conditional Access blocks the daemon's device-code flow, so the
        // browser enrollment tool is named, and the wording says why.
        assert!(p.detail.contains("tools/pkce-enroll.py"), "{}", p.detail);
        assert!(p.detail.contains("Conditional Access"), "{}", p.detail);
        // And what happens next, because the daemon really does restart
        // itself once the enrollment file appears.
        assert!(p.detail.contains("restarts by itself"), "{}", p.detail);

        // Unsent work is still reported, the way the exposure arm does it.
        let busy = present(Some(state(true, 4, 0, 0)), CredentialState::Rejected);
        assert!(
            busy.detail
                .contains("4 changes are still waiting to upload"),
            "{}",
            busy.detail
        );
    }

    #[test]
    fn exposures_outrank_a_refused_sign_in() {
        // Exposure corrupts reads happening now; a dead sign-in stops sync
        // loudly. The one a person must see first is the quiet one.
        let p = present(Some(state(true, 0, 0, 1)), CredentialState::Rejected);
        assert_eq!(p.icon, ICON_EXPOSED);
        assert_eq!(p.headline, "1 mount bypasses hydration");
    }

    #[test]
    fn a_stopped_daemon_never_renders_a_held_sign_in_state() {
        // The service resets its credential property to "unknown" when the
        // daemon dies, but this mapping must not depend on that: a stopped
        // daemon cannot tell a missing credential from a locked keyring,
        // and a re-enroll instruction over a locked keyring is the exact
        // wrong message this surface exists to avoid.
        let p = present(Some(state(false, 0, 0, 0)), CredentialState::Rejected);
        assert_eq!(p.headline, "Sync daemon not running");
        assert!(!p.detail.contains("pkce-enroll"), "{}", p.detail);
        let gone = present(None, CredentialState::Rejected);
        assert_eq!(gone.headline, "State service not running");
    }

    #[test]
    fn an_unsaved_rotation_is_a_caveat_on_every_running_state_not_a_state() {
        for base in [
            state(true, 0, 0, 0), // synced
            state(true, 2, 0, 0), // unsent
            state(true, 0, 0, 1), // exposed
        ] {
            let plain = present(Some(base), CredentialState::Healthy);
            let unsaved = present(Some(base), CredentialState::Unsaved);
            assert_eq!(
                plain.headline, unsaved.headline,
                "the headline stays about the work"
            );
            assert!(
                unsaved
                    .detail
                    .contains("could not be saved to Linux Secret Service"),
                "{}",
                unsaved.detail
            );
            assert!(
                unsaved.detail.contains("unlock the keyring"),
                "{}",
                unsaved.detail
            );
        }
        // Not on the stopped state (held knowledge), and not on rejection
        // (the credential is dead; whether it was being saved is history).
        for p in [
            present(Some(state(false, 0, 0, 0)), CredentialState::Unsaved),
            present(Some(state(true, 0, 0, 0)), CredentialState::Rejected),
        ] {
            assert!(!p.detail.contains("could not be saved"), "{}", p.detail);
        }
    }

    #[test]
    fn an_unknown_credential_presents_exactly_like_a_healthy_one() {
        // "Unknown" is the older-daemon and just-restarted case; nagging on
        // it would nag on every deployment that has not upgraded yet.
        for base in [
            None,
            Some(state(false, 1, 2, 3)),
            Some(state(true, 0, 0, 0)),
            Some(state(true, 5, 2, 0)),
            Some(state(true, 0, 0, 2)),
        ] {
            assert_eq!(
                present(base, CredentialState::Unknown),
                present(base, CredentialState::Healthy)
            );
        }
    }

    fn menu(mount: Option<PathBuf>) -> DBusMenu {
        let (events, _queue) = mpsc::channel();
        DBusMenu {
            headline: "Up to date".to_owned(),
            opener: Arc::new(Opener {
                mount,
                open: Box::new(|_| {}),
            }),
            events,
            revision: 1,
        }
    }

    #[test]
    fn the_menu_offers_the_folder_only_when_it_knows_the_mount() {
        let with = menu(Some(PathBuf::from("/home/user/OneDrive")));
        assert_eq!(
            with.item_ids(),
            [
                MENU_STATUS,
                MENU_SEPARATOR_A,
                MENU_FOLDER,
                MENU_SEPARATOR_B,
                MENU_QUIT
            ]
        );
        let without = menu(None);
        assert_eq!(
            without.item_ids(),
            [MENU_STATUS, MENU_SEPARATOR_A, MENU_QUIT]
        );
        assert!(without.item_properties(MENU_FOLDER).is_none());
    }

    #[test]
    fn the_status_entry_is_a_statement_not_an_action() {
        let m = menu(None);
        let props = m.item_properties(MENU_STATUS).unwrap();
        assert!(props
            .iter()
            .any(|(k, v)| *k == "label" && *v == Value::from("Up to date")));
        assert!(props
            .iter()
            .any(|(k, v)| *k == "enabled" && *v == Value::from(false)));
    }

    #[test]
    fn get_layout_serves_the_whole_tree_from_the_root() {
        let m = menu(Some(PathBuf::from("/mnt")));
        let (revision, MenuLayout(id, props, children)) = m.get_layout(0, -1, Vec::new()).unwrap();
        assert_eq!(revision, 1);
        assert_eq!(id, MENU_ROOT);
        assert!(props.contains_key("children-display"));
        assert_eq!(children.len(), 5);

        // Depth 0 keeps the children out, per the dbusmenu contract.
        let (_, MenuLayout(_, _, children)) = m.get_layout(0, 0, Vec::new()).unwrap();
        assert!(children.is_empty());

        // A leaf answers alone; an unknown id is an error, not a shrug.
        let (_, MenuLayout(id, props, children)) = m.get_layout(MENU_QUIT, -1, Vec::new()).unwrap();
        assert_eq!(id, MENU_QUIT);
        assert!(props.contains_key("label"));
        assert!(children.is_empty());
        assert!(m.get_layout(99, -1, Vec::new()).is_err());
    }

    #[test]
    fn quit_clicks_reach_the_run_loop_and_hovers_do_not() {
        let (events, queue) = mpsc::channel();
        let m = DBusMenu {
            headline: String::new(),
            opener: Arc::new(Opener {
                mount: None,
                open: Box::new(|_| {}),
            }),
            events,
            revision: 1,
        };
        m.event(MENU_QUIT, "hovered".to_owned(), Value::from(0i32), 0);
        m.event(MENU_STATUS, "clicked".to_owned(), Value::from(0i32), 0);
        assert!(queue.try_recv().is_err());
        m.event(MENU_QUIT, "clicked".to_owned(), Value::from(0i32), 0);
        assert!(matches!(queue.try_recv(), Ok(TrayEvent::Quit)));
    }

    #[test]
    fn folder_clicks_open_the_mount_and_nothing_else() {
        let opened = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&opened);
        let (events, _queue) = mpsc::channel();
        let m = DBusMenu {
            headline: String::new(),
            opener: Arc::new(Opener {
                mount: Some(PathBuf::from("/home/user/OneDrive")),
                open: Box::new(move |p| seen.lock().unwrap().push(p.to_owned())),
            }),
            events,
            revision: 1,
        };
        m.event(MENU_FOLDER, "clicked".to_owned(), Value::from(0i32), 0);
        m.event(MENU_FOLDER, "opened".to_owned(), Value::from(0i32), 0);
        assert_eq!(
            *opened.lock().unwrap(),
            [PathBuf::from("/home/user/OneDrive")]
        );
    }
}
