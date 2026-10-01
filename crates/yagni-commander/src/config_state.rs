//! The settings in use, where they come from, and why the file might not be
//! in use. A gpui global, read by every view.

use std::path::PathBuf;

use gpui_kit::Global;
use yagni_commander_core::Config;

#[derive(Default)]
pub(crate) struct CurrentConfig {
    pub config: Config,
    /// The config file; `None` without a config directory.
    pub path: Option<PathBuf>,
    /// Why the file isn't in use (it doesn't parse, or there is no config
    /// directory). While set, the settings dialog can't save.
    pub problem: Option<String>,
}

impl Global for CurrentConfig {}

impl CurrentConfig {
    /// At startup: the file's settings, or defaults and the problem. A
    /// broken file is never overwritten.
    pub fn load(path: Option<PathBuf>) -> Self {
        let Some(path) = path else {
            return Self {
                problem: Some("No config directory found".into()),
                ..Default::default()
            };
        };
        match Config::load_or_create(&path) {
            Ok(config) => Self {
                config,
                path: Some(path),
                problem: None,
            },
            Err(e) => Self {
                config: Config::default(),
                path: Some(path),
                problem: Some(format!("Config ignored: {e}")),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_reports_a_broken_file_and_runs_on_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "log = maybe\n").unwrap();
        let state = CurrentConfig::load(Some(path.clone()));
        assert_eq!(state.config, Config::default());
        assert!(state.problem.unwrap().starts_with("Config ignored: "));
        assert_eq!(state.path, Some(path));
    }

    #[test]
    fn load_without_a_config_directory_has_a_problem() {
        let state = CurrentConfig::load(None);
        assert_eq!(state.problem.as_deref(), Some("No config directory found"));
    }

    #[test]
    fn load_creates_the_template() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let state = CurrentConfig::load(Some(path.clone()));
        assert!(state.problem.is_none());
        assert!(path.exists());
    }
}
