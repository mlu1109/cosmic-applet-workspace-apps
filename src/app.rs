// SPDX-License-Identifier: MPL-2.0

use crate::config::Config;
use crate::wayland_subscription::{self, WorkspaceEvent, WorkspaceInfo, ToplevelAppInfo};
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::{Limits, Subscription};
use cosmic::prelude::*;
use cosmic::widget;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use cosmic::Action::App;

static AUTOSIZE_MAIN_ID: LazyLock<widget::Id> = LazyLock::new(|| widget::Id::new("autosize-main"));

/// The application model stores app-specific state used to describe its interface and
/// drive its logic.
#[derive(Default)]
pub struct AppModel {
    /// Application state which is managed by the COSMIC runtime.
    core: cosmic::Core,
    /// Configuration data that persists between application runs.
    config: Config,
    /// Current workspaces
    workspaces: Vec<WorkspaceInfo>,
    /// Current applications
    top_levels: HashMap<String, ToplevelAppInfo>,
    /// App icon cache
    app_icons: HashMap<String, Option<PathBuf>>,
}

/// Messages emitted by the application and its widgets.
#[derive(Debug, Clone)]
pub enum Message {
    UpdateConfig(Config),
    WorkspaceEvent(WorkspaceEvent),
    IconLoaded(String, Option<PathBuf>),
}

/// Create a COSMIC application from the app model
impl AppModel {
    fn workspace_button(&self, workspace: &WorkspaceInfo) -> Element<'_, Message> {
        // Calculate icon size based on panel height (use ~60% of panel height, capped)
        let icon_size = if let Some(b) = self.core.applet.suggested_bounds {
            (b.height as f32 * 0.6).min(24.0).max(12.0) as u16
        } else {
            16
        };
        
        let mut content = widget::row()
            .spacing(2)
            .align_y(cosmic::iced::Alignment::Center);
        
        let text = widget::text(format!("{}", workspace.name))
            .size(14);
        
        let text = if workspace.is_active {
            text.font(cosmic::iced::Font {
                weight: cosmic::iced::font::Weight::Bold,
                ..Default::default()
            })
        } else {
            text
        };
        
        content = content.push(text);

        for toplevel_id in &workspace.top_levels {
            if let Some(toplevel_info) = self.top_levels.get(toplevel_id) {
                let app_id = &toplevel_info.app_id;
                let is_active = toplevel_info.is_active;
                if let Some(icon_path) = self.app_icons.get(app_id) {
                    if icon_path.is_none() {
                        continue;
                    }
                    let icon = widget::icon::from_path(icon_path.clone().unwrap())
                        .icon()
                        .size(icon_size);
                    
                    let icon_element: Element<'_, Message> = if is_active {
                        widget::container(icon)
                            .style(move |theme| {
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
                        icon.into()
                    };
                    content = content.push(icon_element);
                } else {
                    let placeholder = widget::text(app_id.chars().next().unwrap_or('?').to_string())
                        .size(12);
                    content = content.push(placeholder);
                }
            }
        }

        let is_active = workspace.is_active;
        let container = widget::container(content)
            .padding([4, 8])
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

    /// Describes the interface based on the current state of the application model.
    ///
    /// The applet's button in the panel will be drawn using the main view method.
    /// This view should emit messages to toggle the applet's popup window, which will
    /// be drawn using the `view_window` method.
    fn view(&self) -> Element<'_, Self::Message> {
        let mut row = widget::row().spacing(4);

        if self.workspaces.is_empty() {
            row = row.push(widget::text("...").size(14));
        } else {
            for workspace in &self.workspaces {
                row = row.push(self.workspace_button(workspace));
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

        widget::autosize::autosize(
            widget::container(row).padding(0),
            AUTOSIZE_MAIN_ID.clone(),
        )
        .limits(limits)
        .into()
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
                .map(|update| {
                    Message::UpdateConfig(update.config)
                }),
            // Workspace subscription
            wayland_subscription::workspace_subscription()
                .map(Message::WorkspaceEvent),
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
            Message::WorkspaceEvent(WorkspaceEvent::WorkspacesChanged(workspaces)) => {
                self.workspaces = workspaces;
                self.workspaces.sort_by(|a, b| a.name.cmp(&b.name));

                // Collect all app_ids that need icons
                let mut app_ids_to_load = Vec::new();
                for ws in &self.workspaces {
                    for toplevel_id in &ws.top_levels {
                        if let Some(toplevel_info) = self.top_levels.get(toplevel_id) {
                            let app_id = toplevel_info.app_id.clone();
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
                self.top_levels.insert(toplevel.id.clone(), toplevel.clone());

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
                self.top_levels.insert(toplevel.id.clone(), toplevel);
            }
            Message::WorkspaceEvent(WorkspaceEvent::ToplevelRemoved(id)) => {
                self.top_levels.remove(&id);
            }
            Message::IconLoaded(app_id, path) => {
                self.app_icons.insert(app_id, path);
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced_runtime::Appearance> {
        Some(cosmic::applet::style())
    }
}

async fn load_app_icon(app_id: String) -> Option<PathBuf> {
    tokio::task::spawn_blocking(move || {
        // Try direct lookup first
        if let Some(path) = freedesktop_icons::lookup(&app_id)
            .with_size(16)
            .with_cache()
            .find() {
            return Some(path);
        }
        
        // Try case-insensitive lookup
        let app_id_lower = app_id.to_lowercase();
        if let Some(path) = freedesktop_icons::lookup(&app_id_lower)
            .with_size(16)
            .with_cache()
            .find() {
            return Some(path);
        }
        
        // Search desktop files for matching StartupWMClass
        if let Some(icon_name) = find_icon_from_desktop_file(&app_id) {
            // Try the icon name from desktop file
            if let Some(path) = freedesktop_icons::lookup(&icon_name)
                .with_size(16)
                .with_cache()
                .find() {
                return Some(path);
            }
            
            // If icon is an absolute path, use it directly
            if std::path::Path::new(&icon_name).is_absolute() && std::path::Path::new(&icon_name).exists() {
                return Some(PathBuf::from(icon_name));
            }
        }
        
        // Fallback to default
        freedesktop_icons::lookup("application-default")
            .with_size(16)
            .with_cache()
            .find()
    })
    .await
    .unwrap_or_default()
}

fn find_icon_from_desktop_file(app_id: &str) -> Option<String> {
    use std::fs;
    use std::io::{BufRead, BufReader};

    let icon_name = ["XDG_DATA_DIRS", "XDG_DATA_HOME", "HOME"]
        .iter()
        .find_map(|variable| {
            std::env::var(variable).ok().and_then(|value| {
                value.split(':').find_map(|dir| {
                    let app_dir = format!("{}/applications", dir);
                    if let Ok(entries) = fs::read_dir(app_dir) {
                        for entry in entries.flatten() {
                            if let Ok(file) = fs::File::open(entry.path()) {
                                let reader = BufReader::new(file);
                                let mut icon_name = None;
                                let mut matches = false;

                                for line in reader.lines().flatten() {
                                    if line.starts_with("Icon=") {
                                        icon_name = Some(line[5..].to_string());
                                    } else if line.starts_with("StartupWMClass=") {
                                        let wm_class = &line[15..];
                                        if wm_class.eq_ignore_ascii_case(app_id) {
                                            matches = true;
                                        }
                                    }
                                }

                                if matches && icon_name.is_some() {
                                    return icon_name;
                                }
                            }
                        }
                    }
                    None
                })
            })
        });


    icon_name
}
