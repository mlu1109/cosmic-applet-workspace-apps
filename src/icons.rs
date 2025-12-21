use std::{collections::HashMap, path::PathBuf};

use cosmic::widget;

use crate::desktop_matcher::DesktopMatcher;

const FALLBACK_ICON: &[u8] = include_bytes!("../resources/fallback-icon.svg");

pub struct Icons {
    fallback_icon: widget::icon::Icon,
    app_id_cache: HashMap<String, widget::icon::Icon>,
    desktop_matcher: DesktopMatcher,
}

impl Icons {
    pub fn new() -> Self {
        Self {
            fallback_icon: widget::icon::from_svg_bytes(FALLBACK_ICON).icon(),
            app_id_cache: HashMap::new(),
            desktop_matcher: DesktopMatcher::new(),
        }
    }

    pub fn get_icon(&self, app_id: &str) -> widget::icon::Icon {
        self.app_id_cache.get(app_id).unwrap_or_else(|| &self.fallback_icon).clone()
    }

    pub fn load_icon_if_missing(&mut self, app_id: &str) {
        if !self.app_id_cache.contains_key(app_id) {
            let icon = self.load_icon(app_id);
            self.app_id_cache.insert(app_id.to_string(), icon);
        }
    }

    fn load_icon(&self, app_id: &str) -> widget::icon::Icon {
        let icon_value = self
            .desktop_matcher
            .find_desktop_file(app_id)
            .map(|df| df.icon.clone())
            .flatten();
        let icon_path = match icon_value {
            Some(ref icon_value) if PathBuf::from(icon_value).is_absolute() => {
                Some(PathBuf::from(icon_value))
            }
            Some(ref icon_value) => Self::lookup_icon_path(&icon_value),
            None => Self::lookup_icon_path(app_id),
        };
        if let Some(path) = icon_path {
            widget::icon::from_path(path).icon()
        } else {
            self.fallback_icon.clone()
        }
    }

    fn lookup_icon_path(name: &str) -> Option<PathBuf> {
        freedesktop_icons::lookup(name).find()
    }
}
