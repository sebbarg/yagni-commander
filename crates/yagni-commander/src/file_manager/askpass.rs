//! ssh's questions (core `askpass`): a password, passphrase or code in a
//! masked field, or Yes/No for an unknown host key. Each is answered
//! exactly once; closing the dialog any other way cancels it.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};

use gpui_kit::component::WindowExt;
use gpui_kit::component::input::InputState;
use gpui_kit::{
    App, AppContext, Context, Focusable, InteractiveElement, ParentElement, SharedString,
    WeakEntity, Window, div,
};
use yagni_commander_core::askpass::{Kind, Question};

use super::FileManager;
use super::commands::{OPENER_POLL, dialog_password_field, focus_when_open, themed_dialog};
use crate::button_row::{ButtonRow, OnPress};

/// The question, until it is answered.
/// The question, until it is answered, and who counts open questions.
struct Asked {
    question: RefCell<Option<Question>>,
    manager: WeakEntity<FileManager>,
}

type Pending = Rc<Asked>;

fn answer(pending: &Pending, text: Option<String>, cx: &mut App) {
    if let Some(question) = pending.question.borrow_mut().take() {
        question.answer(text);
        let _ = pending
            .manager
            .update(cx, |fm, _| fm.asking = fm.asking.saturating_sub(1));
    }
}

impl FileManager {
    /// Shows each question that arrives on `questions` (polled every
    /// [`OPENER_POLL`]: a wake-up from the server's thread would upset
    /// gpui's test scheduler).
    pub(crate) fn ask_from(
        &mut self,
        questions: Receiver<Question>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(OPENER_POLL).await;
                match questions.try_recv() {
                    Ok(question) => {
                        let shown =
                            this.update_in(cx, |this, window, cx| this.ask(question, window, cx));
                        if shown.is_err() {
                            break;
                        }
                    }
                    Err(TryRecvError::Disconnected) => break,
                    Err(TryRecvError::Empty) => {}
                }
            }
        })
        .detach();
    }

    fn ask(&mut self, question: Question, window: &mut Window, cx: &mut Context<Self>) {
        let text = SharedString::from(question.prompt.trim().to_owned());
        let kind = question.kind;
        let pending: Pending = Rc::new(Asked {
            question: RefCell::new(Some(question)),
            manager: cx.entity().downgrade(),
        });
        // Jobs hold their dialogs back meanwhile (see `poll_job`).
        self.asking += 1;
        let focus = self.focus.clone();
        // After the dialog closes, focus goes back to the panels.
        let refocus = move |window: &mut Window, cx: &mut App| {
            let focus = focus.clone();
            window.defer(cx, move |window, cx| focus.focus(window, cx));
        };
        let press = |reply: Option<&'static str>| -> OnPress {
            let (pending, refocus) = (pending.clone(), refocus.clone());
            Rc::new(move |window, cx| {
                answer(&pending, reply.map(str::to_owned), cx);
                window.close_dialog(cx);
                refocus(window, cx);
            })
        };
        if kind == Kind::Confirm {
            // No is the default: accepting a host key should be a choice.
            let buttons =
                ButtonRow::build([("Yes", press(Some("yes"))), ("No", press(None))], 1, cx);
            focus_when_open(buttons.focus_handle(cx), window, cx);
            window.open_dialog(cx, move |dialog, _, cx| {
                let (pending, refocus) = (pending.clone(), refocus.clone());
                themed_dialog(dialog, cx)
                    .title("ssh")
                    .w(crate::zoom::dialog_width(560.0, cx))
                    .close_button(false)
                    .overlay_closable(false)
                    .child(
                        div()
                            .debug_selector(|| "ssh-question".into())
                            .child(text.clone()),
                    )
                    .footer(buttons.clone())
                    .on_cancel(move |_, window, cx| {
                        answer(&pending, None, cx);
                        refocus(window, cx);
                        true
                    })
            });
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).masked(true));
        let submit: OnPress = Rc::new({
            let (pending, input, refocus) = (pending.clone(), input.clone(), refocus.clone());
            move |window, cx| {
                let typed = input.read(cx).value().to_string();
                answer(&pending, Some(typed), cx);
                refocus(window, cx);
            }
        });
        let ok: OnPress = Rc::new({
            let submit = submit.clone();
            move |window, cx| {
                submit(window, cx);
                window.close_dialog(cx);
            }
        });
        let buttons = ButtonRow::build([("OK", ok), ("Cancel", press(None))], 0, cx);
        window.defer(cx, {
            let input = input.clone();
            move |window, cx| input.update(cx, |state, cx| state.focus(window, cx))
        });
        window.open_dialog(cx, move |dialog, _, cx| {
            let (pending, refocus, submit) = (pending.clone(), refocus.clone(), submit.clone());
            themed_dialog(dialog, cx)
                .title("ssh")
                .w(crate::zoom::dialog_width(480.0, cx))
                .close_button(false)
                .overlay_closable(false)
                .child(
                    div()
                        .debug_selector(|| "ssh-question".into())
                        .child(text.clone()),
                )
                .child(dialog_password_field(&input))
                .footer(buttons.clone())
                // Enter in the field.
                .on_ok(move |_, window, cx| {
                    submit(window, cx);
                    true
                })
                .on_cancel(move |_, window, cx| {
                    answer(&pending, None, cx);
                    refocus(window, cx);
                    true
                })
        });
    }
}
