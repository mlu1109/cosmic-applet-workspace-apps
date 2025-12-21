# Desktop File Matcher Library - Implementation Plan

## Objective
Create a separate library module in this project that finds matching .desktop files for a given app_id without modifying existing code.

## Matching Strategies

The library will implement the following matching strategies in priority order:

1. **Exact filename match**
   - Example: app_id `org.mozilla.firefox` → `org.mozilla.firefox.desktop`
   
2. **StartupWMClass field match**
   - Parse .desktop files and match against the `StartupWMClass` key
   - This is the primary method for mapping window classes to desktop entries per freedesktop.org spec
   - Example: app_id `Firefox` might match a desktop file with `StartupWMClass=Firefox`

3. **Case-insensitive filename match**
   - Some applications use inconsistent casing
   - Example: app_id `Firefox` → `firefox.desktop`

4. **Base name fallback**
   - Extract the last component after the final dot
   - Example: app_id `org.mozilla.firefox` → try `firefox.desktop`
   - Useful for reverse-DNS style app IDs

## Additional Cases to Consider

Beyond the cases mentioned, the library should also handle:

5. **Icon field extraction**
   - Since the existing code uses desktop files for icon lookup, include this in the DesktopEntry struct

6. **Name field extraction**
   - Useful for displaying human-readable application names

7. **XDG directory scanning**
   - Follow XDG Base Directory Specification:
     - `$XDG_DATA_HOME/applications/` (default: `~/.local/share/applications/`)
     - `$XDG_DATA_DIRS/applications/` (default: `/usr/local/share/applications/:/usr/share/applications/`)

## Implementation Structure

### New File: `src/desktop_matcher.rs`

**Public API:**
```rust
pub struct DesktopMatcher {
    // Internal indexes for fast lookup
}

pub struct DesktopEntry {
    pub path: PathBuf,
    pub startup_wm_class: Option<String>,
    pub icon: Option<String>,
    pub name: Option<String>,
}

impl DesktopMatcher {
    pub fn new() -> Self
    pub fn find_desktop_file(&self, app_id: &str) -> Option<&DesktopEntry>
}
```

**Internal Design:**
- Use HashMap indexes for O(1) lookups:
  - `filename_index`: exact filename → DesktopEntry
  - `lowercase_filename_index`: lowercase filename → DesktopEntry
  - `wm_class_index`: StartupWMClass → DesktopEntry
- Parse all .desktop files once during initialization
- No external dependencies required (use std library only)

### Changes Required

1. **Create** `src/desktop_matcher.rs` - New module file
2. **Update** `src/main.rs` - Add `mod desktop_matcher;` declaration only

## Testing Considerations

The library can be tested by:
- Running on a real system with existing .desktop files
- Verifying common app_id patterns (Firefox, Chrome, VSCode, etc.)
- Checking that existing code continues to work unchanged

## Non-Goals

- Not modifying any existing code in `app.rs`, `config.rs`, `wayland_subscription.rs`, etc.
- Not replacing the existing icon loading logic
- Not adding new dependencies to Cargo.toml
