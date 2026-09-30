use fm_core::theme::{Rgb, TOKYO_NIGHT as P};
use fm_core::{COLUMNS, Command, Commander, Entry, EntryKind, Side, SortKey};
use gpui::{
    App, Bounds, Context, Div, FocusHandle, KeyBinding, Menu, MenuItem, MouseButton,
    MouseDownEvent, MouseMoveEvent, Rgba, ScrollStrategy, UniformListScrollHandle, Window,
    WindowBounds, WindowOptions, actions, div, prelude::*, px, relative, size, uniform_list,
};

const ROW_HEIGHT: f32 = 22.0;
const HEADER_HEIGHT: f32 = 28.0;
const FOOTER_HEIGHT: f32 = 24.0;
const COLUMN_HEADER_HEIGHT: f32 = 22.0;
const CELL_SPACING: f32 = 10.0;
const STATUS_HEIGHT: f32 = 22.0;
const PADDING: f32 = 6.0;
const DIVIDER_WIDTH: f32 = 6.0;
const CONTEXT: &str = "FileManager";

actions!(
    fm,
    [
        SwitchPanel,
        CursorUp,
        CursorDown,
        CursorHome,
        CursorEnd,
        PageUp,
        PageDown,
        Activate,
        GoUp,
        Quit
    ]
);

fn main() {
    let mut args = std::env::args().skip(1);
    let left = args.next().unwrap_or_else(|| ".".into());
    let right = args.next().unwrap_or_else(|| left.clone());
    let commander = Commander::new(&left, &right)
        .unwrap_or_else(|e| panic!("cannot open {left} / {right}: {e}"));

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    gpui_platform::application().run(move |cx: &mut App| {
        // gpui has no built-in quit: register it, bind it, and expose it in the menu bar.
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("alt-f4", Quit, None),
        ]);
        cx.set_menus([Menu::new("fm").items([MenuItem::action("Quit", Quit)])]);
        // Keep macOS from leaving a windowless process behind.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        // The keymap is data: this is where a user config file will plug in.
        cx.bind_keys([
            KeyBinding::new("tab", SwitchPanel, Some(CONTEXT)),
            KeyBinding::new("up", CursorUp, Some(CONTEXT)),
            KeyBinding::new("down", CursorDown, Some(CONTEXT)),
            KeyBinding::new("home", CursorHome, Some(CONTEXT)),
            KeyBinding::new("end", CursorEnd, Some(CONTEXT)),
            KeyBinding::new("pageup", PageUp, Some(CONTEXT)),
            KeyBinding::new("pagedown", PageDown, Some(CONTEXT)),
            KeyBinding::new("enter", Activate, Some(CONTEXT)),
            KeyBinding::new("backspace", GoUp, Some(CONTEXT)),
        ]);

        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| FileManager::new(commander, window, cx)),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}

struct FileManager {
    commander: Commander,
    focus: FocusHandle,
    scroll: [UniformListScrollHandle; 2],
    /// Left panel's share of the width, changed by dragging the divider.
    split_ratio: f32,
    dragging_split: bool,
}

impl FileManager {
    fn new(commander: Commander, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let this = Self {
            commander,
            focus,
            scroll: Default::default(),
            split_ratio: 0.5,
            dragging_split: false,
        };
        this.update_title(window);
        this
    }

    fn execute(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        // Opening files arrives with F4/editor support.
        let _ = self.commander.execute(command);
        let side = self.commander.active();
        let cursor = self.commander.panel(side).cursor();
        self.scroll[index(side)].scroll_to_item(cursor, ScrollStrategy::Nearest);
        self.update_title(window);
        cx.notify();
    }

    fn update_title(&self, window: &mut Window) {
        let path = self.commander.panel(self.commander.active()).path();
        window.set_window_title(&format!("{} - fm (gpui)", path.display()));
    }

    fn page(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let chrome =
            2.0 * PADDING + HEADER_HEIGHT + COLUMN_HEADER_HEIGHT + FOOTER_HEIGHT + STATUS_HEIGHT;
        let rows = ((f32::from(window.viewport_size().height) - chrome) / ROW_HEIGHT) as isize;
        let delta = (rows - 1).max(1);
        self.execute(
            Command::CursorBy(if down { delta } else { -delta }),
            window,
            cx,
        );
    }

    /// Clickable column titles; clicking sorts the panel by that column.
    fn column_headers(&self, side: Side, cx: &mut Context<Self>) -> Div {
        let sort = self.commander.panel(side).sort();
        div()
            .h(px(COLUMN_HEADER_HEIGHT))
            .flex_none()
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(CELL_SPACING))
            .border_b_1()
            .border_color(color(P.border))
            .text_size(px(12.0))
            .children(COLUMNS.iter().map(|column| {
                let key = column.key;
                cell(column)
                    .cursor_pointer()
                    .text_color(color(if sort.key == key { P.text } else { P.text_dim }))
                    .child(column.header(sort))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.execute(Command::SortBy(side, key), window, cx);
                        }),
                    )
            }))
    }

    fn panel(&self, side: Side, cx: &mut Context<Self>) -> Div {
        let panel = self.commander.panel(side);
        let is_active = self.commander.active() == side;

        let header = div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .px(px(10.0))
            .flex()
            .items_center()
            .bg(color(if is_active {
                P.header_active_bg
            } else {
                P.header_bg
            }))
            .text_color(color(if is_active { P.text } else { P.text_dim }))
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_start()
                    .child(panel.path().display().to_string()),
            );

        let list = uniform_list(
            ("entries", index(side)),
            panel.entries().len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let panel = this.commander.panel(side);
                let is_active = this.commander.active() == side;
                range
                    .map(|ix| {
                        let cursor = (ix == panel.cursor()).then_some(is_active);
                        entry_row(&panel.entries()[ix], cursor).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                this.execute(Command::CursorTo(side, ix), window, cx);
                                if event.click_count == 2 {
                                    this.execute(Command::Activate, window, cx);
                                }
                            }),
                        )
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll[index(side)])
        .flex_1();

        let dirs = panel
            .entries()
            .iter()
            .filter(|e| e.kind == EntryKind::Dir)
            .count();
        let files = panel
            .entries()
            .iter()
            .filter(|e| e.kind == EntryKind::File)
            .count();
        let footer = div()
            .h(px(FOOTER_HEIGHT))
            .flex_none()
            .px(px(10.0))
            .flex()
            .items_center()
            .bg(color(P.header_bg))
            .text_color(color(P.text_dim))
            .text_size(px(12.0))
            .child(format!("{dirs} dirs, {files} files"));

        div()
            .flex()
            .flex_col()
            .h_full()
            .min_w_0()
            .overflow_hidden()
            .rounded(px(6.0))
            .border_1()
            .border_color(color(if is_active { P.accent } else { P.border }))
            .bg(color(P.panel_bg))
            .child(header)
            .child(self.column_headers(side, cx))
            .child(list)
            .child(footer)
    }
}

impl Render for FileManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let divider = div()
            .id("divider")
            .w(px(DIVIDER_WIDTH))
            .h_full()
            .flex_none()
            .cursor_col_resize()
            .when(self.dragging_split, |d| d.bg(color(P.accent)))
            .hover(|d| d.bg(color(P.accent)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.dragging_split = true;
                    cx.notify();
                }),
            );

        let status = match self.commander.error() {
            Some(err) => div().text_color(color(P.error)).child(err.to_owned()),
            None => div()
                .text_color(color(P.text_dim))
                .child("Tab switch · ↑↓ move · Enter open · Backspace up"),
        };

        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, _: &SwitchPanel, w, cx| {
                    this.execute(Command::SwitchPanel, w, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &CursorUp, w, cx| this.execute(Command::CursorUp, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CursorDown, w, cx| this.execute(Command::CursorDown, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CursorHome, w, cx| this.execute(Command::CursorHome, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CursorEnd, w, cx| this.execute(Command::CursorEnd, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &Activate, w, cx| this.execute(Command::Activate, w, cx)),
            )
            .on_action(cx.listener(|this, _: &GoUp, w, cx| this.execute(Command::GoUp, w, cx)))
            .on_action(cx.listener(|this, _: &PageUp, w, cx| this.page(false, w, cx)))
            .on_action(cx.listener(|this, _: &PageDown, w, cx| this.page(true, w, cx)))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if !this.dragging_split {
                    return;
                }
                if event.pressed_button != Some(MouseButton::Left) {
                    this.dragging_split = false;
                } else {
                    let width = f32::from(window.viewport_size().width) - 2.0 * PADDING;
                    let x = f32::from(event.position.x) - PADDING - DIVIDER_WIDTH / 2.0;
                    this.split_ratio = (x / width).clamp(0.1, 0.9);
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging_split = false;
                    cx.notify();
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(color(P.window_bg))
            .text_color(color(P.text))
            .text_size(px(14.0))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .p(px(PADDING))
                    .child(
                        self.panel(Side::Left, cx)
                            .w(relative(self.split_ratio))
                            .flex_none(),
                    )
                    .child(divider)
                    .child(self.panel(Side::Right, cx).flex_1()),
            )
            .child(
                status
                    .h(px(STATUS_HEIGHT))
                    .flex_none()
                    .px(px(10.0))
                    .text_size(px(12.0)),
            )
    }
}

/// A table cell sized and aligned for `column`, shared by headers and rows.
fn cell(column: &fm_core::Column) -> Div {
    let cell = match column.width {
        Some(width) => div().w(px(width)).flex_none(),
        None => div().flex_1().min_w_0(),
    };
    cell.truncate().when(column.align_right, |d| d.text_right())
}

/// `cursor`: `None` if not under the cursor, `Some(panel_is_active)` otherwise.
fn entry_row(entry: &Entry, cursor: Option<bool>) -> Div {
    let (fg, bg) = match cursor {
        Some(true) => (P.text_on_accent, Some(P.accent)),
        Some(false) => (name_color(entry), Some(P.cursor_inactive_bg)),
        None => (name_color(entry), None),
    };
    let detail = if cursor == Some(true) { fg } else { P.text_dim };
    div()
        .w_full()
        .h(px(ROW_HEIGHT))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(CELL_SPACING))
        .when_some(bg, |d, bg| d.bg(color(bg)))
        .children(COLUMNS.iter().map(|column| {
            let is_name = column.key == SortKey::Name;
            cell(column)
                .when(!is_name, |d| d.text_size(px(13.0)))
                .text_color(color(if is_name { fg } else { detail }))
                .child(column.cell(entry))
        }))
}

fn name_color(entry: &Entry) -> Rgb {
    match entry.kind {
        EntryKind::Parent | EntryKind::Dir if entry.is_symlink => P.symlink,
        EntryKind::Parent | EntryKind::Dir => P.dir,
        EntryKind::File => P.text,
    }
}

fn index(side: Side) -> usize {
    match side {
        Side::Left => 0,
        Side::Right => 1,
    }
}

fn color(rgb: Rgb) -> Rgba {
    gpui::rgb(rgb.0)
}
