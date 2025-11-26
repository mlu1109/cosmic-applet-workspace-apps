// SPDX-License-Identifier: MPL-2.0

use cosmic::cctk::{
    self,
    sctk::{
        self,
        output::{OutputHandler, OutputState},
        registry::{ProvidesRegistryState, RegistryState},
        seat::{SeatHandler, SeatState},
    },
    toplevel_info::{ToplevelInfo, ToplevelInfoHandler, ToplevelInfoState},
    wayland_client::{
        Connection, QueueHandle,
        globals::registry_queue_init,
        protocol::{wl_seat, wl_output::{WlOutput}},
    },
    workspace::{WorkspaceHandler, WorkspaceState},
};
use cosmic::iced;
use futures_channel::mpsc;
use futures_util::StreamExt;
use std::{collections::HashMap, thread};
use std::collections::HashSet;
use cosmic::cctk::cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1::State;
use cosmic::cctk::wayland_client::Proxy;
use cosmic::cctk::wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1;
use cosmic::cctk::wayland_protocols::ext::workspace::v1::client::ext_workspace_handle_v1::ExtWorkspaceHandleV1;

#[derive(Clone, Debug)]
pub enum WorkspaceEvent {
    WorkspacesChanged(Vec<WorkspaceInfo>),
    ToplevelAdded(ToplevelAppInfo),
    ToplevelUpdated(ToplevelAppInfo),
    ToplevelRemoved(String),
}

#[derive(Clone, Debug)]
pub struct WorkspaceInfo {
    pub name: String,
    pub top_levels: Vec<String>,
    pub is_active: bool,
}

#[derive(Clone, Debug)]
pub struct ToplevelAppInfo {
    pub id: String,
    pub app_id: String,
    pub is_active: bool,
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
pub fn workspace_subscription() -> iced::Subscription<WorkspaceEvent> {
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
    seat_state: SeatState,                   // Tracks input devices (keyboard, mouse)
    
    // Communication channel to send events to the iced application
    sender: mpsc::Sender<WorkspaceEvent>,
    
    // Local tracking: maps each toplevel window to its workspaces
    toplevel_workspaces: HashMap<ExtForeignToplevelHandleV1, HashSet<ExtWorkspaceHandleV1>>,
    
    // Output (monitor) filtering - which display this applet is running on
    configured_output: String,               // Name from COSMIC_PANEL_OUTPUT env var
    expected_output: Option<WlOutput>,       // Resolved Wayland output object
}

impl AppData {
    fn send_event(&mut self, event: WorkspaceEvent) {
        let _ = self.sender.try_send(event);
    }

    fn toplevel_to_app_info(&self, handle: &ExtForeignToplevelHandleV1, info: &ToplevelInfo) -> ToplevelAppInfo {
        let is_active  = info.state.contains(&State::Activated);
        let top_level = ToplevelAppInfo {
            id: handle.id().to_string(),
            app_id: info.app_id.clone(),
            is_active,
        };
        top_level
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
        let mut workspaces = Vec::new();

        // Build a map of workspace handles to names
        let mut workspace_names = HashMap::new();
        for group in self.workspace_state.workspace_groups() {
            for workspace_handle in &group.workspaces {
                if let Some(workspace) = self.workspace_state.workspace_info(workspace_handle) {
                    workspace_names.insert(workspace_handle.clone(), workspace.name.clone());
                }
            }
        }

        // Get top_levels per workspace, filtered by current display
        for group in self.workspace_state.workspace_groups() {
            // Filter by current output (display)
            let is_current_output = self.expected_output.as_ref()
                .map(|expected| group.outputs.iter().any(|o| o == expected))
                .unwrap_or(true);
            
            if !is_current_output {
                continue;
            }

            for workspace_handle in &group.workspaces {
                if let Some(workspace) = self.workspace_state.workspace_info(workspace_handle) {
                    let mut top_levels_with_pos = Vec::new();
                    
                    // Find all top_levels in this workspace with their position
                    for (toplevel_handle, toplevel_workspaces) in &self.toplevel_workspaces {
                        if toplevel_workspaces.contains(workspace_handle) {
                            if let Some(info) = self.toplevel_info_state.info(toplevel_handle) {
                                // Get position from geometry for the current output
                                let (x_pos, y_pos) = self.expected_output.as_ref()
                                    .and_then(|output| info.geometry.get(output))
                                    .map(|g| (g.x, g.y))
                                    .or_else(|| info.geometry.values().next().map(|g| (g.x, g.y)))
                                    .unwrap_or((0, 0));
                                top_levels_with_pos.push((toplevel_handle.id().to_string(), x_pos, y_pos));
                            } else {
                                // Fallback if no info available
                                top_levels_with_pos.push((toplevel_handle.id().to_string(), 0, 0));
                            }
                        }
                    }
                    
                    // Sort by x position (left to right), then by y position (top to bottom)
                    top_levels_with_pos.sort_by_key(|(_, x, y)| (*x, *y));
                    let toplevel_ids: Vec<String> = top_levels_with_pos.into_iter()
                        .map(|(id, _, _)| id)
                        .collect();

                    workspaces.push(WorkspaceInfo {
                        name: workspace.name.clone(),
                        top_levels: toplevel_ids,
                        is_active: workspace.state.contains(cosmic::cctk::wayland_protocols::ext::workspace::v1::client::ext_workspace_handle_v1::State::Active),
                    });
                }
            }
        }

        self.send_event(WorkspaceEvent::WorkspacesChanged(workspaces));
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
            self.toplevel_workspaces.insert(toplevel.clone(), info.workspace.clone());
            let app_info = self.toplevel_to_app_info(toplevel, info);
            self.send_event(WorkspaceEvent::ToplevelAdded(app_info));
            self.done();
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
            self.toplevel_workspaces.insert(toplevel.clone(), info.workspace.clone());
            let app_info = self.toplevel_to_app_info(toplevel, info);
            self.send_event(WorkspaceEvent::ToplevelUpdated(app_info));
            self.done();
        }
    }

    /// Called when a toplevel window is closed/destroyed.
    fn toplevel_closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        toplevel: &ExtForeignToplevelHandleV1,
    ) {
        self.toplevel_workspaces.remove(toplevel);
        let id = format!("{:?}", toplevel.id());
        self.send_event(WorkspaceEvent::ToplevelRemoved(id));
        self.done();
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
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: WlOutput,
    ) {
    }
}

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
sctk::delegate_seat!(AppData);            // Routes seat (input device) events to SeatHandler methods
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
async fn start(conn: Connection) -> mpsc::Receiver<WorkspaceEvent> {
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
        let seat_state = SeatState::new(&globals, &qh);

        let mut app_data = AppData {
            registry_state,
            output_state,
            workspace_state,
            toplevel_info_state,
            seat_state,
            sender,
            toplevel_workspaces: HashMap::new(),
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
