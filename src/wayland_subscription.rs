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

#[derive(Clone, Debug)]
pub enum WaylandEvent {
    WorkspacesChanged(Vec<AppWorkspace>),
    ToplevelsUpdated(String, HashMap<String, HashMap<String, AppToplevel>>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppWorkspace {
    pub id: String, // TODO: Use ObjectId?
    pub name: String,
    pub is_active: bool,
}

impl AppWorkspace {
    pub fn new(info: &Workspace) -> Self {
        let id = info.handle.id().to_string();
        let name = info.name.clone();
        let is_active = info.state.contains(ext_workspace_handle_v1::State::Active);
        AppWorkspace { id, name, is_active }
    }
}


#[derive(Clone, Debug, PartialEq)]
pub struct AppToplevel {
    pub id: String,
    pub app_id: String,
    pub is_active: bool,
    pub workspace_id: String, // FIXME: Assumes that a toplevel is only on one workspace
    pub geometry: (i32, i32, i32, i32), // x, y, width, height
}

impl AppToplevel {
    pub fn new(handle: &ExtForeignToplevelHandleV1, info: &ToplevelInfo, wl_output: Option<&WlOutput>) -> Self {
        let id = handle.id().to_string();
        let app_id = info.app_id.clone();
        let is_active = info.state.contains(&zcosmic_toplevel_handle_v1::State::Activated);
        let geometry = wl_output.map(|output| info.geometry.get(output)).flatten().map(|geometry| (geometry.x, geometry.y, geometry.width, geometry.height)).unwrap_or_default();
        let workspace_id = if let Some(ws) = info.workspace.iter().find(|_| true) {
            ws.id().clone().to_string()
        } else {
            "???".to_string() // FIXME: Do something, pick the first workspace? "Fallback workspace"?
        };
        AppToplevel { id, app_id, is_active, workspace_id, geometry }
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
    registry_state: RegistryState,           // Tracks available Wayland global objects
    output_state: OutputState,               // Tracks display/monitor information
    workspace_state: WorkspaceState,         // Tracks workspace (virtual desktop) state
    toplevel_info_state: ToplevelInfoState,  // Tracks window/toplevel information
    //seat_state: SeatState,                   // Tracks input devices (keyboard, mouse)

    // Communication channel to send events to the iced application
    sender: mpsc::Sender<WaylandEvent>,

    // Mirrored app state
    workspaces: Vec<AppWorkspace>,
    toplevels: HashMap<String, AppToplevel>,
    workspace_toplevels: HashMap<String, HashMap<String, AppToplevel>>,


    // Output (monitor) filtering - which display this applet is running on
    configured_output: String,               // Name from COSMIC_PANEL_OUTPUT env var
    expected_output: Option<WlOutput>,       // Resolved Wayland output object
}

impl AppData {
    fn send_event(&mut self, event: WaylandEvent) {
        let _ = self.sender.try_send(event);
    }

    fn get_matching_toplevel(&self, toplevel: AppToplevel) -> Option<&AppToplevel> {
        let ws = toplevel.workspace_id;
        self.workspace_toplevels.get(ws.as_str()).and_then(|ws_toplevels| ws_toplevels.get(&toplevel.id))
    }

    fn is_active_output(&self, output: &WlOutput) -> bool {
        if let Some(expected) = &self.expected_output {
            expected.id() == output.id()
        } else {
            true
        }
    }

    fn add_top_level(&mut self, toplevel: AppToplevel) {
        let ws_id = &toplevel.workspace_id;
        let mut ws_toplevels = self.workspace_toplevels.get(ws_id).cloned().unwrap_or_default();
        ws_toplevels.insert(toplevel.id.clone(), toplevel.clone());
        self.workspace_toplevels.insert(ws_id.to_string(), ws_toplevels); // TODO: Necessary?
        self.toplevels.insert(toplevel.id.clone(), toplevel);
    }

    fn remove_toplevel(&mut self, id: &str) -> bool {
        if let Some(toplevel) = self.toplevels.remove(id) {
            let ws_id = &toplevel.workspace_id;
            if let Some(ws_toplevels) = self.workspace_toplevels.get_mut(ws_id) {
                ws_toplevels.remove(id);
                return true
            }
        }
        false
    }
}

/// WorkspaceHandler trait implementation.
///
/// This trait defines callbacks for workspace-related events from the compositor.
/// The compositor uses a batching model: it sends multiple events, then calls done()
/// to signal "all updates have been sent, now process them as a batch".
impl WorkspaceHandler for AppData {
    fn workspace_state(&mut self) -> &mut WorkspaceState {
        &mut self.workspace_state
    }

    /// Called when the compositor has finished sending all workspace state updates.
    /// This is where we process the accumulated changes and send them to the app.
    fn done(&mut self) {
        let mut new_state = Vec::new();

        for group in self.workspace_state.workspace_groups() {
            let include = group.outputs.iter().any(|output| self.is_active_output(output));
            if !include {
                continue;
            }
            for workspace_handle in &group.workspaces {
                if let Some(workspace) = self.workspace_state.workspace_info(workspace_handle) {
                    let app_workspace = AppWorkspace::new(workspace);
                    new_state.push(app_workspace);
                }
            }
        }
        new_state.sort_by(|a, b| a.name.cmp(&b.name));

        let old_state = self.workspaces.clone();
        if old_state == new_state {
            return;
        }

        self.workspaces = new_state;

        self.send_event(WaylandEvent::WorkspacesChanged(self.workspaces.clone()));
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
        toplevel: &ExtForeignToplevelHandleV1,
    ) {
        if let Some(info) = self.toplevel_info_state.info(toplevel) {
            let toplevel = AppToplevel::new(toplevel, info, self.expected_output.as_ref());
            let toplevel_id = toplevel.id.clone();
            self.add_top_level(toplevel);
            self.send_event(WaylandEvent::ToplevelsUpdated(toplevel_id, self.workspace_toplevels.clone()));
        }
    }

    /// Called when an existing toplevel's state changes (title, app_id, activated state, etc.)
    fn update_toplevel(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        toplevel: &ExtForeignToplevelHandleV1,
    ) {
        if let Some(info) = self.toplevel_info_state.info(toplevel) {
            let new_app_top_level = AppToplevel::new(toplevel, info, self.expected_output.as_ref());
            let old_app_top_level = self.get_matching_toplevel(new_app_top_level.clone());
            if Some(&new_app_top_level) == old_app_top_level {
                return;
            }
            let toplevel_id = new_app_top_level.id.clone();
            self.add_top_level(new_app_top_level);
            self.send_event(WaylandEvent::ToplevelsUpdated(toplevel_id, self.workspace_toplevels.clone()));
        }
    }

    /// Called when a toplevel window is closed/destroyed.
    fn toplevel_closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        toplevel: &ExtForeignToplevelHandleV1,
    ) {
        let toplevel_id = toplevel.id().to_string();
        let removed = self.remove_toplevel(toplevel_id.as_str());
        if removed {
            self.send_event(WaylandEvent::ToplevelsUpdated(toplevel_id, self.workspace_toplevels.clone()));
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

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: WlOutput,
    ) {
        let info = self.output_state.info(&output).unwrap();
        if info.name.as_deref() == Some(&self.configured_output) {
            self.expected_output = Some(output);
        }
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: WlOutput,
    ) {}

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: WlOutput,
    ) {}
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
cctk::delegate_workspace!(AppData);       // Routes workspace events to WorkspaceHandler methods
cctk::delegate_toplevel_info!(AppData);   // Routes toplevel events to ToplevelInfoHandler methods
sctk::delegate_output!(AppData);          // Routes output (monitor) events to OutputHandler methods
//sctk::delegate_seat!(AppData);            // Routes seat (input device) events to SeatHandler methods
sctk::delegate_registry!(AppData);        // Routes registry (global discovery) events

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
            workspaces: Vec::new(),
            configured_output,
            expected_output: None,
        };

        // Main event loop: waits for events from compositor and dispatches to handlers
        // blocking_dispatch() blocks until events arrive, then calls the appropriate
        // handler methods on app_data based on the delegate macros above
        loop {
            event_queue.blocking_dispatch(&mut app_data).unwrap();
        }
    });

    receiver
}
