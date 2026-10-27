//! Shift-Enter, "Open with": the applications registered for a file's type,
//! and starting one of them with the file. Linux follows the freedesktop
//! specs (`desktop` for the applications, `mime` for the type); macOS asks
//! Launch Services (`macos`).

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::shell_path;
use crate::viewer::FileSource;

#[cfg(not(target_os = "macos"))]
pub mod desktop;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
pub mod mime;

/// An application that can open the file, with its command line for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// As the desktop shows it.
    pub name: String,
    /// The one the system opens the file with.
    pub default: bool,
    pub program: OsString,
    pub args: Vec<OsString>,
    /// Where it starts: the file's folder, or the one its entry names.
    pub dir: PathBuf,
}

/// What [`apps_for`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The default first, then by name.
    pub apps: Vec<App>,
    /// The file's type (a MIME type), for "no application" messages.
    pub kind: Option<String>,
}

/// The applications for the regular file `path` (links followed). Reads
/// the file's first bytes and the system's registrations, so callers run it
/// on a thread of their own.
pub fn apps_for(path: &Path) -> io::Result<Found> {
    let source = FileSource::open(path)?;
    #[cfg(not(target_os = "macos"))]
    let found = {
        let env = desktop::Env::current();
        let types = mime::types_of(&source, path, &env)?;
        Found {
            apps: desktop::apps(&env, &types, path),
            kind: types.first().cloned(),
        }
    };
    #[cfg(target_os = "macos")]
    let found = {
        drop(source);
        Found {
            apps: macos::apps(path)?,
            kind: None,
        }
    };
    Ok(Found {
        apps: ordered(found.apps),
        ..found
    })
}

/// The default first, then by name, ignoring case.
pub fn ordered(mut apps: Vec<App>) -> Vec<App> {
    apps.sort_by_cached_key(|app| (!app.default, app.name.to_lowercase()));
    apps
}

/// Starts `app` with its file, without waiting. Only failing to start is an
/// error: a GUI application may exit non-zero when closed.
pub fn start(app: &App) -> io::Result<()> {
    let mut child = shell_path::command(&app.program)
        .args(&app.args)
        .current_dir(&app.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("cannot start “{}”: {e}", app.name)))?;
    // Reap it when it exits, so finished applications don't linger as zombies.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, default: bool) -> App {
        App {
            name: name.into(),
            default,
            program: "true".into(),
            args: Vec::new(),
            dir: PathBuf::from("/"),
        }
    }

    #[test]
    fn the_default_comes_first_then_names_ignoring_case() {
        let apps = ordered(vec![
            app("kate", false),
            app("Okular", false),
            app("Zed", true),
            app("Ark", false),
        ]);
        let names: Vec<_> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Zed", "Ark", "kate", "Okular"]);
    }

    #[test]
    fn start_runs_the_command_in_its_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        let app = App {
            args: vec![
                "-c".into(),
                format!("pwd > '{0}.part' && mv '{0}.part' '{0}'", out.display()).into(),
            ],
            program: "sh".into(),
            dir: tmp.path().to_path_buf(),
            ..app("Shell", false)
        };
        start(&app).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !out.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let written = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            written.trim_end(),
            tmp.path().canonicalize().unwrap().to_str().unwrap()
        );
    }

    #[test]
    fn a_missing_program_is_an_error_naming_the_application() {
        let app = App {
            program: "/nonexistent/program".into(),
            ..app("Ghost", false)
        };
        let err = start(&app).unwrap_err();
        assert!(err.to_string().contains("“Ghost”"), "{err}");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn apps_for_finds_a_text_files_type_with_the_systems_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("notes");
        std::fs::write(&file, "plain words\n").unwrap();
        let found = apps_for(&file).unwrap();
        assert_eq!(found.kind.as_deref(), Some("text/plain"));
        assert!(
            found
                .apps
                .iter()
                .all(|app| app.args.contains(&file.clone().into()))
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn apps_for_refuses_what_is_not_a_regular_file() {
        let tmp = tempfile::tempdir().unwrap();
        let err = apps_for(tmp.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
