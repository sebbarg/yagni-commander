//! Ctrl-Shift-T: the default terminal emulator, started in a folder.
//!
//! Linux has no single "default terminal", so the first of these wins:
//! `$TERMINAL` (Omarchy sets it to `xdg-terminal-exec`); on KDE, the
//! terminal chosen in System Settings (`TerminalApplication` in
//! `kdeglobals`, Konsole when unset); `xdg-terminal-exec`;
//! `x-terminal-emulator` (Debian's alternatives link); then a list of
//! common terminals on `PATH`. macOS opens Terminal.app.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

/// What the choice depends on, read from the process and the user's files
/// by [`Env::current`]; tests build their own.
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// `$TERMINAL`.
    pub terminal: Option<String>,
    /// `$XDG_CURRENT_DESKTOP` names KDE.
    pub kde: bool,
    /// The contents of `~/.config/kdeglobals`.
    pub kdeglobals: Option<String>,
    /// `$PATH`.
    pub path: Option<OsString>,
}

/// A terminal to start: the program and its arguments, the folder last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: OsString,
    pub args: Vec<OsString>,
}

/// Terminals looked for on `PATH` after the explicit choices.
const KNOWN: [&str; 7] = [
    "konsole",
    "gnome-terminal",
    "kitty",
    "alacritty",
    "foot",
    "wezterm",
    "xterm",
];

/// The terminal to start in `dir` on Linux and the BSDs, or `None` when
/// none is found. `$TERMINAL` and KDE's setting are taken as given (a
/// failure to start them is reported); the others only when on `PATH`.
pub fn choose(env: &Env, dir: &Path) -> Option<Launch> {
    let explicit = env
        .terminal
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| env.kde.then(|| kde_terminal(env.kdeglobals.as_deref())));
    if let Some(command) = explicit {
        let mut words = shlex::split(&command)?.into_iter();
        let program = words.next()?;
        let real = find_in_path(&program, env.path.as_deref())
            .map_or_else(|| PathBuf::from(&program), |p| real_path(&p));
        return Some(with_dir(
            program.into(),
            words.map(Into::into).collect(),
            &real,
            dir,
        ));
    }
    ["xdg-terminal-exec", "x-terminal-emulator"]
        .iter()
        .chain(&KNOWN)
        .find_map(|name| find_in_path(name, env.path.as_deref()))
        .map(|found| {
            let real = real_path(&found);
            with_dir(found.into_os_string(), Vec::new(), &real, dir)
        })
}

/// The folder argument some terminals need, besides being started in it:
/// Konsole's profile can name its own start folder, gnome-terminal opens
/// windows from a server, and xdg-terminal-exec passes `--dir` on.
fn with_dir(program: OsString, mut args: Vec<OsString>, real: &Path, dir: &Path) -> Launch {
    let name = real
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir_text = dir.as_os_str();
    let joined = |option: &str| {
        let mut arg = OsString::from(option);
        arg.push(dir_text);
        arg
    };
    match name.as_str() {
        "konsole" => args.extend([OsString::from("--workdir"), dir_text.to_owned()]),
        n if n.starts_with("gnome-terminal") => args.push(joined("--working-directory=")),
        "xdg-terminal-exec" => args.push(joined("--dir=")),
        _ => {}
    }
    Launch { program, args }
}

/// KDE's terminal: `TerminalApplication` in `[General]` of `kdeglobals`
/// (a key may carry a `[$e]` flag), else Konsole, KDE's default.
fn kde_terminal(kdeglobals: Option<&str>) -> String {
    let mut in_general = false;
    for line in kdeglobals.unwrap_or_default().lines() {
        let line = line.trim();
        if line.starts_with('[') && !line.starts_with("[$") {
            in_general = line == "[General]";
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.split('[').next().unwrap_or(key).trim();
        if in_general && key == "TerminalApplication" && !value.trim().is_empty() {
            return value.trim().to_owned();
        }
    }
    "konsole".to_owned()
}

/// `name` as an executable file: a path as given, else the first match in
/// `path` (a `PATH`-style list).
fn find_in_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if name.contains('/') {
        return Some(PathBuf::from(name)).filter(|p| executable(p));
    }
    std::env::split_paths(path?)
        .map(|dir| dir.join(name))
        .find(|p| executable(p))
}

/// Where a link (an alternatives link, say) ends up; the path itself if
/// that can't be read.
fn real_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl Env {
    /// This process's environment and the user's `kdeglobals`.
    pub fn current() -> Self {
        let kde = std::env::var("XDG_CURRENT_DESKTOP")
            .is_ok_and(|d| d.split(':').any(|part| part.eq_ignore_ascii_case("KDE")));
        Self {
            terminal: std::env::var("TERMINAL").ok(),
            kde,
            kdeglobals: kde
                .then(dirs::config_dir)
                .flatten()
                .and_then(|dir| std::fs::read_to_string(dir.join("kdeglobals")).ok()),
            path: crate::shell_path::current(),
        }
    }
}

/// Ctrl-Shift-T: starts `command` (split like a shell would) or else the
/// default terminal (macOS: Terminal.app), in `dir`, without waiting. Only
/// failing to start is an error: a shell's exit code is its own business.
pub fn open_terminal(command: Option<&str>, dir: &Path) -> io::Result<()> {
    let launch = resolve(command, &Env::current(), cfg!(target_os = "macos"), dir)?;
    let mut child = crate::shell_path::command(&launch.program)
        .args(&launch.args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            let name = launch.program.to_string_lossy();
            io::Error::new(e.kind(), format!("cannot start “{name}”: {e}"))
        })?;
    // Reap it when it exits, so closed terminals don't linger as zombies.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// What [`open_terminal`] starts.
fn resolve(command: Option<&str>, env: &Env, macos: bool, dir: &Path) -> io::Result<Launch> {
    if let Some(command) = command {
        let mut words = shlex::split(command)
            .ok_or_else(|| io::Error::other("the terminal command has unbalanced quotes"))?
            .into_iter();
        let program = words
            .next()
            .ok_or_else(|| io::Error::other("the terminal command is empty"))?;
        return Ok(Launch {
            program: program.into(),
            args: words.map(Into::into).collect(),
        });
    }
    if macos {
        return Ok(Launch {
            program: "open".into(),
            args: vec!["-a".into(), "Terminal".into(), dir.as_os_str().to_owned()],
        });
    }
    choose(env, dir).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "No terminal emulator found. Set $TERMINAL to the one you use.",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    /// A folder of fake programs, as `PATH`.
    fn bin(names: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for name in names {
            let path = tmp.path().join(name);
            std::fs::write(&path, b"#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        tmp
    }

    fn env(path: &Path) -> Env {
        Env {
            path: Some(path.as_os_str().to_owned()),
            ..Env::default()
        }
    }

    fn strings(launch: &Launch) -> Vec<String> {
        std::iter::once(&launch.program)
            .chain(&launch.args)
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    const DIR: &str = "/work/my folder";

    #[test]
    fn terminal_variable_comes_first_with_its_arguments() {
        let path = bin(&["xdg-terminal-exec", "konsole"]);
        let e = Env {
            terminal: Some("kitty --single-instance".into()),
            kde: true,
            ..env(path.path())
        };
        let launch = choose(&e, Path::new(DIR)).unwrap();
        assert_eq!(strings(&launch), ["kitty", "--single-instance"]);
    }

    #[test]
    fn omarchys_xdg_terminal_exec_gets_the_folder() {
        let path = bin(&[]);
        let e = Env {
            terminal: Some("xdg-terminal-exec".into()),
            ..env(path.path())
        };
        let launch = choose(&e, Path::new(DIR)).unwrap();
        assert_eq!(
            strings(&launch),
            ["xdg-terminal-exec", &format!("--dir={DIR}")]
        );
    }

    #[test]
    fn kde_uses_the_system_settings_choice_or_konsole() {
        let path = bin(&["xdg-terminal-exec"]);
        let mut e = Env {
            kde: true,
            ..env(path.path())
        };
        let launch = choose(&e, Path::new(DIR)).unwrap();
        assert_eq!(strings(&launch), ["konsole", "--workdir", DIR]);
        e.kdeglobals = Some("[KDE]\nfoo=1\n[General]\nTerminalApplication[$e]=alacritty\n".into());
        assert_eq!(strings(&choose(&e, Path::new(DIR)).unwrap()), ["alacritty"]);
        // Only the [General] group counts.
        e.kdeglobals = Some("[Other]\nTerminalApplication=xterm\n".into());
        assert_eq!(strings(&choose(&e, Path::new(DIR)).unwrap())[0], "konsole");
    }

    #[test]
    fn then_xdg_terminal_exec_then_x_terminal_emulator_then_known_ones() {
        let path = bin(&["xdg-terminal-exec", "x-terminal-emulator", "xterm"]);
        let first = |p: &Path| choose(&env(p), Path::new(DIR)).map(|l| strings(&l)[0].clone());
        assert!(first(path.path()).unwrap().ends_with("xdg-terminal-exec"));
        std::fs::remove_file(path.path().join("xdg-terminal-exec")).unwrap();
        assert!(first(path.path()).unwrap().ends_with("x-terminal-emulator"));
        std::fs::remove_file(path.path().join("x-terminal-emulator")).unwrap();
        assert!(first(path.path()).unwrap().ends_with("xterm"));
        std::fs::remove_file(path.path().join("xterm")).unwrap();
        assert_eq!(first(path.path()), None);
    }

    #[test]
    fn an_alternatives_link_to_konsole_still_gets_its_folder_argument() {
        // Kubuntu: x-terminal-emulator -> konsole. Konsole's profile can set
        // its own start folder, so it needs --workdir.
        let real = bin(&["konsole"]);
        let path = bin(&[]);
        symlink(
            real.path().join("konsole"),
            path.path().join("x-terminal-emulator"),
        )
        .unwrap();
        let args = strings(&choose(&env(path.path()), Path::new(DIR)).unwrap());
        assert!(args[0].ends_with("x-terminal-emulator"));
        assert_eq!(&args[1..], ["--workdir", DIR]);
    }

    #[test]
    fn gnome_terminal_gets_its_folder_option() {
        let path = bin(&["gnome-terminal"]);
        let args = strings(&choose(&env(path.path()), Path::new(DIR)).unwrap());
        assert_eq!(&args[1..], [format!("--working-directory={DIR}")]);
    }

    #[test]
    fn an_empty_terminal_variable_is_ignored() {
        let path = bin(&["xterm"]);
        let e = Env {
            terminal: Some("  ".into()),
            ..env(path.path())
        };
        assert!(strings(&choose(&e, Path::new(DIR)).unwrap())[0].ends_with("xterm"));
    }

    #[test]
    fn a_program_that_is_not_executable_does_not_count() {
        let path = bin(&["xterm"]);
        let xterm = path.path().join("xterm");
        std::fs::set_permissions(&xterm, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(choose(&env(path.path()), Path::new(DIR)), None);
    }

    #[test]
    fn resolve_uses_a_given_command_then_macos_then_the_choice() {
        let empty = bin(&[]);
        let e = env(empty.path());
        let dir = Path::new(DIR);
        let given = resolve(Some("my-term -x"), &e, true, dir).unwrap();
        assert_eq!(strings(&given), ["my-term", "-x"]);
        let mac = resolve(None, &e, true, dir).unwrap();
        assert_eq!(strings(&mac), ["open", "-a", "Terminal", DIR]);
        let none = resolve(None, &e, false, dir).unwrap_err();
        assert_eq!(none.kind(), io::ErrorKind::NotFound);
        assert!(none.to_string().contains("$TERMINAL"));
        assert!(resolve(Some("'open"), &e, false, dir).is_err());
        assert!(resolve(Some("  "), &e, false, dir).is_err());
        let unbalanced = Env {
            terminal: Some("'kitty".into()),
            ..e.clone()
        };
        assert!(resolve(None, &unbalanced, false, dir).is_err());
    }

    #[test]
    fn open_terminal_starts_the_command_in_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        let folder = tmp.path().join("a folder");
        std::fs::create_dir(&folder).unwrap();
        let command = format!("sh -c 'pwd > \"{}\"'", out.display());
        open_terminal(Some(&command), &folder).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::fs::read_to_string(&out).map_or(true, |s| s.is_empty()) {
            assert!(std::time::Instant::now() < deadline, "did not run");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(&out).unwrap().trim(),
            folder.display().to_string()
        );
        let e = open_terminal(Some("no-such-terminal-here"), &folder).unwrap_err();
        assert!(
            e.to_string()
                .contains("cannot start “no-such-terminal-here”")
        );
    }
}
