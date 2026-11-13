//! Num+ and Num-: a prompt for a mask (core `select`), then the matching
//! entries of the active panel are selected or deselected. The prompt
//! starts with the last mask used.

use std::io;
use std::rc::Rc;

use gpui_kit::{Context, Window};
use yagni_commander_core::select::SelectMask;

use super::FileManager;
use super::commands::{PROMPT_WIDTH, Prompt};

impl FileManager {
    /// Num+ (`select`) or Num-: asks for a mask, then applies it.
    pub(super) fn select_group(
        &mut self,
        select: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_loading(cx) {
            return;
        }
        self.end_search(cx);
        let (title, error_title) = if select {
            ("Select files (folders: */)", "Cannot select")
        } else {
            ("Deselect files (folders: */)", "Cannot deselect")
        };
        let initial = self.last_mask.clone();
        self.prompt_name(
            Prompt {
                title,
                error_title,
                initial: &initial,
                selection: 0..initial.len(),
                width: PROMPT_WIDTH,
            },
            Rc::new(move |this, typed, _, cx| {
                let mask = SelectMask::parse(typed).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "give a mask, such as *.txt")
                })?;
                this.last_mask = typed.trim().to_owned();
                this.notice = None;
                this.commander.update(cx, |c, cx| {
                    c.select_by_mask(&mask, select);
                    cx.notify();
                });
                Ok(())
            }),
            window,
            cx,
        );
    }
}
