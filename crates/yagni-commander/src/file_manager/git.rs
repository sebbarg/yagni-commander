//! The Git branch in each path header (see `SPEC.md`, Git; core `git`).
//! Nothing here sits between a key and the screen: a folder's listing
//! shows first, then a thread finds its repository (a few stats) and
//! another runs `git status` there, and the header gets the branch when
//! they report. Plain threads, as for directory reads: a dead mount may
//! hang either.
//!
//! A status is kept per repository and runs again only when asked: Ctrl-R,
//! the window becoming active (a commit in a terminal changes no file the
//! watcher sees), a watcher report for a shown folder, or the repository
//! shown anew. Moving between folders of one repository runs nothing.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use gpui_kit::{App, Context, SharedString};
use yagni_commander_core::Side;
use yagni_commander_core::git::{self, Status};

use super::FileManager;
use super::loads::rest;
use crate::config_state::CurrentConfig;

const FAST_POLL: Duration = Duration::from_millis(10);
const SLOW_POLL: Duration = Duration::from_millis(100);
/// Polling slows down once a status has run this long.
const SLOW_AFTER: Duration = Duration::from_millis(150);

#[derive(Default)]
pub(crate) struct Heads {
    /// Lookups and status runs on threads (the app), or right away
    /// (tests, like `FileManager::load`).
    threads: bool,
    sides: [Shown; 2],
    /// The repositories shown, by root.
    repos: HashMap<PathBuf, Repo>,
    lookups: Vec<(PathBuf, Receiver<Option<PathBuf>>)>,
    runs: Vec<(PathBuf, Receiver<io::Result<Status>>, Instant)>,
    polling: bool,
    /// Repositories whose status took longer than `git::TIMEOUT`: given
    /// up on until Ctrl-R.
    slow: HashSet<PathBuf>,
}

/// What a side's header is about: its folder, and the repository that
/// folder is in, once known.
#[derive(Default, PartialEq)]
struct Shown {
    dir: Option<PathBuf>,
    root: Option<PathBuf>,
}

struct Repo {
    status: Option<Status>,
    state: State,
    /// When the last run ended, and how long it took.
    last: Option<(Instant, Duration)>,
}

#[derive(PartialEq)]
enum State {
    Fresh,
    /// To run, once [`rest`] allows: a status that took long makes the
    /// next one of that repository wait as long again, so a busy folder's
    /// watcher reports can't keep git running back to back.
    Due,
    /// Asked again while running: runs once more when it ends.
    Running {
        again: bool,
    },
}

impl Heads {
    /// `threads`: run lookups and status on threads, else right away.
    pub(crate) fn new(threads: bool) -> Self {
        Self {
            threads,
            ..Default::default()
        }
    }
}

impl Repo {
    fn mark_stale(&mut self) {
        match &mut self.state {
            State::Fresh => self.state = State::Due,
            State::Due => {}
            State::Running { again } => *again = true,
        }
    }
}

impl FileManager {
    /// Follows the front tabs' folders: a side shown anew looks up its
    /// repository. Called from the commander observer and when the setting
    /// changes. Inside the old repository the old branch stays up until the
    /// lookup says otherwise, so moving around a repository doesn't blink.
    pub(super) fn sync_git(&mut self, cx: &mut Context<Self>) {
        let enabled = cx.global::<CurrentConfig>().config.git;
        let commander = self.commander.read(cx);
        let dirs = [Side::Left, Side::Right].map(|side| {
            let panel = commander.panel(side);
            (enabled && panel.is_loaded() && !panel.is_remote() && !panel.in_archive())
                .then(|| panel.real_dir().to_path_buf())
        });
        let mut lookups = Vec::new();
        for (shown, dir) in self.git.sides.iter_mut().zip(dirs) {
            if shown.dir == dir {
                continue;
            }
            let root = shown
                .root
                .take()
                .filter(|root| dir.as_ref().is_some_and(|dir| dir.starts_with(root)));
            *shown = Shown {
                dir: dir.clone(),
                root,
            };
            lookups.extend(dir);
        }
        for dir in lookups {
            self.look_up(dir, cx);
        }
        self.git_changed(cx);
    }

    /// Runs the shown repositories' status again (Ctrl-R, the window
    /// activated), or only for the sides showing `dir` (a watcher report).
    /// The folders are looked up again too: a `git init` or a removed
    /// `.git` shows.
    pub(super) fn refresh_git(&mut self, dir: Option<&Path>, cx: &mut Context<Self>) {
        let mut lookups = Vec::new();
        for shown in &self.git.sides {
            let Some(shown_dir) = &shown.dir else {
                continue;
            };
            if dir.is_some_and(|dir| dir != shown_dir) {
                continue;
            }
            if let Some(repo) = shown.root.as_ref().and_then(|r| self.git.repos.get_mut(r)) {
                repo.mark_stale();
            }
            if !lookups.contains(shown_dir) {
                lookups.push(shown_dir.clone());
            }
        }
        for dir in lookups {
            self.look_up(dir, cx);
        }
        self.git_changed(cx);
    }

    /// Ctrl-R: also tries the repositories given up on.
    pub(super) fn reload_git(&mut self, cx: &mut Context<Self>) {
        self.git.slow.clear();
        self.refresh_git(None, cx);
    }

    fn look_up(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if !self.git.threads {
            let root = git::find_root(&dir);
            self.found_root(&dir, root);
            return;
        }
        if self.git.lookups.iter().any(|(d, _)| *d == dir) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let looked_up = dir.clone();
        // If the thread can't start, the poll sees the channel closed.
        let _ = std::thread::Builder::new()
            .name("git-root".into())
            .spawn(move || {
                let _ = tx.send(git::find_root(&looked_up));
            });
        self.git.lookups.push((dir, rx));
        self.poll_git_soon(cx);
    }

    fn found_root(&mut self, dir: &Path, root: Option<PathBuf>) {
        for shown in &mut self.git.sides {
            if shown.dir.as_deref() == Some(dir) {
                shown.root = root.clone();
            }
        }
        if let Some(root) = root
            && !self.git.slow.contains(&root)
        {
            self.git.repos.entry(root).or_insert(Repo {
                status: None,
                state: State::Due,
                last: None,
            });
        }
    }

    /// Drops repositories no longer shown, starts the runs that are due
    /// and puts the labels in the headers.
    fn git_changed(&mut self, cx: &mut Context<Self>) {
        let sides = &self.git.sides;
        self.git
            .repos
            .retain(|root, _| sides.iter().any(|s| s.root.as_ref() == Some(root)));
        let now = cx.background_executor().now();
        let due: Vec<PathBuf> = self
            .git
            .repos
            .iter()
            .filter(|(_, repo)| repo.state == State::Due && rest(repo.last, now) <= now)
            .map(|(root, _)| root.clone())
            .collect();
        for root in due {
            if let Some(repo) = self.git.repos.get_mut(&root) {
                repo.state = State::Running { again: false };
            }
            if self.git.threads {
                let (tx, rx) = mpsc::channel();
                let dir = root.clone();
                let _ = std::thread::Builder::new()
                    .name("git-status".into())
                    .spawn(move || {
                        let _ = tx.send(git::status(&dir, git::TIMEOUT));
                    });
                self.git.runs.push((root, rx, now));
            } else {
                let result = git::status(&root, git::TIMEOUT);
                self.finished(&root, result, now, Duration::ZERO);
            }
        }
        if self.git.repos.values().any(|r| r.state == State::Due) {
            self.poll_git_soon(cx);
        }
        self.show_git(cx);
    }

    fn finished(&mut self, root: &Path, result: io::Result<Status>, now: Instant, took: Duration) {
        if result
            .as_ref()
            .is_err_and(|e| e.kind() == io::ErrorKind::TimedOut)
        {
            self.git.slow.insert(root.to_path_buf());
            self.git.repos.remove(root);
            return;
        }
        // Not shown any more: dropped.
        let Some(repo) = self.git.repos.get_mut(root) else {
            return;
        };
        // No git, not a repository after all, another user's: nothing.
        repo.status = result.ok();
        repo.last = Some((now, took));
        repo.state = match repo.state {
            State::Running { again: true } => State::Due,
            _ => State::Fresh,
        };
    }

    /// The status a side's header shows, if any.
    fn git_status(&self, side: Side) -> Option<&Status> {
        let root = self.git.sides[side as usize].root.as_ref()?;
        self.git.repos.get(root)?.status.as_ref()
    }

    /// What a side's header shows, the dot as `•` (tests).
    #[cfg(test)]
    pub(crate) fn git_label(&self, side: Side) -> Option<String> {
        let status = self.git_status(side)?;
        let dot = if status.dirty { " •" } else { "" };
        Some(format!("{}{dot}", status.label()))
    }

    fn show_git(&self, cx: &mut App) {
        for (side, view) in [(Side::Left, &self.left), (Side::Right, &self.right)] {
            let git = self
                .git_status(side)
                .map(|status| (SharedString::from(status.label()), status.dirty));
            view.update(cx, |view, cx| view.set_git(git, cx));
        }
    }

    fn poll_git_soon(&mut self, cx: &mut Context<Self>) {
        if self.git.polling {
            return;
        }
        self.git.polling = true;
        cx.spawn(async move |this, cx| {
            let mut wait = FAST_POLL;
            loop {
                cx.background_executor().timer(wait).await;
                match this.update(cx, |this, cx| this.poll_git(cx)) {
                    Ok(Some(next)) => wait = next,
                    _ => break,
                }
            }
        })
        .detach();
    }

    /// Takes the finished lookups and runs. Returns when to poll again, or
    /// `None` once nothing is pending.
    fn poll_git(&mut self, cx: &mut Context<Self>) -> Option<Duration> {
        let now = cx.background_executor().now();
        let mut found = Vec::new();
        self.git.lookups.retain(|(dir, rx)| match rx.try_recv() {
            Ok(root) => {
                found.push((dir.clone(), root));
                false
            }
            Err(TryRecvError::Disconnected) => {
                found.push((dir.clone(), None));
                false
            }
            Err(TryRecvError::Empty) => true,
        });
        let mut ended = Vec::new();
        self.git.runs.retain(|(root, rx, started)| {
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Disconnected) => Err(io::Error::other("git status stopped")),
                Err(TryRecvError::Empty) => return true,
            };
            ended.push((
                root.clone(),
                result,
                now.saturating_duration_since(*started),
            ));
            false
        });
        let news = !found.is_empty() || !ended.is_empty();
        for (dir, root) in found {
            self.found_root(&dir, root);
        }
        for (root, result, took) in ended {
            self.finished(&root, result, now, took);
        }
        if news {
            self.git_changed(cx);
        }
        let next = self.next_git_poll(now);
        self.git.polling = next.is_some();
        next
    }

    /// Fast while lookups or young runs are pending, slow for long runs,
    /// and for a run held back by [`rest`], never past its start.
    fn next_git_poll(&self, now: Instant) -> Option<Duration> {
        let lookups = (!self.git.lookups.is_empty()).then_some(FAST_POLL);
        let runs = self.git.runs.iter().map(|(_, _, started)| {
            if now.saturating_duration_since(*started) < SLOW_AFTER {
                FAST_POLL
            } else {
                SLOW_POLL
            }
        });
        let waiting = self
            .git
            .repos
            .values()
            .filter(|repo| repo.state == State::Due)
            .map(|repo| {
                rest(repo.last, now)
                    .saturating_duration_since(now)
                    .clamp(FAST_POLL, SLOW_POLL)
            });
        lookups.into_iter().chain(runs).chain(waiting).min()
    }
}
