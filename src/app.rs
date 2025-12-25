// SPDX-License-Identifier: MPL-2.0

use crate::config::Config;
use crate::icons::Icons;
use crate::wayland_subscription::{self, AppToplevel, AppWorkspace, WaylandEvent};
use cosmic::applet::Size;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::{Limits, Subscription};
use cosmic::prelude::*;
use cosmic::widget;
use std::collections::HashMap;
use std::sync::LazyLock;
use wayland_protocols::ext::workspace::v1::client::ext_workspace_handle_v1::ExtWorkspaceHandleV1;

static AUTOSIZE_MAIN_ID: LazyLock<widget::Id> = LazyLock::new(|| widget::Id::new("autosize-main"));

pub struct AppModel {
    /// Application state which is managed by the COSMIC runtime.
    core: cosmic::Core,
    /// Configuration data that persists between application runs.
    config: Config,
    /// Current workspaces
    workspaces: Vec<AppWorkspace>,
    /// Current applications
    workspace_toplevels: HashMap<ExtWorkspaceHandleV1, Vec<AppToplevel>>,
    /// App icon cache
    app_icons: Icons,
}

#[derive(Debug, Clone)]
pub enum Message {
    UpdateConfig(Config),
    WaylandEvent(WaylandEvent),
}

impl AppModel {
    fn get_workspace_toplevels(&self, workspace: &AppWorkspace) -> Vec<AppToplevel> {
        let res = self.workspace_toplevels.get(&workspace.handle);
        if let Some(res) = res {
            res.clone()
        } else {
            Vec::new()
        }
    }

    fn new_workspace_button(&self, workspace: &AppWorkspace) -> Element<'_, Message> {
        // Use the applet context to get proper sizing based on panel configuration
        let icon_size = self.core.applet.suggested_size(true).0;
        let text_size = match &self.core.applet.size {
            Size::PanelSize(panel_size) => {
                let size = panel_size.get_applet_icon_size_with_padding(false);
                // Scale text with panel size
                (size as f32 * 0.4).max(10.0) as u16
            }
            Size::Hardcoded((_w, _h)) => 14,
        };

        let spacing = self.core.applet.spacing as f32;
        let icon_spacing = self.core.applet.spacing as f32 * 0.5;
        let (padding_major, padding_minor) = self.core.applet.suggested_padding(true);
        let padding = if self.core.applet.is_horizontal() {
            [padding_minor as f32, padding_major as f32]
        } else {
            [padding_major as f32, padding_minor as f32]
        };

        let mut content = widget::row()
            .spacing(icon_spacing)
            .align_y(cosmic::iced::Alignment::Center);

        let text = widget::text(format!("{}", workspace.name)).size(text_size);

        let text = if workspace.is_active {
            text.font(cosmic::iced::Font {
                weight: cosmic::iced::font::Weight::Bold,
                ..Default::default()
            })
        } else {
            text
        };

        content = content.push(text);

        let ws_top_levels = self.get_workspace_toplevels(workspace);

        if !ws_top_levels.is_empty() {
            content = content.push(widget::horizontal_space().width(spacing + 2.0));
        }

        for toplevel in ws_top_levels {
            let element = self.new_application_icon_element(
                toplevel.app_id.as_str(),
                toplevel.is_active,
                icon_size,
            );
            content = content.push(element);
        }

        let is_active = workspace.is_active;
        let container = widget::container(content)
            .padding(padding)
            .style(move |theme| {
                let cosmic = theme.cosmic();
                widget::container::Style {
                    background: None,
                    text_color: if is_active {
                        Some(cosmic.on_bg_color().into())
                    } else {
                        Some(cosmic::iced::Color {
                            a: 0.5,
                            ..cosmic.on_bg_color().into()
                        })
                    },
                    border: cosmic::iced_core::Border {
                        width: if is_active { 2.0 } else { 0.0 },
                        color: if is_active {
                            cosmic.accent_color().into()
                        } else {
                            cosmic::iced::Color::TRANSPARENT
                        },
                        radius: cosmic.radius_s().into(),
                    },
                    ..Default::default()
                }
            });
        container.into()
    }

    fn new_application_icon_element(
        &self,
        app_id: &str,
        is_active: bool,
        icon_size: u16,
    ) -> Element<'_, Message> {
        let icon = self.app_icons.get_icon(app_id).size(icon_size);
        let container = widget::container(icon).center(icon_size as f32 + 4.0);
        if is_active {
            container
                .style(move |theme: &Theme| {
                    let cosmic = theme.cosmic();
                    widget::container::Style {
                        background: None,
                        text_color: None,
                        border: cosmic::iced_core::Border {
                            width: 1.5,
                            color: cosmic.accent_color().into(),
                            radius: cosmic.radius_xs().into(),
                        },
                        ..Default::default()
                    }
                })
                .into()
        } else {
            container.into()
        }
    }
}

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
            workspace_toplevels: HashMap::new(),
            workspaces: Vec::new(),
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
            app_icons: Icons::new(),
        };

        (app, Task::none())
    }

    /// Register subscriptions for this application.
    ///
    /// Subscriptions are long-lived async tasks running in the background which
    /// emit messages to the application through a channel. They may be conditionally
    /// activated by selectively appending to the subscription batch, and will
    /// continue to execute for the duration that they remain in the batch.
    fn subscription(&self) -> Subscription<Self::Message> {
        let subscriptions = vec![
            // Watch for application configuration changes.
            self.core()
                .watch_config::<Config>(Self::APP_ID)
                .map(|update| Message::UpdateConfig(update.config)),
            // Workspace subscription
            wayland_subscription::workspace_subscription().map(Message::WaylandEvent),
        ];

        Subscription::batch(subscriptions)
    }

    /// Handles messages emitted by the application and its widgets.
    ///
    /// Tasks may be returned for asynchronous execution of code in the background
    /// on the application's async runtime. The application will not exit until all
    /// tasks are finished.
    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::UpdateConfig(config) => {
                self.config = config;
            }
            Message::WaylandEvent(WaylandEvent::WorkspacesChanged(workspaces)) => {
                self.workspaces = workspaces;
                self.workspaces.sort_by_key(|ws| ws.coordinates);
            }
            Message::WaylandEvent(WaylandEvent::ToplevelsUpdated(ws_toplevels)) => {
                let mut transformed = HashMap::new();
                for (ws_id, toplevels_by_id) in ws_toplevels {
                    let mut toplevels: Vec<AppToplevel> = Vec::new();
                    for toplevel in toplevels_by_id.values() {
                        self.app_icons.load_icon_if_missing(&toplevel.app_id);
                        toplevels.push(toplevel.clone());
                    }
                    toplevels.sort_by_key(|tl| tl.coordinates);
                    transformed.insert(ws_id, toplevels);
                }
                self.workspace_toplevels = transformed;
            }
        }
        Task::none()
    }

    /// Describes the interface based on the current state of the application model.
    ///
    /// The applet's button in the panel will be drawn using the main view method.
    /// This view should emit messages to toggle the applet's popup window, which will
    /// be drawn using the `view_window` method.
    fn view(&self) -> Element<'_, Self::Message> {
        // Use applet spacing configuration
        let row_spacing = self.core.applet.spacing as f32;
        let text_size = match &self.core.applet.size {
            Size::PanelSize(panel_size) => {
                let size = panel_size.get_applet_icon_size_with_padding(false);
                (size as f32 * 0.4).max(10.0) as u16
            }
            Size::Hardcoded(_) => 14,
        };

        let mut row = widget::row().spacing(row_spacing);

        if self.workspaces.is_empty() {
            row = row.push(widget::text("...").size(text_size));
        } else {
            for workspace in &self.workspaces {
                row = row.push(self.new_workspace_button(workspace));
            }
        }

        let mut limits = Limits::NONE.min_width(1.).min_height(1.);
        if let Some(b) = self.core.applet.suggested_bounds {
            if b.width as i32 > 0 {
                limits = limits.max_width(b.width);
            }
            if b.height as i32 > 0 {
                limits = limits.max_height(b.height);
            }
        }

        widget::autosize::autosize(widget::container(row).padding(0), AUTOSIZE_MAIN_ID.clone())
            .limits(limits)
            .into()
    }

    fn style(&self) -> Option<cosmic::iced_runtime::Appearance> {
        Some(cosmic::applet::style())
    }
}
