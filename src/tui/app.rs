use super::theme::Theme;
use crate::{
    domain::{Contest, Standing},
    server::proto::{Phase, Snapshot},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Setup,
    Lobby,
    Contest,
    Results,
    History,
    Review,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pane {
    Problems,
    Standings,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Confirm {
    Abandon,
    Leave,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    None,
    Quit,
    Retry,
    Start,
    End,
    Export,
    Queue,
    Open,
    Copy,
}

pub(super) struct App {
    pub screen: Screen,
    pub underneath: Screen,
    pub contest: Option<Contest>,
    pub standings: Vec<Standing>,
    pub room: Option<Snapshot>,
    pub server_url: Option<String>,
    pub handle: String,
    pub theme: Theme,
    pub pane: Pane,
    pub problem: usize,
    pub standing: usize,
    pub detail_scroll: u16,
    pub help_scroll: u16,
    pub clipboard: Option<arboard::Clipboard>,
    pub status: String,
    pub notice: String,
    pub connected: bool,
    pub reconnecting: bool,
    pub offset_ms: i64,
    pub clock_synced: bool,
    pub countdown: u64,
    pub paused: bool,
    pub confirm: Option<Confirm>,
    pub reveal: bool,
    pub complete: bool,
    pub saved: bool,
    pub saving: bool,
}

impl App {
    pub fn new(handle: String, theme: &str, ascii: bool) -> Self {
        Self {
            screen: Screen::Setup,
            underneath: Screen::Setup,
            contest: None,
            standings: Vec::new(),
            room: None,
            server_url: None,
            handle,
            theme: Theme::new(theme, ascii),
            pane: Pane::Problems,
            problem: 0,
            standing: 0,
            detail_scroll: 0,
            help_scroll: 0,
            clipboard: None,
            status: String::new(),
            notice: String::new(),
            connected: true,
            reconnecting: false,
            offset_ms: 0,
            clock_synced: false,
            countdown: 3,
            paused: false,
            confirm: None,
            reveal: false,
            complete: false,
            saved: false,
            saving: false,
        }
    }

    pub fn base_screen(&self) -> Screen {
        if self.screen == Screen::Help {
            self.underneath
        } else {
            self.screen
        }
    }

    pub fn set_screen(&mut self, screen: Screen) {
        if self.screen == Screen::Help {
            self.underneath = screen;
        } else {
            self.screen = screen;
        }
    }

    pub fn is_room(&self) -> bool {
        self.server_url.is_some()
    }

    pub fn is_host(&self) -> bool {
        self.room.as_ref().is_some_and(|state| {
            state.participants.iter().any(|p| {
                p.handle.eq_ignore_ascii_case(&self.handle) && p.is_host && !p.is_spectator
            })
        })
    }

    pub fn own(&self) -> Option<&Standing> {
        self.standings
            .iter()
            .find(|s| !s.is_bot && s.name.eq_ignore_ascii_case(&self.handle))
    }

    pub fn hidden(&self) -> bool {
        self.contest.as_ref().is_some_and(|c| c.blind) && !self.reveal
    }

    pub fn remaining(&self, now_ms: i64) -> u32 {
        let Some(contest) = &self.contest else {
            return 0;
        };
        let deadline = self
            .room
            .as_ref()
            .and_then(|r| r.deadline)
            .unwrap_or_else(|| contest.deadline());
        let remaining = deadline
            .saturating_mul(1000)
            .saturating_sub(now_ms.saturating_add(self.offset_ms));
        (remaining.saturating_add(999) / 1000).clamp(0, i64::from(contest.duration_sec)) as u32
    }

    pub fn selected_url(&self) -> Option<String> {
        if self.base_screen() == Screen::Lobby {
            let room = self.room.as_ref()?;
            return Some(format!(
                "cfvirtual join --server {} --code {} --handle <handle>",
                self.server_url.as_deref()?,
                room.code
            ));
        }
        Some(
            self.contest
                .as_ref()?
                .problems
                .get(self.problem)?
                .problem
                .url(),
        )
    }

    pub fn key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::Quit;
        }
        if let Some(confirm) = self.confirm {
            return match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.confirm = None;
                    if confirm == Confirm::End {
                        Action::End
                    } else {
                        Action::Quit
                    }
                }
                KeyCode::Char('n' | 'q') | KeyCode::Esc => {
                    self.confirm = None;
                    Action::None
                }
                _ => Action::None,
            };
        }
        if self.screen == Screen::Help {
            match key.code {
                KeyCode::Char('?' | 'q') | KeyCode::Esc | KeyCode::Enter => {
                    self.screen = self.underneath
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1).min(24)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                _ => {}
            }
            return Action::None;
        }
        if self.paused {
            if matches!(key.code, KeyCode::Char('p') | KeyCode::Esc | KeyCode::Enter) {
                self.paused = false;
            } else if key.code == KeyCode::Char('q') {
                self.confirm = Some(Confirm::Abandon);
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Char('?') => {
                self.underneath = self.screen;
                self.screen = Screen::Help;
                self.help_scroll = 0;
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.screen == Screen::Results {
                    return Action::Quit;
                }
                self.confirm = Some(if self.is_room() {
                    Confirm::Leave
                } else {
                    Confirm::Abandon
                });
            }
            KeyCode::Char('x')
                if self.is_host()
                    && self
                        .room
                        .as_ref()
                        .is_some_and(|r| r.phase == Phase::Running) =>
            {
                self.confirm = Some(Confirm::End);
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.pane = match (self.pane, key.code == KeyCode::BackTab) {
                    (Pane::Problems, false) | (Pane::Details, true) => Pane::Standings,
                    (Pane::Standings, false) | (Pane::Problems, true) => Pane::Details,
                    _ => Pane::Problems,
                };
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(true),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(false),
            KeyCode::Char('b') => {
                if self.complete || self.contest.as_ref().is_some_and(|c| !c.blind) {
                    self.reveal = !self.reveal;
                } else {
                    self.status =
                        "Blind contest: ratings and tags are locked until results.".into();
                }
            }
            KeyCode::Char('p')
                if !self.is_room() && self.screen == Screen::Contest && !self.complete =>
            {
                self.paused = true
            }
            KeyCode::Char('e') if self.screen == Screen::Results && self.saved => {
                return Action::Export
            }
            KeyCode::Char('r')
                if self.screen == Screen::Results && self.saved && !self.is_room() =>
            {
                return Action::Queue
            }
            KeyCode::Char('v') if self.screen == Screen::Results && self.saved => {
                return Action::Queue
            }
            KeyCode::Char('r') => return Action::Retry,
            KeyCode::Enter if self.screen == Screen::Lobby => {
                if self.is_host() && self.connected {
                    return Action::Start;
                }
                self.status =
                    "Waiting for the host to start (guests and spectators are read-only).".into();
            }
            KeyCode::Enter => {
                self.pane = Pane::Details;
                self.detail_scroll = 0;
            }
            KeyCode::Char('o' | 's') if self.screen != Screen::Lobby => return Action::Open,
            KeyCode::Char('y') => return Action::Copy,
            _ => {}
        }
        Action::None
    }

    fn move_selection(&mut self, down: bool) {
        if self.screen == Screen::Lobby {
            self.detail_scroll = if down {
                self.detail_scroll.saturating_add(1)
            } else {
                self.detail_scroll.saturating_sub(1)
            };
            return;
        }
        let (selected, count) = match self.pane {
            Pane::Problems => (
                &mut self.problem,
                self.contest.as_ref().map_or(0, |c| c.problems.len()),
            ),
            Pane::Standings => (&mut self.standing, self.standings.len()),
            Pane::Details => {
                self.detail_scroll = if down {
                    self.detail_scroll.saturating_add(1)
                } else {
                    self.detail_scroll.saturating_sub(1)
                };
                return;
            }
        };
        *selected = if down {
            selected.saturating_add(1).min(count.saturating_sub(1))
        } else {
            selected.saturating_sub(1)
        };
        self.detail_scroll = 0;
    }
}
