use super::{
    app::{App, Confirm, Pane, Screen},
    theme::Theme,
};
use crate::domain::ProblemStatus;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};

fn block<'a>(title: impl Into<Line<'a>>, theme: Theme, focused: bool) -> Block<'a> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(theme.border())
        .border_style(Style::default().fg(if focused { theme.accent } else { theme.muted }))
}

pub(super) fn draw(frame: &mut Frame<'_>, app: &App, now_ms: i64) {
    let area = frame.area();
    if area.width < 60 || area.height < 18 {
        frame.render_widget(Paragraph::new("Terminal too small.\nResize to at least 60x18 (80x24 recommended).\nq confirms leaving; Ctrl-C exits cleanly.")
            .wrap(Wrap { trim: false }), area);
        if app.confirm.is_some() {
            overlay(frame, app, "Leave contest? y/Enter: yes | n/Esc: cancel");
        }
        return;
    }
    let parts = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(if app.notice.is_empty() { 3 } else { 4 }),
    ])
    .split(area);
    header(frame, app, parts[0], now_ms);
    match app.base_screen() {
        Screen::Setup => setup(frame, app, parts[1]),
        Screen::Lobby => lobby(frame, app, parts[1]),
        Screen::Contest | Screen::Results => contest(frame, app, parts[1]),
        Screen::History | Screen::Review => frame.render_widget(Paragraph::new(
            "Use cfvirtual history or cfvirtual review for read-only saved records.\nAdd --json for structured output."
        ).block(block("Saved practice", app.theme, true)).wrap(Wrap { trim: false }), parts[1]),
        Screen::Help => {}
    }
    footer(frame, app, parts[2]);
    if app.paused {
        overlay(frame, app, "PAUSE OVERLAY\nThe authoritative clock, bots and polling KEEP RUNNING.\np / Enter / Esc resumes the view.");
    }
    if let Some(confirm) = app.confirm {
        overlay(frame, app, match confirm {
            Confirm::Abandon => "Abandon this contest? Unsaved progress will be lost.\ny / Enter: leave | n / Esc: cancel",
            Confirm::Leave => "Leave this room? The contest continues on the server.\nHost: use x to end for everyone instead.\ny / Enter: leave | n / Esc: cancel",
            Confirm::End => "End the contest early for EVERYONE?\nThe server will reconcile final verdicts before results.\ny / Enter: end | n / Esc: cancel",
        });
    }
    if app.screen == Screen::Help {
        help(frame, app);
    }
}

fn header(frame: &mut Frame<'_>, app: &App, area: Rect, now_ms: i64) {
    let mode = app
        .room
        .as_ref()
        .map(|r| format!("room {}", r.code))
        .unwrap_or_else(|| {
            app.contest
                .as_ref()
                .map_or("solo", |c| c.mode.as_str())
                .to_owned()
        });
    let timer = match app.base_screen() {
        Screen::Setup => format!("starts in {}", app.countdown),
        Screen::Lobby => "lobby".into(),
        Screen::Results => "FINAL RESULTS".into(),
        _ if app.complete => "saving results".into(),
        _ => format!("{} remaining", clock(app.remaining(now_ms))),
    };
    let score = app.own().map_or_else(
        || "spectator / awaiting scores".into(),
        |own| {
            format!(
                "{}/{} solved | penalty {}s",
                own.solved,
                app.contest.as_ref().map_or(0, |c| c.problems.len()),
                own.penalty
            )
        },
    );
    let connection = if !app.is_room() {
        "local"
    } else if app.connected {
        "connected"
    } else {
        "DISCONNECTED"
    };
    frame.render_widget(
        Paragraph::new(format!("{} | {score} | {connection}", app.handle)).block(block(
            format!(" CFVirtual | {mode} | {timer} "),
            app.theme,
            true,
        )),
        area,
    );
}

fn setup(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(contest) = &app.contest else {
        return;
    };
    let explanation = if contest.mode == "demo" {
        "Offline demo: synthetic submissions and BOT events, with no network requests."
    } else {
        "Public submissions are checked every 20 seconds.\nOffline: cached problems and bots work; live verdicts need connectivity.\nUnavailable preparation history falls back to uniform tag weights."
    };
    let text = format!(
        "Contest ready. Starting in {}...\n\n{} problems | {} | {} synthetic BOT opponents\n\nSubmit solutions manually in your browser.\n{explanation}\n\n? help | q cancel",
        app.countdown, contest.problems.len(), clock(contest.duration_sec), contest.bots.len()
    );
    frame.render_widget(
        Paragraph::new(text)
            .block(block("Setup / countdown", app.theme, true))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn lobby(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(room) = &app.room else {
        frame.render_widget(
            Paragraph::new(
                "Waiting for the server's welcome snapshot...\nr reconnect | ? help | q leave",
            )
            .block(block("Lobby", app.theme, true)),
            area,
        );
        return;
    };
    let mut lines = vec![
        Line::from(format!(
            "Room {} | {}",
            room.code,
            app.server_url.as_deref().unwrap_or("")
        )),
        Line::from(format!(
            "{} problems | {} | {} | {}",
            room.settings.problem_count,
            clock(room.settings.duration_sec),
            if room.settings.blind {
                "blind"
            } else {
                "revealed"
            },
            if room.settings.password_required {
                "password gate"
            } else {
                "open room"
            }
        )),
        Line::from(""),
    ];
    for p in &room.participants {
        lines.push(Line::from(format!(
            "[{}] {}{}{}",
            if p.connected { "+" } else { "-" },
            p.handle,
            if p.is_host { " HOST" } else { "" },
            if p.is_spectator { " SPECTATOR" } else { "" }
        )));
    }
    for bot in &room.bots {
        lines.push(Line::from(format!("[+] {} BOT ({})", bot.name, bot.rating)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(if app.is_host() {
        "Enter: Start | y: copy join command"
    } else {
        "Waiting for host Start | y: copy join command"
    }));
    lines.push(Line::from(app.selected_url().unwrap_or_default()));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block(
                "Lobby / roster (+ online, - offline)",
                app.theme,
                true,
            ))
            .scroll((app.detail_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn contest(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if area.width >= 100 {
        let columns = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(area);
        problems(frame, app, columns[0]);
        let right = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(columns[1]);
        standings(frame, app, right[0]);
        details(frame, app, right[1]);
    } else {
        // A full-width problem table remains useful at 60 columns. Secondary
        // panes stack below it; selecting Details replaces the standings pane.
        let count = app.contest.as_ref().map_or(0, |c| c.problems.len()) as u16;
        let top_height = count
            .saturating_add(3)
            .min(area.height.saturating_sub(5))
            .max(5);
        let rows =
            Layout::vertical([Constraint::Length(top_height), Constraint::Min(0)]).split(area);
        problems(frame, app, rows[0]);
        if app.pane == Pane::Details {
            details(frame, app, rows[1]);
        } else {
            standings(frame, app, rows[1]);
        }
    }
}

fn problems(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(contest) = &app.contest else {
        return;
    };
    let rows = contest.problems.iter().map(|p| {
        let status = app.own().and_then(|s| s.problems.get(&p.slot));
        let text = status_text(status);
        let style = Style::default().fg(if status.is_some_and(|s| s.solved) {
            app.theme.good
        } else if status.is_some_and(|s| s.rejected > 0) {
            app.theme.warning
        } else {
            app.theme.muted
        });
        Row::new(vec![
            Cell::from(p.slot.to_string()),
            Cell::from(format!("{} {}", p.problem.key, p.problem.name)),
            Cell::from(if app.hidden() {
                "hidden".into()
            } else {
                p.problem.rating.map_or("-".into(), |r| r.to_string())
            }),
            Cell::from(text).style(style),
            Cell::from(
                status
                    .and_then(|s| s.accepted_at_sec)
                    .map_or("-".into(), short_clock),
            ),
            Cell::from(status.map_or("-".into(), |s| s.attempts.to_string())),
        ])
    });
    let title = if app.base_screen() == Screen::Results {
        "Results / Problems (Enter: review hints)"
    } else {
        "Problems (Enter: details)"
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(9),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(5),
        ],
    )
    .header(
        Row::new(["#", "Problem", "Rating", "Status", "Time", "Tries"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(block(title, app.theme, app.pane == Pane::Problems))
    .row_highlight_style(app.theme.selected());
    let mut state = table_state(app.problem, area.height);
    frame.render_stateful_widget(table, area, &mut state);
}

fn standings(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let rows = app.standings.iter().enumerate().map(|(rank, s)| {
        Row::new(vec![
            Cell::from((rank + 1).to_string()),
            Cell::from(format!("{}{}", s.name, if s.is_bot { " BOT" } else { "" })),
            Cell::from(s.solved.to_string()),
            Cell::from(format!("{}s", s.penalty)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Min(15),
            Constraint::Length(6),
            Constraint::Length(10),
        ],
    )
    .header(Row::new(["#", "Player", "Solved", "Penalty"]))
    .block(block(
        "Standings (Tab cycles panes)",
        app.theme,
        app.pane == Pane::Standings,
    ))
    .row_highlight_style(if app.pane == Pane::Standings {
        app.theme.selected()
    } else {
        Style::default()
    });
    // While the problem pane is focused keep the leaderboard ordered from #1.
    let selected = if app.pane == Pane::Standings {
        app.standing
    } else {
        0
    };
    let mut state = table_state(selected, area.height);
    frame.render_stateful_widget(table, area, &mut state);
}

fn details(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(contest) = &app.contest else {
        return;
    };
    let Some(p) = contest.problems.get(app.problem) else {
        return;
    };
    let status = app.own().and_then(|s| s.problems.get(&p.slot));
    let mut lines = vec![
        Line::from(format!("{}: {}", p.slot, p.problem.name)),
        Line::from(if app.hidden() {
            "Rating / tags: hidden (blind contest)".into()
        } else {
            format!(
                "Rating: {} | Tags: {}",
                p.problem.rating.map_or("unrated".into(), |r| r.to_string()),
                p.problem.tags.join(", ")
            )
        }),
        Line::from(format!(
            "Status: {} | Tries: {}",
            status_text(status),
            status.map_or(0, |s| s.attempts)
        )),
    ];
    if app.complete && !app.hidden() {
        let baseline = app.own().map_or(contest.host_rating, |s| s.rating);
        let hint = match p.problem.rating {
            Some(r) if i64::from(r) <= i64::from(baseline) - 200 => "should have solved",
            Some(r) if i64::from(r) > i64::from(baseline) + 200 => "stretch",
            Some(_) => "solid / at level",
            None => "unrated",
        };
        lines.push(Line::from(format!(
            "Review: {hint}. {}",
            if status.is_some_and(|s| s.solved) {
                "Keep the technique."
            } else {
                "Revisit this problem."
            }
        )));
    }
    lines.push(Line::from(p.problem.url()));
    lines.push(Line::from("o / s open | y copy | b reveal | jk scroll"));
    if !app.notice.is_empty() {
        lines.push(Line::from(app.notice.as_str()));
    }
    lines.push(Line::from(app.status.as_str()));
    if app.complete {
        lines.push(Line::from("Problem links (j/k scroll):"));
        for problem in &contest.problems {
            lines.push(Line::from(format!(
                "{}: {}",
                problem.slot,
                problem.problem.url()
            )));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(block(
                "Details / manual submission",
                app.theme,
                app.pane == Pane::Details,
            ))
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
        area,
    );
}

fn footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let keys = if app.base_screen() == Screen::Results {
        if app.is_room() {
            "e export | v queue failed | r reconnect | b reveal | ? help | q quit"
        } else {
            "e export | r queue failed | b reveal | Tab panes | ? help | q quit"
        }
    } else if app.base_screen() == Screen::Lobby {
        "Enter host Start | y copy join | r reconnect | jk scroll | ? help | q leave"
    } else {
        "jk/arrows select | Tab panes | o open | y copy | r retry | ? help | q leave"
    };
    let mut text = Vec::new();
    if !app.notice.is_empty() {
        text.push(Line::from(Span::styled(
            app.notice.as_str(),
            Style::default().fg(app.theme.warning),
        )));
    }
    text.extend([
        Line::from(Span::styled(
            app.status.as_str(),
            Style::default().fg(app.theme.muted),
        )),
        Line::from(app.selected_url().unwrap_or_default()),
        Line::from(Span::styled(keys, Style::default().fg(app.theme.accent))),
    ]);
    frame.render_widget(Paragraph::new(text), area);
}

fn help(frame: &mut Frame<'_>, app: &App) {
    let text = [
        "KEYBOARD HELP (? / Enter / Esc closes; j/k scrolls)",
        "j/k or arrows  Select problems / standings; scroll details",
        "Tab/Shift-Tab  Cycle problems, standings, details",
        "Enter          Selected problem details / host Start in lobby",
        "o / s          Open problem page; submit manually in browser",
        "y              Copy URL (lobby: copy join command)",
        "b              Ratings/tags: locked during a blind contest",
        "               Reveal is allowed after final results",
        "p              Solo overlay ONLY: clock/bots/polling continue",
        "r              Solo: retry API; results: queue failed problems",
        "               Room: reconnect with the in-memory session",
        "e / v          Results: export Markdown / queue failed problems",
        "x              Host only: confirm End for everyone",
        "q / Esc        Confirm abandonment / leave room; results: exit",
        "Ctrl-C         Exit immediately, restoring the terminal",
        "Offline        Cached problems/bots work; live verdicts need CF",
        "Finalization   Two final polls; only in-window submissions count",
        "Saved data     cfvirtual history / cfvirtual review [--json]",
        "Room scores and clock are owned by the server, including bots.",
        "No submissions are automated; BOT opponents are synthetic.",
    ]
    .join("\n");
    frame.render_widget(Clear, frame.area());
    frame.render_widget(
        Paragraph::new(text)
            .block(block("Help", app.theme, true))
            .wrap(Wrap { trim: false })
            .scroll((app.help_scroll, 0)),
        frame.area(),
    );
}

fn overlay(frame: &mut Frame<'_>, app: &App, text: &str) {
    let area = frame.area();
    let width = area.width.min(70);
    let height = area.height.min(8);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(block(
            "Confirm / overlay",
            app.theme,
            true,
        )),
        popup,
    );
}

fn table_state(selected: usize, height: u16) -> TableState {
    let mut state = TableState::default().with_selected(Some(selected));
    *state.offset_mut() = selected.saturating_sub(usize::from(height.saturating_sub(4)));
    state
}

fn status_text(status: Option<&ProblemStatus>) -> &'static str {
    match status {
        Some(s) if s.solved => "AC",
        Some(s) if s.rejected > 0 => "WA(poll)",
        Some(_) => "unsolved",
        None => "unknown",
    }
}

fn clock(seconds: u32) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}
fn short_clock(seconds: u32) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{domain::scoring, tui::demo};
    use ratatui::{backend::TestBackend, Terminal};

    fn app(theme: &str, ascii: bool) -> App {
        let contest = demo::demo_contest(42);
        let mut app = App::new("demo_player".into(), theme, ascii);
        app.standings = scoring::score(
            &contest,
            &demo::submissions(&contest),
            contest.started_at + 65,
        );
        app.contest = Some(contest);
        app.screen = Screen::Contest;
        app.status = "Offline demo: no network".into();
        app
    }

    fn rendered(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
        let now = app
            .contest
            .as_ref()
            .map_or(0, |c| (c.started_at + 65) * 1000);
        terminal.draw(|frame| draw(frame, app, now)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn useful_at_80x24_and_degrades_to_60x18() {
        let app = app("default", false);
        for (width, height) in [(80, 24), (60, 18), (120, 35)] {
            let text = rendered(&app, width, height);
            for expected in [
                "CFVirtual",
                "Problem",
                "Status",
                "Standings",
                "BOT",
                "3/6 solved",
                "codeforces.com",
            ] {
                assert!(
                    text.contains(expected),
                    "missing {expected} at {width}x{height}: {text}"
                );
            }
        }
    }

    #[test]
    fn tiny_and_extreme_sizes_are_explicit_and_never_panic() {
        let app = app("default", false);
        for (width, height) in [(59, 18), (60, 17), (8, 2), (1, 1), (0, 0)] {
            let text = rendered(&app, width, height);
            if width >= 20 && height >= 3 {
                assert!(text.contains("Terminal too small"));
            }
        }
    }

    #[test]
    fn ascii_colorblind_blind_and_results_are_readable_without_color() {
        let mut app = app("colorblind", true);
        let text = rendered(&app, 80, 24);
        assert!(text.is_ascii());
        assert!(text.contains('+'));
        assert!(text.contains("AC"));
        assert!(text.contains("BOT"));
        app.contest.as_mut().expect("contest").blind = true;
        app.pane = Pane::Details;
        let hidden = rendered(&app, 80, 24);
        assert!(hidden.contains("hidden"));
        assert!(!hidden.contains("Tags: math"));
        assert!(!hidden.contains("800"));
        app.complete = true;
        app.screen = Screen::Results;
        app.reveal = true;
        let results = rendered(&app, 80, 24);
        assert!(results.contains("FINAL RESULTS"));
        assert!(results.contains("should have solved"));
        assert!(results.contains("Tags: math"));
    }

    #[test]
    fn pause_and_help_do_not_change_clock_or_allow_blind_reveal() {
        use crossterm::event::{KeyCode, KeyEvent};
        let mut app = app("default", true);
        app.contest.as_mut().expect("contest").blind = true;
        let now = app.contest.as_ref().expect("contest").started_at * 1000;
        app.key(KeyEvent::from(KeyCode::Char('b')));
        assert!(app.hidden());
        app.key(KeyEvent::from(KeyCode::Char('p')));
        assert!(rendered(&app, 80, 24).contains("KEEP RUNNING"));
        assert_eq!(app.remaining(now + 10_000), 80);
        app.key(KeyEvent::from(KeyCode::Char('p')));
        app.key(KeyEvent::from(KeyCode::Char('?')));
        assert!(rendered(&app, 80, 24).contains("clock/bots/polling continue"));
        app.set_screen(Screen::Results);
        app.key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Results);
    }
}
