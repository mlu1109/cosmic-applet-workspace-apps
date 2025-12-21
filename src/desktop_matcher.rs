use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Represents a parsed desktop entry with relevant fields
#[derive(Debug, Clone)]
pub struct DesktopEntry {
    pub path: PathBuf,
    pub startup_wm_class: Option<String>,
    pub icon: Option<String>,
}

/// Desktop file matcher that searches for .desktop files matching an app ID
pub struct DesktopMatcher {
    /// Cache of desktop entries indexed by filename (without .desktop extension)
    filename_index: HashMap<String, DesktopEntry>,
    /// Cache of desktop entries indexed by lowercase filename
    lowercase_filename_index: HashMap<String, DesktopEntry>,
    /// Cache of desktop entries indexed by StartupWMClass
    wm_class_index: HashMap<String, DesktopEntry>,
}

impl DesktopMatcher {
    /// Create a new desktop matcher by scanning XDG data directories
    pub fn new() -> Self {
        let mut matcher = Self {
            filename_index: HashMap::new(),
            lowercase_filename_index: HashMap::new(),
            wm_class_index: HashMap::new(),
        };
        matcher.scan_directories();
        matcher
    }

    /// Scan XDG data directories for desktop files
    fn scan_directories(&mut self) {
        let data_dirs = Self::get_xdg_data_dirs();
        
        for data_dir in data_dirs {
            let apps_dir = Path::new(&data_dir).join("applications");
            if !apps_dir.exists() {
                continue;
            }

            if let Ok(entries) = fs::read_dir(&apps_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("desktop") {
                        if let Some(entry) = Self::parse_desktop_file(&path) {
                            self.index_entry(entry);
                        }
                    }
                }
            }
        }
    }

    /// Index a desktop entry for fast lookup
    pub fn index_entry(&mut self, entry: DesktopEntry) {
        if let Some(filename) = entry.path.file_stem().and_then(|s| s.to_str()) {
            let filename_str = filename.to_string();
            
            // Index by exact filename (only if not already present - first one wins)
            self.filename_index
                .entry(filename_str.clone())
                .or_insert_with(|| entry.clone());
            
            // Index by lowercase filename for case-insensitive search
            self.lowercase_filename_index
                .entry(filename_str.to_lowercase())
                .or_insert_with(|| entry.clone());
            
            // Index by StartupWMClass if present
            if let Some(ref wm_class) = entry.startup_wm_class {
                self.wm_class_index
                    .entry(wm_class.clone())
                    .or_insert(entry);
            }
        }
    }

    /// Parse a desktop file and extract relevant fields
    pub fn parse_desktop_file(path: &Path) -> Option<DesktopEntry> {
        let file = fs::File::open(path).ok()?;
        let reader = BufReader::new(file);
        
        let mut in_desktop_entry = false;
        let mut startup_wm_class = None;
        let mut icon = None;
        
        for line in reader.lines().flatten() {
            let line = line.trim();
            
            // Check if we're in the [Desktop Entry] section
            if line == "[Desktop Entry]" {
                in_desktop_entry = true;
                continue;
            } else if line.starts_with('[') {
                in_desktop_entry = false;
                continue;
            }
            
            if !in_desktop_entry {
                continue;
            }
            
            // Parse key=value pairs
            if let Some((key, value)) = line.split_once('=') {
                match key.trim() {
                    "StartupWMClass" => startup_wm_class = Some(value.trim().to_string()),
                    "Icon" => icon = Some(value.trim().to_string()),
                    _ => {}
                }
            }
        }
        
        Some(DesktopEntry {
            path: path.to_path_buf(),
            startup_wm_class,
            icon,
        })
    }

    /// Find a desktop file matching the given app ID
    /// 
    /// Tries multiple strategies in order:
    /// 1. Exact filename match
    /// 2. StartupWMClass match
    /// 3. Case-insensitive filename match
    pub fn find_desktop_file(&self, app_id: &str) -> Option<&DesktopEntry> {
        if let Some(entry) = self.filename_index.get(app_id) {
            return Some(entry);
        }
        
        if let Some(entry) = self.wm_class_index.get(app_id) {
            return Some(entry);
        }
        
        let app_id_lower = app_id.to_lowercase();
        if let Some(entry) = self.lowercase_filename_index.get(&app_id_lower) {
            return Some(entry);
        }
        
        None
    }

    pub fn get_xdg_data_dirs() -> Vec<String> {
        let mut dirs = Vec::new();
        
        if let Some(data_home) = Self::get_xdg_data_home() {
            dirs.push(data_home);
        }
        
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
        dirs.extend(data_dirs.split(':').map(String::from));
        
        dirs
    }

    fn get_xdg_data_home() -> Option<String> {
        std::env::var("XDG_DATA_HOME").ok().or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|home| format!("{}/.local/share", home))
        })
    }
}

impl Default for DesktopMatcher {
    fn default() -> Self {
        Self::new()
    }
}
