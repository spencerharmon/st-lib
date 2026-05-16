//! Shared config-file location lookup for st-suite tools.
//!
//! Convention: each tool reads `<app>.yaml` from the first existing path of:
//!
//! 1. `$HOME/.config/st-tools/<app>.yaml`
//! 2. `/etc/st-tools/<app>.yaml`
//!
//! Parsing is left to the caller.

use std::path::PathBuf;

/// Find the config file for `app_name` (without extension; `.yaml` is appended).
///
/// Returns the first existing path, or `None` if no config is found.
pub fn find_config(app_name: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = home::home_dir() {
        candidates.push(home.join(".config/st-tools").join(format!("{}.yaml", app_name)));
    }
    candidates.push(PathBuf::from(format!("/etc/st-tools/{}.yaml", app_name)));

    candidates.into_iter().find(|p| p.exists())
}
