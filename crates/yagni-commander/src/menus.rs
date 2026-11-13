//! The menus, as data. One description feeds the native menu bar (macOS,
//! through `cx.set_menus`), the in-window menu bar (Linux, `menu_bar.rs`)
//! and, later, the right-click menu, both through [`popup`]. Items dispatch
//! the same actions as the keys, so menus show the keys next to them.

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{
    Action, App, Entity, FocusHandle, InteractiveElement, Menu, MenuItem, ParentElement, Styled,
    Window, div, relative,
};
use yagni_commander_core::{Commander, SortKey};

use crate::actions::{
    About, Back, CheckForUpdates, CloseTab, CompareContents, Connect, Copy, CopyHere, CopyName,
    CopyParentPath, CopyPath, CountFolderSizes, Delete, DeselectAll, DeselectGroup,
    DeselectSameExtension, DirectoryHotlist, Edit, EditNewFile, Extract, FindFiles, FolderHistory,
    Forward, GoToConfig, GoToLog, InvertSelection, MakeDirectory, MountsLeft, MountsRight, Move,
    NewTab, NextTab, OpenSettings, OpenTerminal, OpenWith, Pack, PrevTab, Quit, Reload, Rename,
    SelectAll, SelectGroup, SelectSameExtension, ShowProperties, SortByModified, SortByName,
    SortByOwner, SortByPermissions, SortBySize, SwapPanels, SyncOtherPanel, ToggleHidden, Trash,
    UserCommands, View, ZoomIn, ZoomOut, ZoomReset,
};

pub enum MenuEntry {
    /// `checked: None` draws no check mark at all.
    Item {
        label: &'static str,
        action: Box<dyn Action>,
        checked: Option<bool>,
        /// The key as the in-window menu shows it, in place of gpui's name
        /// for it (the keypad's `add` would read "Add"; TC writes "Num +").
        keys: Option<&'static str>,
    },
    Separator,
}

impl Clone for MenuEntry {
    fn clone(&self) -> Self {
        match self {
            Self::Item {
                label,
                action,
                checked,
                keys,
            } => Self::Item {
                label,
                action: action.boxed_clone(),
                checked: *checked,
                keys: *keys,
            },
            Self::Separator => Self::Separator,
        }
    }
}

#[derive(Clone)]
pub struct MenuDef {
    pub title: &'static str,
    pub entries: Vec<MenuEntry>,
}

/// What the check marks show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuState {
    pub show_hidden: bool,
    /// The active panel's sort column.
    pub sort: SortKey,
    /// Whether the optional columns are shown: Modified, Owner, Permissions.
    pub columns: [bool; 3],
}

impl MenuState {
    pub fn of(commander: &Commander) -> Self {
        Self {
            show_hidden: commander.shows_hidden(),
            sort: commander.panel(commander.active()).sort().key,
            columns: OPTIONAL.map(|key| commander.shows_column(key)),
        }
    }

    fn shows(&self, key: SortKey) -> bool {
        OPTIONAL
            .iter()
            .position(|&k| k == key)
            .is_none_or(|i| self.columns[i])
    }
}

const OPTIONAL: [SortKey; 3] = [SortKey::Modified, SortKey::Owner, SortKey::Permissions];

fn item(label: &'static str, action: impl Action) -> MenuEntry {
    MenuEntry::Item {
        label,
        action: Box::new(action),
        checked: None,
        keys: None,
    }
}

/// An item on a keypad key, shown as TC writes it (`Num +`).
fn keypad(label: &'static str, action: impl Action, keys: &'static str) -> MenuEntry {
    MenuEntry::Item {
        label,
        action: Box::new(action),
        checked: None,
        keys: Some(keys),
    }
}

fn check(label: &'static str, action: impl Action, checked: bool) -> MenuEntry {
    MenuEntry::Item {
        label,
        action: Box::new(action),
        checked: Some(checked),
        keys: None,
    }
}

/// The menu bar's titles on Linux, in bar order. macOS adds the app menu
/// in front and has no Alt-letter keys.
pub const TITLES: [&str; 5] = ["Files", "Mark", "Commands", "Show", "Help"];

/// The letter that opens a menu with Alt, underlined in its title: the
/// first, lowercase.
pub fn letter(title: &str) -> char {
    title.chars().next().map_or(' ', |c| c.to_ascii_lowercase())
}

const ABOUT: &str = "About yagni-commander";
const CHECK_FOR_UPDATES: &str = "Check for updates...";
const SETTINGS: &str = "Settings...";

/// Help's own items, on both platforms.
fn go_to_items() -> [MenuEntry; 2] {
    [
        item("Go to config file", GoToConfig),
        item("Go to operation log", GoToLog),
    ]
}

/// Every menu, in bar order. With `mac`, About and Quit go to the app menu
/// (the platform's convention) and Help keeps only the go-to items.
pub fn menus(state: MenuState, mac: bool) -> Vec<MenuDef> {
    let mut files = vec![
        item("View", View),
        item("Edit", Edit),
        item("Open with...", OpenWith),
        item("New file", EditNewFile),
        item("Copy", Copy),
        item("Copy here...", CopyHere),
        item("Move", Move),
        item("New folder", MakeDirectory),
        item("Rename", Rename),
        item("Move to trash", Trash),
        item("Delete permanently", Delete),
        item("Pack...", Pack),
        item("Extract...", Extract),
        item("Compare by content", CompareContents),
        MenuEntry::Separator,
        item("Copy path", CopyPath),
        item("Copy name", CopyName),
        item("Copy parent path", CopyParentPath),
        item("Properties...", ShowProperties),
    ];
    if !mac {
        files.extend([
            MenuEntry::Separator,
            item(SETTINGS, OpenSettings),
            MenuEntry::Separator,
            item("Quit", Quit),
        ]);
    }
    let sorted = |key| state.sort == key;
    let mut show = vec![
        check("Hidden files", ToggleHidden, state.show_hidden),
        MenuEntry::Separator,
        check("Sort by name", SortByName, sorted(SortKey::Name)),
        check("Sort by size", SortBySize, sorted(SortKey::Size)),
    ];
    // Sort items only for the optional columns shown.
    show.extend(
        [
            state.shows(SortKey::Modified).then(|| {
                check(
                    "Sort by modified",
                    SortByModified,
                    sorted(SortKey::Modified),
                )
            }),
            state
                .shows(SortKey::Owner)
                .then(|| check("Sort by owner", SortByOwner, sorted(SortKey::Owner))),
            state.shows(SortKey::Permissions).then(|| {
                check(
                    "Sort by permissions",
                    SortByPermissions,
                    sorted(SortKey::Permissions),
                )
            }),
        ]
        .into_iter()
        .flatten(),
    );
    show.extend([
        MenuEntry::Separator,
        item("Zoom in", ZoomIn),
        item("Zoom out", ZoomOut),
        item("Actual size", ZoomReset),
    ]);
    let mut defs = vec![
        MenuDef {
            title: TITLES[0],
            entries: files,
        },
        // TC's Mark menu.
        MenuDef {
            title: TITLES[1],
            entries: vec![
                item("Select all", SelectAll),
                item("Deselect all", DeselectAll),
                keypad("Select group...", SelectGroup, "Num +"),
                keypad("Deselect group...", DeselectGroup, "Num -"),
                keypad("Invert selection", InvertSelection, "Num *"),
                keypad("Select same extension", SelectSameExtension, "Alt+Num +"),
                keypad(
                    "Deselect same extension",
                    DeselectSameExtension,
                    "Alt+Num -",
                ),
            ],
        },
        MenuDef {
            title: TITLES[2],
            entries: vec![
                item("Same folder in other panel", SyncOtherPanel),
                item("Swap panels", SwapPanels),
                item("Reload", Reload),
                item("Directory hotlist", DirectoryHotlist),
                item("Folder history", FolderHistory),
                item("Back", Back),
                item("Forward", Forward),
                item("Connect...", Connect),
                item("Mounts left", MountsLeft),
                item("Mounts right", MountsRight),
                item("Find files...", FindFiles),
                item("Count folder sizes", CountFolderSizes),
                item("Open terminal here", OpenTerminal),
                item("User commands", UserCommands),
                MenuEntry::Separator,
                item("New tab", NewTab),
                item("Close tab", CloseTab),
                item("Next tab", NextTab),
                item("Previous tab", PrevTab),
            ],
        },
        MenuDef {
            title: TITLES[3],
            entries: show,
        },
    ];
    if mac {
        let app = MenuDef {
            title: "yagni-commander",
            entries: vec![
                item(ABOUT, About),
                item(CHECK_FOR_UPDATES, CheckForUpdates),
                MenuEntry::Separator,
                item(SETTINGS, OpenSettings),
                MenuEntry::Separator,
                item("Quit", Quit),
            ],
        };
        defs.insert(0, app);
        defs.push(MenuDef {
            title: TITLES[4],
            entries: go_to_items().into(),
        });
    } else {
        let mut help = vec![
            item(ABOUT, About),
            item(CHECK_FOR_UPDATES, CheckForUpdates),
            MenuEntry::Separator,
        ];
        help.extend(go_to_items());
        defs.push(MenuDef {
            title: TITLES[4],
            entries: help,
        });
    }
    defs
}

/// The menus for `cx.set_menus`.
pub fn to_gpui(defs: &[MenuDef]) -> Vec<Menu> {
    defs.iter()
        .map(|def| {
            Menu::new(def.title).items(def.entries.iter().map(|entry| match entry {
                MenuEntry::Item {
                    label,
                    action,
                    checked,
                    ..
                } => MenuItem::Action {
                    name: (*label).into(),
                    action: action.boxed_clone(),
                    os_action: None,
                    checked: checked.unwrap_or(false),
                    disabled: false,
                },
                MenuEntry::Separator => MenuItem::Separator,
            }))
        })
        .collect()
}

/// A drawn popup of `entries`. Its items dispatch to `action_context` (the
/// file manager's focus), which also gets focus back when it closes. Used
/// by the Linux menu bar; meant for the right-click menu too.
pub fn popup(
    entries: &[MenuEntry],
    action_context: FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> Entity<PopupMenu> {
    let entries = entries.to_vec();
    PopupMenu::build(window, cx, move |mut menu, _, _| {
        for entry in entries {
            menu = match entry {
                // gpui-component draws no key for an element item: ours,
                // in its key's look (muted, extra small).
                MenuEntry::Item {
                    label,
                    action,
                    checked: None,
                    keys: Some(keys),
                } => menu.menu_element(action, move |_, cx| {
                    div()
                        .w_full()
                        .flex()
                        .gap_3()
                        .items_center()
                        .justify_between()
                        .child(label)
                        .child(
                            div()
                                .debug_selector(move || format!("menu-key-{keys}"))
                                .flex_shrink_0()
                                .text_xs()
                                .line_height(relative(1.))
                                .text_color(cx.theme().muted_foreground)
                                .child(keys),
                        )
                }),
                MenuEntry::Item {
                    label,
                    action,
                    checked: None,
                    ..
                } => menu.menu(label, action),
                MenuEntry::Item {
                    label,
                    action,
                    checked: Some(on),
                    ..
                } => menu.menu_with_check(label, on, action),
                MenuEntry::Separator => menu.separator(),
            };
        }
        menu.action_context(action_context)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(def: &MenuDef) -> Vec<&'static str> {
        def.entries
            .iter()
            .map(|e| match e {
                MenuEntry::Item { label, .. } => *label,
                MenuEntry::Separator => "-",
            })
            .collect()
    }

    fn checked(defs: &[MenuDef], label: &str) -> Option<bool> {
        defs.iter().flat_map(|d| &d.entries).find_map(|e| match e {
            MenuEntry::Item {
                label: l, checked, ..
            } if *l == label => *checked,
            _ => None,
        })
    }

    const STATE: MenuState = MenuState {
        show_hidden: false,
        sort: SortKey::Name,
        columns: [true; 3],
    };

    #[test]
    fn linux_menus_are_files_commands_show_help() {
        let defs = menus(STATE, false);
        let titles: Vec<_> = defs.iter().map(|d| d.title).collect();
        assert_eq!(titles, ["Files", "Mark", "Commands", "Show", "Help"]);
        assert_eq!(
            labels(&defs[0]),
            [
                "View",
                "Edit",
                "Open with...",
                "New file",
                "Copy",
                "Copy here...",
                "Move",
                "New folder",
                "Rename",
                "Move to trash",
                "Delete permanently",
                "Pack...",
                "Extract...",
                "Compare by content",
                "-",
                "Copy path",
                "Copy name",
                "Copy parent path",
                "Properties...",
                "-",
                "Settings...",
                "-",
                "Quit"
            ]
        );
        assert_eq!(
            labels(&defs[1]),
            [
                "Select all",
                "Deselect all",
                "Select group...",
                "Deselect group...",
                "Invert selection",
                "Select same extension",
                "Deselect same extension"
            ]
        );
        assert_eq!(
            labels(&defs[2]),
            [
                "Same folder in other panel",
                "Swap panels",
                "Reload",
                "Directory hotlist",
                "Folder history",
                "Back",
                "Forward",
                "Connect...",
                "Mounts left",
                "Mounts right",
                "Find files...",
                "Count folder sizes",
                "Open terminal here",
                "User commands",
                "-",
                "New tab",
                "Close tab",
                "Next tab",
                "Previous tab"
            ]
        );
        assert_eq!(
            labels(&defs[3]),
            [
                "Hidden files",
                "-",
                "Sort by name",
                "Sort by size",
                "Sort by modified",
                "Sort by owner",
                "Sort by permissions",
                "-",
                "Zoom in",
                "Zoom out",
                "Actual size"
            ]
        );
        assert_eq!(
            labels(&defs[4]),
            [
                "About yagni-commander",
                "Check for updates...",
                "-",
                "Go to config file",
                "Go to operation log"
            ]
        );
    }

    #[test]
    fn menu_letters_are_distinct() {
        let titles: Vec<_> = menus(STATE, false).iter().map(|d| d.title).collect();
        assert_eq!(titles, TITLES);
        let mut letters: Vec<_> = TITLES.iter().map(|t| letter(t)).collect();
        assert_eq!(letters, ['f', 'm', 'c', 's', 'h']);
        letters.sort();
        letters.dedup();
        assert_eq!(letters.len(), TITLES.len());
    }

    #[test]
    fn macos_puts_about_and_quit_in_the_app_menu() {
        let defs = menus(STATE, true);
        let titles: Vec<_> = defs.iter().map(|d| d.title).collect();
        assert_eq!(
            titles,
            [
                "yagni-commander",
                "Files",
                "Mark",
                "Commands",
                "Show",
                "Help"
            ]
        );
        assert_eq!(
            labels(&defs[5]),
            ["Go to config file", "Go to operation log"]
        );
        assert_eq!(
            labels(&defs[0]),
            [
                "About yagni-commander",
                "Check for updates...",
                "-",
                "Settings...",
                "-",
                "Quit"
            ]
        );
        assert!(!labels(&defs[1]).contains(&"Quit"));
    }

    #[test]
    fn checks_follow_hidden_files_and_the_sort_column() {
        let state = MenuState {
            show_hidden: true,
            sort: SortKey::Size,
            ..STATE
        };
        let defs = menus(state, false);
        assert_eq!(checked(&defs, "Hidden files"), Some(true));
        assert_eq!(checked(&defs, "Sort by size"), Some(true));
        assert_eq!(checked(&defs, "Sort by name"), Some(false));
        assert_eq!(checked(&defs, "Copy"), None);
        assert_eq!(checked(&menus(STATE, false), "Hidden files"), Some(false));
    }

    #[test]
    fn items_carry_their_actions() {
        let defs = menus(STATE, false);
        let action = |label: &str| {
            defs.iter()
                .flat_map(|d| &d.entries)
                .find_map(|e| match e {
                    MenuEntry::Item {
                        label: l, action, ..
                    } if *l == label => Some(action.boxed_clone()),
                    _ => None,
                })
                .unwrap()
        };
        assert!(action("Copy").partial_eq(&crate::actions::Copy));
        assert!(action("Sort by owner").partial_eq(&crate::actions::SortByOwner));
        assert!(action("Hidden files").partial_eq(&crate::actions::ToggleHidden));
        assert!(action("Zoom in").partial_eq(&crate::actions::ZoomIn));
        assert!(action("Zoom out").partial_eq(&crate::actions::ZoomOut));
        assert!(action("Actual size").partial_eq(&crate::actions::ZoomReset));
        assert!(action("Settings...").partial_eq(&crate::actions::OpenSettings));
    }

    #[test]
    fn gpui_menus_keep_titles_items_and_checks() {
        let state = MenuState {
            show_hidden: true,
            ..STATE
        };
        let gpui = to_gpui(&menus(state, false));
        assert_eq!(gpui[3].name.as_ref(), "Show");
        match &gpui[3].items[0] {
            gpui_kit::MenuItem::Action { name, checked, .. } => {
                assert_eq!(name.as_ref(), "Hidden files");
                assert!(*checked);
            }
            _ => panic!("expected an action"),
        }
        assert!(matches!(gpui[3].items[1], gpui_kit::MenuItem::Separator));
    }

    #[test]
    fn hidden_columns_have_no_sort_item() {
        let state = MenuState {
            columns: [false, true, false],
            ..STATE
        };
        let defs = menus(state, false);
        assert_eq!(
            labels(&defs[3]),
            [
                "Hidden files",
                "-",
                "Sort by name",
                "Sort by size",
                "Sort by owner",
                "-",
                "Zoom in",
                "Zoom out",
                "Actual size"
            ]
        );
    }

    /// gpui's native macOS menu shows an action's first binding when it is
    /// a single keystroke, and gets keys it can't express wrong (Shift-Enter
    /// as Shift-E). With the bindings registered in macOS's order, every
    /// item shows a key it can express, or none.
    #[gpui_kit::test]
    fn native_menu_shows_no_wrong_keys(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::actions::bind_keys_for(cx, true);
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            for def in menus(STATE, true) {
                for entry in def.entries {
                    let MenuEntry::Item { label, action, .. } = entry else {
                        continue;
                    };
                    let Some(first) = keymap.bindings_for_action(action.as_ref()).next() else {
                        continue;
                    };
                    if let [keystroke] = first.keystrokes() {
                        let key = &keystroke.inner().key;
                        assert!(
                            crate::actions::native_menu_key(key),
                            "{label}: the native menu can't show {key}"
                        );
                    }
                }
            }
        });
    }

    /// macOS: every Ctrl key also works with Cmd, and the native menu
    /// (the first binding) shows the Cmd one.
    #[gpui_kit::test]
    fn the_native_menu_shows_cmd_for_every_ctrl_key(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            crate::actions::bind_keys_for(cx, true);
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let single = |b: &gpui_kit::KeyBinding| match b.keystrokes() {
                [k] => Some(k.inner().clone()),
                _ => None,
            };
            for def in menus(STATE, true) {
                for entry in def.entries {
                    let MenuEntry::Item { label, action, .. } = entry else {
                        continue;
                    };
                    let keys: Vec<_> = keymap
                        .bindings_for_action(action.as_ref())
                        .filter_map(single)
                        .collect();
                    let has_ctrl = keys.iter().any(|k| k.modifiers.control && k.key != "tab");
                    if !has_ctrl {
                        continue;
                    }
                    let first = keymap
                        .bindings_for_action(action.as_ref())
                        .next()
                        .and_then(single)
                        .unwrap();
                    assert!(
                        first.modifiers.platform && !first.modifiers.control,
                        "{label}: the menu shows {}",
                        first.unparse()
                    );
                }
            }
            // Every Ctrl binding has its Cmd twin in the same context, but
            // Ctrl-Tab and Ctrl-Shift-Tab (Cmd-Tab is the system's).
            let all: Vec<_> = keymap.bindings().collect();
            for binding in &all {
                let Some(k) = single(binding) else { continue };
                if !k.modifiers.control {
                    continue;
                }
                let twin = all.iter().any(|other| {
                    single(other).is_some_and(|o| {
                        o.modifiers.platform
                            && !o.modifiers.control
                            && o.modifiers.shift == k.modifiers.shift
                            && o.modifiers.alt == k.modifiers.alt
                            && o.key == k.key
                    }) && other.action().partial_eq(binding.action())
                        && other.predicate() == binding.predicate()
                });
                assert_eq!(twin, k.key != "tab", "{}", k.unparse());
            }
        });
    }

    #[test]
    fn cmd_twins_only_on_macos_and_never_for_tab() {
        use crate::actions::{Connect, NextTab, Quit, with_cmd_twins};
        use gpui_kit::KeyBinding;
        let bindings = || {
            [
                KeyBinding::new("ctrl-f", Connect, None),
                KeyBinding::new("ctrl-tab", NextTab, None),
                KeyBinding::new("alt-f4", Quit, None),
                KeyBinding::new("ctrl-k ctrl-f", Connect, None),
            ]
        };
        assert_eq!(with_cmd_twins(bindings(), false).len(), 4);
        let mac = with_cmd_twins(bindings(), true);
        assert_eq!(mac.len(), 5, "one twin: Ctrl-F");
        let first = mac[0].keystrokes()[0].inner();
        assert!(first.modifiers.platform && !first.modifiers.control);
        assert_eq!(first.key, "f");
        assert!(mac[0].action().partial_eq(&Connect));
    }

    #[test]
    fn native_menu_keys_are_characters_and_the_keys_gpui_maps() {
        use crate::actions::native_menu_key;
        for key in [
            "a", "=", ",", "f1", "f12", "f35", "delete", "space", "pageup",
        ] {
            assert!(native_menu_key(key), "{key}");
        }
        for key in ["enter", "tab", "f0", "f36", "fn", "kp_enter"] {
            assert!(!native_menu_key(key), "{key}");
        }
    }

    /// gpui's `key_to_native` (gpui-pre-macos `events.rs`) lacks Enter and
    /// Tab, so `actions::hide_native_menu_keys` hides those items' keys in
    /// the macOS menu. Fails once the gpui in `Cargo.lock` maps both: then
    /// remove the workaround (and this test), and the menu shows the keys.
    #[test]
    fn gpui_still_cannot_show_enter_and_tab_in_the_native_menu() {
        let lock = include_str!("../../../Cargo.lock");
        let version = lock
            .split("[[package]]")
            .find(|p| p.contains("name = \"gpui-pre-macos\"\n"))
            .and_then(|p| p.lines().find_map(|l| l.strip_prefix("version = ")))
            .map(|v| v.trim_matches('"').to_owned())
            .expect("gpui-pre-macos in Cargo.lock");
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".cargo"))
            })
            .expect("a cargo home");
        let source = std::fs::read_dir(cargo_home.join("registry/src"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|index| {
                index
                    .path()
                    .join(format!("gpui-pre-macos-{version}/src/events.rs"))
            })
            .find_map(|path| std::fs::read_to_string(path).ok());
        let Some(source) = source else {
            // Not unpacked (an offline or vendored build): nothing to check.
            eprintln!("gpui-pre-macos {version} source not found; not checked");
            return;
        };
        // Renamed in 0.3.12.
        let table = ["fn key_to_native", "fn key_to_menu_equivalent"]
            .iter()
            .find_map(|name| source.split(name).nth(1))
            .and_then(|rest| rest.split("_ =>").next())
            .expect("the menu key table in gpui-pre-macos");
        let maps = |key: &str| table.contains(&format!("\"{key}\" =>"));
        assert!(
            !(maps("enter") && maps("tab")),
            "gpui-pre-macos {version} now shows Enter and Tab in the native menu: \
             remove actions::hide_native_menu_keys, its tests and the SPEC note"
        );
    }
}
