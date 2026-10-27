//! macOS: Launch Services' applications for a file, through `NSWorkspace`
//! (macOS 12 and later). Started with `open -a`.

use std::io;
use std::path::{Path, PathBuf};

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSFileManager, NSString, NSURL};

use super::App;

pub(super) fn apps(file: &Path) -> io::Result<Vec<App>> {
    let text = file
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path is not UTF-8"))?;
    let workspace = NSWorkspace::sharedWorkspace();
    let url = NSURL::fileURLWithPath(&NSString::from_str(text));
    let default = workspace
        .URLForApplicationToOpenURL(&url)
        .and_then(|app| app.path())
        .map(|path| path.to_string());
    let manager = NSFileManager::defaultManager();
    let dir = file
        .parent()
        .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
    let apps = workspace
        .URLsForApplicationsToOpenURL(&url)
        .iter()
        .filter_map(|app| app.path().map(|path| path.to_string()))
        .map(|bundle| {
            // As Finder shows it: "Visual Studio Code", not the bundle's
            // file name.
            let name = manager
                .displayNameAtPath(&NSString::from_str(&bundle))
                .to_string();
            App {
                name: name.strip_suffix(".app").unwrap_or(&name).to_owned(),
                default: default.as_deref() == Some(bundle.as_str()),
                program: "open".into(),
                args: vec!["-a".into(), bundle.into(), file.as_os_str().to_owned()],
                dir: dir.clone(),
            }
        })
        .collect();
    Ok(apps)
}
