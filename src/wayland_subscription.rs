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
        protocol::{wl_seat, wl_output::{self, WlOutput}},
    },
    workspace::{WorkspaceHandler, WorkspaceState},
};
use cosmic::iced;
use futures_channel::mpsc;
use futures_util::StreamExt;
use std::{collections::HashMap, thread};
use std::collections::HashSet;
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
    pub coordinates: Vec<u32>,
    pub top_levels: Vec<String>,
    pub is_active: bool,
}

#[derive(Clone, Debug)]
pub struct ToplevelAppInfo {
    pub id: String,
    pub app_id: String,
    pub title: String,
    pub workspaces: Vec<String>,
}

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

pub struct AppData {
    #[allow(dead_code)]
    qh: QueueHandle<Self>,
    registry_state: RegistryState,
    output_state: OutputState,
    workspace_state: WorkspaceState,
    toplevel_info_state: ToplevelInfoState,
    seat_state: SeatState,
    sender: mpsc::Sender<WorkspaceEvent>,
    toplevel_workspaces: HashMap<ExtForeignToplevelHandleV1, HashSet<ExtWorkspaceHandleV1>>,
    configured_output: String,
    expected_output: Option<WlOutput>,
}

impl AppData {
    fn send_event(&mut self, event: WorkspaceEvent) {
        let _ = self.sender.try_send(event);
    }

    fn get_workspace_name(&self, handle: &ExtWorkspaceHandleV1) -> Option<String> {
        self.workspace_state.workspace_info(handle).map(|ws| ws.name.clone())
    }

    fn toplevel_to_app_info(&self, handle: &ExtForeignToplevelHandleV1, info: &ToplevelInfo) -> ToplevelAppInfo {
        let workspaces = info.workspace.iter()
            .filter_map(|ws| self.get_workspace_name(ws))
            .collect();
        
        ToplevelAppInfo {
            id: format!("{:?}", handle.id()),
            app_id: info.app_id.clone(),
            title: info.title.clone(),
            workspaces,
        }
    }
}

impl WorkspaceHandler for AppData {
    fn workspace_state(&mut self) -> &mut WorkspaceState {
        &mut self.workspace_state
    }

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
                    let mut toplevel_ids = Vec::new();
                    
                    // Find all top_levels in this workspace
                    for (toplevel_handle, toplevel_workspaces) in &self.toplevel_workspaces {
                        if toplevel_workspaces.contains(workspace_handle) {
                            if let Some(info) = self.toplevel_info_state.info(toplevel_handle) {
                                toplevel_ids.push(format!("{}: {}", info.app_id, info.title));
                            }
                        }
                    }

                    workspaces.push(WorkspaceInfo {
                        name: workspace.name.clone(),
                        coordinates: workspace.coordinates.clone(),
                        top_levels: toplevel_ids,
                        is_active: workspace.state.contains(cosmic::cctk::wayland_protocols::ext::workspace::v1::client::ext_workspace_handle_v1::State::Active),
                    });
                }
            }
        }

        self.send_event(WorkspaceEvent::WorkspacesChanged(workspaces));
    }
}

impl ToplevelInfoHandler for AppData {
    fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState {
        &mut self.toplevel_info_state
    }

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
        output: wl_output::WlOutput,
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
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
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

cctk::delegate_workspace!(AppData);
cctk::delegate_toplevel_info!(AppData);
sctk::delegate_output!(AppData);
sctk::delegate_seat!(AppData);
sctk::delegate_registry!(AppData);

async fn start(conn: Connection) -> mpsc::Receiver<WorkspaceEvent> {
    let (sender, receiver) = mpsc::channel(16);

    thread::spawn(move || {
        let (globals, mut event_queue) = registry_queue_init(&conn).unwrap();
        let qh = event_queue.handle();

        let configured_output = std::env::var("COSMIC_PANEL_OUTPUT")
            .ok()
            .unwrap_or_default();

        let registry_state = RegistryState::new(&globals);
        let output_state = OutputState::new(&globals, &qh);
        let workspace_state = WorkspaceState::new(&registry_state, &qh);
        let toplevel_info_state = ToplevelInfoState::new(&registry_state, &qh);
        let seat_state = SeatState::new(&globals, &qh);

        let mut app_data = AppData {
            qh: qh.clone(),
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

        loop {
            event_queue.blocking_dispatch(&mut app_data).unwrap();
        }
    });

    receiver
}
