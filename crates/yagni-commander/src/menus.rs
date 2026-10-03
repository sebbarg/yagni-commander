//! The menus, as data. One description feeds the native menu bar (macOS,
//! through `cx.set_menus`), the in-window menu bar (Linux, `menu_bar.rs`)
//! and, later, the right-click menu, both through [`popup`]. Items dispatch
//! the same actions as the keys, so menus show the keys next to them.

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{Action, App, Entity, FocusHandle, Menu, MenuItem, Window};
use yagni_commander_core::{Commander, SortKey};

use crate::actions::{
    About, CloseTab, Copy, CopyPath, Delete, DirectoryHotlist, Edit, EditNewFile, MakeDirectory,
    Move, NewTab, NextTab, OpenSettings, PrevTab, Quit, Reload, Rename, SelectAll, SortByModified,
    SortByName, SortByOwner, SortByPermissions, SortBySize, SwapPanels, SyncOtherPanel,
    ToggleHidden, Trash, View,
};

pub enum MenuEntry {
    /// `checked: None` draws no check mark at all.
    Item {
        label: &'static str,
        action: Box<dyn Action>,
        checked: Option<bool>,
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
            } => Self::Item {
                label,
                action: action.boxed_clone(),
                checked: *checked,
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
    }
}

fn check(label: &'static str, action: impl Action, checked: bool) -> MenuEntry {
    MenuEntry::Item {
        label,
        action: Box::new(action),
        checked: Some(checked),
    }
}

const ABOUT: &str = "About yagni-commander";
const SETTINGS: &str = "Settings...";

/// Every menu, in bar order. With `mac`, About and Quit go to the app menu
/// (the platform's convention) and there is no Help menu.
pub fn menus(state: MenuState, mac: bool) -> Vec<MenuDef> {
    let mut files = vec![
        item("View", View),
        item("Edit", Edit),
        item("New file", EditNewFile),
        item("Copy", Copy),
        item("Move", Move),
        item("New folder", MakeDirectory),
        item("Rename", Rename),
        item("Move to trash", Trash),
        item("Delete permanently", Delete),
        MenuEntry::Separator,
        item("Select all", SelectAll),
        item("Copy path", CopyPath),
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
    let mut defs = vec![
        MenuDef {
            title: "Files",
            entries: files,
        },
        MenuDef {
            title: "Commands",
            entries: vec![
                item("Same folder in other panel", SyncOtherPanel),
                item("Swap panels", SwapPanels),
                item("Reload", Reload),
                item("Directory hotlist", DirectoryHotlist),
                MenuEntry::Separator,
                item("New tab", NewTab),
                item("Close tab", CloseTab),
                item("Next tab", NextTab),
                item("Previous tab", PrevTab),
            ],
        },
        MenuDef {
            title: "Show",
            entries: show,
        },
    ];
    if mac {
        let app = MenuDef {
            title: "yagni-commander",
            entries: vec![
                item(ABOUT, About),
                MenuEntry::Separator,
                item(SETTINGS, OpenSettings),
                MenuEntry::Separator,
                item("Quit", Quit),
            ],
        };
        defs.insert(0, app);
    } else {
        defs.push(MenuDef {
            title: "Help",
            entries: vec![item(ABOUT, About)],
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
                MenuEntry::Item {
                    label,
                    action,
                    checked: None,
                } => menu.menu(label, action),
                MenuEntry::Item {
                    label,
                    action,
                    checked: Some(on),
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
        assert_eq!(titles, ["Files", "Commands", "Show", "Help"]);
        assert_eq!(
            labels(&defs[0]),
            [
                "View",
                "Edit",
                "New file",
                "Copy",
                "Move",
                "New folder",
                "Rename",
                "Move to trash",
                "Delete permanently",
                "-",
                "Select all",
                "Copy path",
                "-",
                "Settings...",
                "-",
                "Quit"
            ]
        );
        assert_eq!(
            labels(&defs[1]),
            [
                "Same folder in other panel",
                "Swap panels",
                "Reload",
                "Directory hotlist",
                "-",
                "New tab",
                "Close tab",
                "Next tab",
                "Previous tab"
            ]
        );
        assert_eq!(
            labels(&defs[2]),
            [
                "Hidden files",
                "-",
                "Sort by name",
                "Sort by size",
                "Sort by modified",
                "Sort by owner",
                "Sort by permissions"
            ]
        );
        assert_eq!(labels(&defs[3]), ["About yagni-commander"]);
    }

    #[test]
    fn macos_puts_about_and_quit_in_the_app_menu() {
        let defs = menus(STATE, true);
        let titles: Vec<_> = defs.iter().map(|d| d.title).collect();
        assert_eq!(titles, ["yagni-commander", "Files", "Commands", "Show"]);
        assert_eq!(
            labels(&defs[0]),
            ["About yagni-commander", "-", "Settings...", "-", "Quit"]
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
        assert!(action("Settings...").partial_eq(&crate::actions::OpenSettings));
    }

    #[test]
    fn gpui_menus_keep_titles_items_and_checks() {
        let state = MenuState {
            show_hidden: true,
            ..STATE
        };
        let gpui = to_gpui(&menus(state, false));
        assert_eq!(gpui[2].name.as_ref(), "Show");
        match &gpui[2].items[0] {
            gpui_kit::MenuItem::Action { name, checked, .. } => {
                assert_eq!(name.as_ref(), "Hidden files");
                assert!(*checked);
            }
            _ => panic!("expected an action"),
        }
        assert!(matches!(gpui[2].items[1], gpui_kit::MenuItem::Separator));
    }

    #[test]
    fn hidden_columns_have_no_sort_item() {
        let state = MenuState {
            columns: [false, true, false],
            ..STATE
        };
        let defs = menus(state, false);
        assert_eq!(
            labels(&defs[2]),
            [
                "Hidden files",
                "-",
                "Sort by name",
                "Sort by size",
                "Sort by owner"
            ]
        );
    }
}
