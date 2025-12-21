// SPDX-License-Identifier: MPL-2.0

use cosmic::cctk::cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1;
use cosmic::cctk::wayland_client::Proxy;
use cosmic::cctk::wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1;
use cosmic::cctk::workspace::Workspace;
use cosmic::cctk::{
    self,
    sctk::{
        self,
        output::{OutputHandler, OutputState},
        registry::{ProvidesRegistryState, RegistryState},
    },
    toplevel_info::{ToplevelInfo, ToplevelInfoHandler, ToplevelInfoState},
    wayland_client::{
        globals::registry_queue_init, protocol::wl_output::WlOutput,
        Connection,
        QueueHandle,
    },
    workspace::{WorkspaceHandler, WorkspaceState},
};
use cosmic::iced;
use futures_channel::mpsc;
use futures_util::StreamExt;
use std::{collections::HashMap, thread};
use wayland_protocols::ext::workspace::v1::client::ext_workspace_handle_v1;
use wayland_protocols::ext::workspace::v1::client::ext_workspace_handle_v1::ExtWorkspaceHandleV1;

#[derive(Clone, Debug)]
pub enum WaylandEvent {
    WorkspacesChanged(Vec<AppWorkspace>),
    ToplevelsUpdated(
        HashMap<ExtWorkspaceHandleV1, HashMap<ExtForeignToplevelHandleV1, AppToplevel>>,
    ),
}

impl AppWorkspace {
    pub fn new(info: &Workspace) -> Option<AppWorkspace> {
        let handle = info.handle.clone();
        let name = info.name.clone();
        let is_active = info.state.contains(ext_workspace_handle_v1::State::Active);
        let x = info.coordinates.get(0).unwrap_or(&0);
        let y = info.coordinates.get(1).unwrap_or(&0);
        let coordinates = (*x as i32, *y as i32);
        Some(AppWorkspace {
            handle,
            name,
            is_active,
            coordinates,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppWorkspace {
    pub handle: ExtWorkspaceHandleV1,
    pub name: String,
    pub is_active: bool,
    pub coordinates: (i32, i32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppToplevel {
    pub handle: ExtForeignToplevelHandleV1,
    pub app_id: String,
    pub is_active: bool,
    pub ws_handle: ExtWorkspaceHandleV1,
    pub coordinates: (i32, i32)
}

impl AppToplevel {
    pub fn new(
        info: &ToplevelInfo,
        workspace: &AppWorkspace,
        wl_output: Option<&WlOutput>,
    ) -> Self {
        let handle = info.foreign_toplevel.clone();
        let ws_handle = workspace.handle.clone();
        let app_id = info.app_id.clone();
        let coordinates = if let Some(wl_output) = wl_output {
            let geometry = info.geometry.get(wl_output);
            if let Some(geometry) = geometry {
                (geometry.x, geometry.y)
            } else {
                (0, 0)
            }
        } else {
            (0, 0)
        };
        let is_active = info
            .state
            .contains(&zcosmic_toplevel_handle_v1::State::Activated);
        AppToplevel {
            handle,
            app_id,
            ws_handle,
            is_active,
            coordinates,
        }
    }
}

/// Creates an iced Subscription that streams Wayland workspace and toplevel events.
///
/// This subscription:
/// - Connects to the Wayland compositor via the WAYLAND_DISPLAY environment variable
/// - Sets up a background thread that listens for workspace and window events
/// - Returns a stream of WorkspaceEvent messages that can be handled by the iced application
///
/// The subscription uses a unique ID "workspace-sub" to ensure it's only created once,
/// even if the view function is called multiple times during rendering.
pub fn workspace_subscription() -> iced::Subscription<WaylandEvent> {
    iced::Subscription::run_with_id(
        "workspace-sub",
        futures_util::stream::once(async {
            match Connection::connect_to_env() {
                Ok(conn) => start(conn).await,
                Err(_) => mpsc::channel(1).1,
            }
        })
        .flatten(),
    )
}

/// AppData holds the state needed to handle Wayland protocol events.
///
/// This struct implements various "Handler" traits (WorkspaceHandler, ToplevelInfoHandler, etc.)
/// which define callbacks that get invoked when Wayland events occur.
///
/// The Wayland client-server model works through an event loop:
/// 1. The compositor (server) sends events about workspaces, windows, outputs, etc.
/// 2. These events are dispatched to handler methods defined in the trait implementations
/// 3. The handlers process events and send them to the iced application via the mpsc channel
pub struct AppData {
    // Wayland protocol state managers - these track global compositor state
    registry_state: RegistryState, // Tracks available Wayland global objects
    output_state: OutputState,     // Tracks display/monitor information
    workspace_state: WorkspaceState, // Tracks workspace (virtual desktop) state
    toplevel_info_state: ToplevelInfoState, // Tracks window/toplevel information
    //seat_state: SeatState,                   // Tracks input devices (keyboard, mouse)

    // Communication channel to send events to the iced application
    sender: mpsc::Sender<WaylandEvent>,

    // Mirrored app state
    workspaces: HashMap<ExtWorkspaceHandleV1, AppWorkspace>,
    toplevels: HashMap<ExtForeignToplevelHandleV1, AppToplevel>,
    workspace_toplevels:
        HashMap<ExtWorkspaceHandleV1, HashMap<ExtForeignToplevelHandleV1, AppToplevel>>,

    // Output (monitor) filtering - which display this applet is running on
    configured_output: String, // Name from COSMIC_PANEL_OUTPUT env var
    expected_output: Option<WlOutput>, // Resolved Wayland output object
}

impl AppData {
    fn get_workspace_from_handle(&self, handle: &ExtWorkspaceHandleV1) -> Option<AppWorkspace> {
        if let Some(ws_info) = self.workspace_state.workspace_info(handle) {
            AppWorkspace::new(ws_info)
        } else {
            log::debug!("workspace_handle_id={} info not found", handle.id());
            None
        }
    }

    fn get_toplevel_from_handle(&self, handle: &ExtForeignToplevelHandleV1) -> Option<AppToplevel> {
        let tl_info = self.toplevel_info_state.info(handle);
        if tl_info.is_none() {
            log::debug!("toplevel_handle_id={} info not found", handle.id());
            return None;
        }
        let ws = tl_info?
            .workspace
            .iter()
            .filter_map(|ws_handle| self.get_workspace_from_handle(ws_handle))
            .last();
        if ws.is_none() {
            log::debug!(
                "toplevel_id={} workspace info not found",
                tl_info?.identifier
            );
            return None;
        }
        Some(AppToplevel::new(
            tl_info?,
            &ws?,
            self.expected_output.as_ref(),
        ))
    }

    fn send_event(&mut self, event: WaylandEvent) {
        let _ = self.sender.try_send(event);
    }

    fn get_matching_toplevel(&self, toplevel: &AppToplevel) -> Option<&AppToplevel> {
        self.workspace_toplevels
            .get(&toplevel.ws_handle)
            .and_then(|ws_toplevels| ws_toplevels.get(&toplevel.handle))
    }

    fn is_active_output(&self, output: &WlOutput) -> bool {
        self.expected_output.is_none() || Some(output) == self.expected_output.as_ref()
    }

    fn add_top_level(&mut self, toplevel: AppToplevel) {
        let ws_id = &toplevel.ws_handle;
        let tl_id = &toplevel.handle;
        self.remove_toplevel(tl_id);
        let mut ws_toplevels = self
            .workspace_toplevels
            .get(ws_id)
            .cloned()
            .unwrap_or_default();
        ws_toplevels.insert(tl_id.clone(), toplevel.clone());
        self.workspace_toplevels.insert(ws_id.clone(), ws_toplevels);
        self.toplevels.insert(tl_id.clone(), toplevel);
    }

    fn remove_toplevel(&mut self, handle: &ExtForeignToplevelHandleV1) -> bool {
        if let Some(toplevel) = self.toplevels.remove(handle) {
            let ws_id = &toplevel.ws_handle;
            if let Some(ws_toplevels) = self.workspace_toplevels.get_mut(ws_id) {
                ws_toplevels.remove(handle);
                return true;
            } else {
                log::debug!("toplevel_id={} remove - workspace not found", handle.id());
            }
        } else {
            log::debug!(
                "toplevel_id={} remove ignored - toplevel not found",
                handle.id()
            );
        }
        false
    }
}

/// WorkspaceHandler trait implementation.
///
/// This trait defines callbacks for workspace-related events from2 the compositor.
/// The compositor uses a batching model: it sends multiple events, then calls done()
/// to signal "all updates have been sent, now process them as a batch".
impl WorkspaceHandler for AppData {
    fn workspace_state(&mut self) -> &mut WorkspaceState {
        &mut self.workspace_state
    }

    /// Called when the compositor has finished sending all workspace state updates.
    /// This is where we process the accumulated changes and send them to the app.
    fn done(&mut self) {
        let mut new_state = HashMap::new();
        for group in self.workspace_state.workspace_groups() {
            let include = group
                .outputs
                .iter()
                .any(|output| self.is_active_output(output));
            if !include {
                continue;
            }
            for workspace_handle in &group.workspaces {
                if let Some(ws) = self.get_workspace_from_handle(workspace_handle) {
                    new_state.insert(ws.handle.clone(), ws);
                } else {
                    log::debug!(
                        "workspace_handle_id={} could not retrieve workspace info",
                        workspace_handle.id()
                    );
                }
            }
        }
        let old_state = &self.workspaces;
        if *old_state == new_state {
            return;
        }

        let removed_keys = old_state
            .keys()
            .filter(|&k| !new_state.contains_key(k))
            .cloned()
            .collect::<Vec<_>>();
        for key in removed_keys {
            self.workspace_toplevels.remove(&key);
        }

        self.workspaces = new_state;
        let mut workspaces_vec = self.workspaces.values().cloned().collect::<Vec<_>>();
        workspaces_vec.sort_by_key(|ws| ws.name.clone());
        self.send_event(WaylandEvent::WorkspacesChanged(workspaces_vec));
    }
}

/// ToplevelInfoHandler trait implementation.
///
/// This trait defines callbacks for window/toplevel-related events.
/// A "toplevel" is a top-level window (not a popup or subsurface).
/// In COSMIC, stacked/tabbed windows appear as a single toplevel.
impl ToplevelInfoHandler for AppData {
    fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState {
        &mut self.toplevel_info_state
    }

    /// Called when a new window/toplevel is created.
    fn new_toplevel(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        handle: &ExtForeignToplevelHandleV1,
    ) {
        if let Some(tl) = self.get_toplevel_from_handle(handle) {
            self.add_top_level(tl);
            self.send_event(WaylandEvent::ToplevelsUpdated(
                self.workspace_toplevels.clone(),
            ));
        } else {
            log::debug!(
                "toplevel_handle_id={} ignored - could not retrieve toplevel info from handle",
                handle.id()
            );
        }
    }

    /// Called when an existing toplevel's state changes (title, app_id, activated state, etc.)
    fn update_toplevel(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        toplevel: &ExtForeignToplevelHandleV1,
    ) {
        if let Some(new_app_toplevel) = self.get_toplevel_from_handle(toplevel) {
            let old_app_toplevel = self.get_matching_toplevel(&new_app_toplevel);
            let equals = old_app_toplevel
                .map(|old_app_top_level| *old_app_top_level == new_app_toplevel)
                .unwrap_or(false);
            if !equals {
                self.add_top_level(new_app_toplevel);
                self.send_event(WaylandEvent::ToplevelsUpdated(
                    self.workspace_toplevels.clone(),
                ));
            } else {
                log::debug!(
                    "toplevel_id={}, app_id={} update ignored - no changes detected",
                    new_app_toplevel.handle.id(),
                    new_app_toplevel.app_id
                );
            }
        }
    }

    /// Called when a toplevel window is closed/destroyed.
    fn toplevel_closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        handle: &ExtForeignToplevelHandleV1,
    ) {
        let tl = self.get_toplevel_from_handle(handle);
        if let Some(toplevel) = tl {
            let tl_id = toplevel.handle;
            let removed = self.remove_toplevel(&tl_id);
            if removed {
                self.send_event(WaylandEvent::ToplevelsUpdated(
                    self.workspace_toplevels.clone(),
                ));
            }
        } else {
            log::debug!(
                "toplevel_handle_id={} close ignored - could not retrieve toplevel info from handle",
                handle.id()
            );
        }
    }
}

impl ProvidesRegistryState for AppData {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    sctk::registry_handlers![OutputState,];
}

impl OutputHandler for AppData {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
        let info = self.output_state.info(&output).unwrap();
        if info.name.as_deref() == Some(&self.configured_output) {
            self.expected_output = Some(output);
        }
    }

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {
        log::info!("Hello");
    }

    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {
        log::info!("Hello")
    }
}
/*
impl SeatHandler for AppData {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        _capability: sctk::seat::Capability,
    ) {
    }
    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        _capability: sctk::seat::Capability,
    ) {
    }
    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {
    }
}
*/
// Delegate macros: These generate boilerplate code to wire up Wayland event dispatching.
//
// The Wayland protocol works by having the compositor send events over a socket.
// The client library needs to know "when event X arrives, which handler method to call".
// These delegate macros generate that dispatching code automatically.
//
// Without these macros, you'd need to manually implement the Dispatch trait for each
// protocol interface, routing events to the appropriate handler methods.
cctk::delegate_workspace!(AppData); // Routes workspace events to WorkspaceHandler methods
cctk::delegate_toplevel_info!(AppData); // Routes toplevel events to ToplevelInfoHandler methods
sctk::delegate_output!(AppData); // Routes output (monitor) events to OutputHandler methods
//sctk::delegate_seat!(AppData);            // Routes seat (input device) events to SeatHandler methods
sctk::delegate_registry!(AppData); // Routes registry (global discovery) events

/// Starts the Wayland event loop in a background thread.
///
/// This function:
/// 1. Creates a channel for sending events to the iced application
/// 2. Spawns a background thread that runs the Wayland event loop
/// 3. Returns the receiver end of the channel as a stream
///
/// The background thread:
/// - Connects to the Wayland compositor's global registry
/// - Binds to the workspace and toplevel info protocols
/// - Enters an infinite loop that processes Wayland events
/// - When events occur, they're handled by the trait implementations and sent via the channel
async fn start(conn: Connection) -> mpsc::Receiver<WaylandEvent> {
    let (sender, receiver) = mpsc::channel(16);

    thread::spawn(move || {
        // Initialize the Wayland event queue and discover available global objects
        let (globals, mut event_queue) = registry_queue_init(&conn).unwrap();
        let qh = event_queue.handle();

        // Check which monitor/output this applet instance is running on
        let configured_output = std::env::var("COSMIC_PANEL_OUTPUT")
            .ok()
            .unwrap_or_default();

        // Initialize state managers by binding to Wayland protocol interfaces
        // Each of these sends a request to the compositor to start receiving events
        let registry_state = RegistryState::new(&globals);
        let output_state = OutputState::new(&globals, &qh);
        let workspace_state = WorkspaceState::new(&registry_state, &qh);
        let toplevel_info_state = ToplevelInfoState::new(&registry_state, &qh);
        //let seat_state = SeatState::new(&globals, &qh);

        let mut app_data = AppData {
            registry_state,
            output_state,
            workspace_state,
            toplevel_info_state,
            //seat_state,
            sender,
            toplevels: HashMap::new(),
            workspace_toplevels: HashMap::new(),
            workspaces: HashMap::new(),
            configured_output: configured_output.clone(),
            expected_output: None,
        };

        // Check for existing outputs that match the configured output
        // If no specific output is configured, use the first available output
        for output in app_data.output_state.outputs() {
            if let Some(info) = app_data.output_state.info(&output) {
                if configured_output.is_empty() || info.name.as_deref() == Some(&configured_output)
                {
                    app_data.expected_output = Some(output.clone());
                    break;
                }
            }
        }

        // Main event loop: waits for events from compositor and dispatches to handlers
        // blocking_dispatch() blocks until events arrive, then calls the appropriate
        // handler methods on app_data based on the delegate macros above
        loop {
            event_queue
                .blocking_dispatch(&mut app_data)
                .unwrap_or_else(|err| {
                    // TODO: Handle Wayland disconnection gracefully
                    eprintln!("Wayland event dispatch error: {:?}", err);
                    0
                });
        }
    });

    receiver
}
