// SPDX-License-Identifier: MPL-2.0

use crate::config::Config;
use crate::fl;
use crate::wayland_subscription::{self, WorkspaceEvent, WorkspaceInfo, ToplevelAppInfo};
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::{window::Id, Length, Limits, Subscription};
use cosmic::iced_winit::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::event::{wayland::Event as WaylandEvent, PlatformSpecific};
use cosmic::prelude::*;
use cosmic::widget;
use cosmic::cctk::wayland_client::{Connection, Proxy};
use futures_util::SinkExt;
use std::collections::HashMap;
use std::path::PathBuf;
use cosmic::Action::App;
use cosmic::iced_widget::button;

/// The application model stores app-specific state used to describe its interface and
/// drive its logic.
#[derive(Default)]
pub struct AppModel {
    /// Application state which is managed by the COSMIC runtime.
    core: cosmic::Core,
    /// The popup id.
    popup: Option<Id>,
    /// Configuration data that persists between application runs.
    config: Config,
    /// Example row toggler.
    example_row: bool,
    /// Wayland connection for workspace subscription
    wayland_conn: Option<Connection>,
    /// Current workspaces
    workspaces: Vec<WorkspaceInfo>,
    /// Current toplevels/applications
    toplevels: HashMap<String, ToplevelAppInfo>,
    /// App icon cache
    app_icons: HashMap<String, PathBuf>,
}

/// Messages emitted by the application and its widgets.
#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    SubscriptionChannel,
    UpdateConfig(Config),
    ToggleExampleRow(bool),
    WaylandEvent(WaylandEvent),
    WorkspaceEvent(WorkspaceEvent),
    IconLoaded(String, PathBuf),
}

/// Create a COSMIC application from the app model
impl cosmic::Application for AppModel {
    /// The async executor that will be used to run your application's commands.
    type Executor = cosmic::executor::Default;

    /// Data that your application receives to its init method.
    type Flags = ();

    /// Messages which the application and its widgets will emit.
    type Message = Message;

    /// Unique identifier in RDNN (reverse domain name notation) format.
    const APP_ID: &'static str = "com.github.mlu1109.cosmic-applet-workspace-apps";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    /// Initializes the application with any given flags and startup commands.
    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        // Construct the app model with the runtime's core.
        let app = AppModel {
            core,
            config: cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
                .map(|context| match Config::get_entry(&context) {
                    Ok(config) => config,
                    Err((_errors, config)) => {
                        // for why in errors {
                        //     tracing::error!(%why, "error loading app config");
                        // }

                        config
                    }
                })
                .unwrap_or_default(),
            ..Default::default()
        };

        (app, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    /// Describes the interface based on the current state of the application model.
    ///
    /// The applet's button in the panel will be drawn using the main view method.
    /// This view should emit messages to toggle the applet's popup window, which will
    /// be drawn using the `view_window` method.
    fn view(&self) -> Element<'_, Self::Message> {
        let mut row = widget::row().spacing(4).padding([0, 8]);
        // Display each workspace with its number and app icons
        row = row.push("Test");
        for workspace in &self.workspaces {
            let workspace_num = if !workspace.coordinates.is_empty() {
                workspace.coordinates[0] + 1
            } else {
                0
            };

            // Create workspace number button
            let ws_button = button(
                widget::text(format!("{}", workspace_num))
                    .size(14)
            )
            .padding([2, 6]);
                //.style(cosmic::theme::Button::Text);
            row = row.push(ws_button);

            // Add app icons for this workspace
            for toplevel_desc in &workspace.top_levels {
                // Extract app_id from "app_id: title" format
                if let Some(app_id) = toplevel_desc.split(':').next() {
                    let app_id = app_id.trim();

                    if let Some(icon_path) = self.app_icons.get(app_id) {
                        let icon = widget::icon::from_path(icon_path.clone()).icon().size(16);
                        row = row.push(icon);
                    } else {
                        // Show a placeholder or text if icon not loaded
                        let placeholder = widget::text(app_id.chars().next().unwrap_or('?').to_string())
                            .size(12);
                        row = row.push(placeholder);
                    }
                }
            }

            // Add separator between workspaces
            if workspace_num < self.workspaces.len() as u32 {
                row = row.push(widget::text("|").size(12));
            }
        }
        row = row.width(Length::Fill);
        widget::container(row)
            .width(Length::Fill)
            .padding([4, 8])
            .into()
    }

    /// The applet's popup window will be drawn using this view method. If there are
    /// multiple poups, you may match the id parameter to determine which popup to
    /// create a view for.
    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let content_list = widget::list_column()
            .padding(5)
            .spacing(0)
            .add(widget::settings::item(
                fl!("example-row"),
                widget::toggler(self.example_row).on_toggle(Message::ToggleExampleRow),
            ));

        self.core.applet.popup_container(content_list).into()
    }

    /// Register subscriptions for this application.
    ///
    /// Subscriptions are long-lived async tasks running in the background which
    /// emit messages to the application through a channel. They may be conditionally
    /// activated by selectively appending to the subscription batch, and will
    /// continue to execute for the duration that they remain in the batch.
    fn subscription(&self) -> Subscription<Self::Message> {
        struct MySubscription;
        struct WaylandEventsSubscription;

        let mut subscriptions = vec![
            // Create a subscription which emits updates through a channel.
            Subscription::run_with_id(
                std::any::TypeId::of::<MySubscription>(),
                cosmic::iced::stream::channel(4, move |mut channel| async move {
                    _ = channel.send(Message::SubscriptionChannel).await;

                    futures_util::future::pending().await
                }),
            ),
            // Watch for application configuration changes.
            self.core()
                .watch_config::<Config>(Self::APP_ID)
                .map(|update| {
                    // for why in update.errors {
                    //     tracing::error!(?why, "app config error");
                    // }

                    Message::UpdateConfig(update.config)
                }),
            // Listen for Wayland events to get the connection
            cosmic::iced::event::listen_with(|evt, _, _| match evt {
                cosmic::iced::Event::PlatformSpecific(PlatformSpecific::Wayland(evt)) => {
                    Some(Message::WaylandEvent(evt))
                }
                _ => None,
            }),
        ];

        // Add workspace subscription if we have a connection
        if let Some(conn) = &self.wayland_conn {
            subscriptions.push(
                wayland_subscription::workspace_subscription(conn.clone())
                    .map(Message::WorkspaceEvent)
            );
        }

        Subscription::batch(subscriptions)
    }

    /// Handles messages emitted by the application and its widgets.
    ///
    /// Tasks may be returned for asynchronous execution of code in the background
    /// on the application's async runtime. The application will not exit until all
    /// tasks are finished.
    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::SubscriptionChannel => {
                // For example purposes only.
            }
            Message::UpdateConfig(config) => {
                self.config = config;
            }
            Message::ToggleExampleRow(toggled) => self.example_row = toggled,
            Message::WaylandEvent(evt) => {
                // Extract Wayland connection from the event
                if self.wayland_conn.is_none() {
                    if let WaylandEvent::Output(_evt, output) = evt {
                        if let Some(backend) = output.backend().upgrade() {
                            self.wayland_conn = Some(Connection::from_backend(backend));
                        }
                    }
                }
            }
            Message::WorkspaceEvent(WorkspaceEvent::WorkspacesChanged(workspaces)) => {
                self.workspaces = workspaces;
                println!("Workspaces updated: {} workspaces", self.workspaces.len());

                // Collect all app_ids that need icons
                let mut app_ids_to_load = Vec::new();
                for ws in &self.workspaces {
                    println!("  - {} (coords: {:?})", ws.name, ws.coordinates);
                    for app in &ws.top_levels {
                        println!("    -> {}", app);
                        if let Some(app_id) = app.split(':').next() {
                            let app_id = app_id.trim().to_string();
                            if !self.app_icons.contains_key(&app_id) {
                                app_ids_to_load.push(app_id);
                            }
                        }
                    }
                }

                // Load icons asynchronously
                let tasks: Vec<_> = app_ids_to_load.into_iter().map(|app_id| {
                    Task::perform(
                        load_app_icon(app_id.clone()),
                        move |path| App(Message::IconLoaded(app_id.clone(), path))
                    )
                }).collect();
                return Task::batch(tasks);
            }
            Message::WorkspaceEvent(WorkspaceEvent::ToplevelAdded(toplevel)) => {
                println!("Toplevel added: {} - {} (workspaces: {:?})",
                    toplevel.app_id, toplevel.title, toplevel.workspaces);
                self.toplevels.insert(toplevel.id.clone(), toplevel.clone());

                // Load icon if not already loaded
                if !self.app_icons.contains_key(&toplevel.app_id) {
                    let app_id = toplevel.app_id.clone();
                    return Task::perform(
                        load_app_icon(app_id.clone()),
                        move |path| App(Message::IconLoaded(app_id.clone(), path))
                    );
                }
            }
            Message::WorkspaceEvent(WorkspaceEvent::ToplevelUpdated(toplevel)) => {
                println!("Toplevel updated: {} - {} (workspaces: {:?})",
                    toplevel.app_id, toplevel.title, toplevel.workspaces);
                self.toplevels.insert(toplevel.id.clone(), toplevel);
            }
            Message::WorkspaceEvent(WorkspaceEvent::ToplevelRemoved(id)) => {
                println!("Toplevel removed: {}", id);
                self.toplevels.remove(&id);
            }
            Message::IconLoaded(app_id, path) => {
                println!("Icon loaded for {}: {:?}", app_id, path);
                self.app_icons.insert(app_id, path);
            }
            Message::TogglePopup => {
                return if let Some(p) = self.popup.take() {
                    destroy_popup(p)
                } else {
                    let new_id = Id::unique();
                    self.popup.replace(new_id);
                    let mut popup_settings = self.core.applet.get_popup_settings(
                        self.core.main_window_id().unwrap(),
                        new_id,
                        None,
                        None,
                        None,
                    );
                    popup_settings.positioner.size_limits = Limits::NONE
                        .max_width(372.0)
                        .min_width(300.0)
                        .min_height(200.0)
                        .max_height(1080.0);
                    get_popup(popup_settings)
                }
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                }
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced_runtime::Appearance> {
        Some(cosmic::applet::style())
    }
}

async fn load_app_icon(app_id: String) -> PathBuf {
    tokio::task::spawn_blocking(move || {
        freedesktop_icons::lookup(&app_id)
            .with_size(16)
            .with_cache()
            .find()
            .unwrap_or_else(|| {
                freedesktop_icons::lookup("application-default")
                    .with_size(16)
                    .with_cache()
                    .find()
                    .unwrap_or_default()
            })
    })
    .await
    .unwrap_or_default()
}
