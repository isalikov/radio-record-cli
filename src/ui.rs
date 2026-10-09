use crate::{
    account::{Choice, merge},
    app::{App, Category},
    player::{PlayerState, STREAM_ROUNDS},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph},
};
use std::{collections::VecDeque, time::Duration};

/// Color palette for the whole UI. `accent` marks playback and live audio,
/// `alert` marks the selected category, favorites, and errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    bg: Color,
    fg: Color,
    muted: Color,
    edge: Color,
    accent: Color,
    alert: Color,
    selected: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: Color::Rgb(10, 12, 20),
            fg: Color::Rgb(216, 222, 239),
            muted: Color::Rgb(111, 121, 149),
            edge: Color::Rgb(39, 46, 65),
            accent: Color::Rgb(53, 235, 221),
            alert: Color::Rgb(244, 94, 191),
            selected: Color::Rgb(40, 27, 54),
        }
    }
}

impl Theme {
    /// Monochrome amber CRT.
    fn amber() -> Self {
        Self {
            bg: Color::Rgb(24, 14, 0),
            fg: Color::Rgb(255, 176, 0),
            muted: Color::Rgb(168, 110, 24),
            edge: Color::Rgb(94, 58, 8),
            accent: Color::Rgb(255, 213, 79),
            alert: Color::Rgb(255, 236, 189),
            selected: Color::Rgb(66, 38, 0),
        }
    }

    /// Green phosphor terminal.
    fn phosphor() -> Self {
        Self {
            bg: Color::Rgb(2, 12, 5),
            fg: Color::Rgb(72, 240, 120),
            muted: Color::Rgb(48, 150, 74),
            edge: Color::Rgb(18, 70, 32),
            accent: Color::Rgb(150, 255, 180),
            alert: Color::Rgb(200, 255, 214),
            selected: Color::Rgb(8, 52, 20),
        }
    }

    /// Light theme for paper-white terminals.
    fn paper() -> Self {
        Self {
            bg: Color::Rgb(250, 249, 244),
            fg: Color::Rgb(52, 49, 44),
            muted: Color::Rgb(132, 127, 117),
            edge: Color::Rgb(213, 208, 196),
            accent: Color::Rgb(0, 128, 128),
            alert: Color::Rgb(196, 22, 118),
            selected: Color::Rgb(228, 225, 242),
        }
    }

    /// Terminal defaults, used when `NO_COLOR` is set.
    fn plain() -> Self {
        Self {
            bg: Color::Reset,
            fg: Color::Reset,
            muted: Color::Reset,
            edge: Color::Reset,
            accent: Color::Reset,
            alert: Color::Reset,
            selected: Color::Reset,
        }
    }

    /// Theme names accepted by `RADIO_RECORD_THEME`.
    pub fn known() -> &'static [&'static str] {
        &["default", "amber", "phosphor", "paper"]
    }

    /// Map a `RADIO_RECORD_THEME` value (and the `NO_COLOR` flag) to a palette.
    /// `NO_COLOR` wins; unknown and empty names fall back to the default theme.
    pub fn resolve(no_color: bool, name: Option<&str>) -> Self {
        if no_color {
            return Self::plain();
        }
        match name.map(str::trim).filter(|name| !name.is_empty()) {
            Some("amber") => Self::amber(),
            Some("phosphor") => Self::phosphor(),
            Some("paper") => Self::paper(),
            _ => Self::default(),
        }
    }
}

/// Resolve the startup theme from `RADIO_RECORD_THEME` and `NO_COLOR`. An unknown
/// name falls back to the default theme with a note on stderr. Called once at
/// startup, before the terminal takes over the screen.
pub fn theme_from_env() -> Theme {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    let name = std::env::var("RADIO_RECORD_THEME").unwrap_or_default();
    let name = name.trim();
    if !no_color && !name.is_empty() && !Theme::known().contains(&name) {
        eprintln!("radio-record: unknown RADIO_RECORD_THEME '{name}', using default");
    }
    Theme::resolve(no_color, Some(name))
}

pub fn render(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.bg).fg(theme.fg)),
        area,
    );
    if area.width < 44 || area.height < 12 {
        frame.render_widget(
            Paragraph::new("Minimum 44 × 12\nq  quit").style(Style::default().fg(theme.muted)),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    let [body, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(4)])
        .areas(area.inner(Margin::new(2, 1)));
    let category_width = if area.width >= 90 { 25 } else { 18 };
    let [sidebar, right] =
        Layout::horizontal([Constraint::Length(category_width), Constraint::Min(1)]).areas(body);
    categories(frame, app, sidebar, theme);
    let right = right.inner(Margin::new(1, 0));
    if app.show_history && body.height >= 12 {
        let [stations_area, history_area] = Layout::vertical([
            Constraint::Min(6),
            Constraint::Length((body.height / 3).clamp(5, 8)),
        ])
        .areas(right);
        stations(frame, app, stations_area, theme);
        history(frame, app, history_area, theme);
    } else {
        stations(frame, app, right, theme);
    }
    playback(frame, app, footer, theme);
    if app.help {
        help(frame, area, app.help_scroll, theme);
    }
    if app.show_diag {
        diag(frame, area, app, theme);
    }
    if app.account.open {
        account(frame, area, app, theme);
    }
}

fn panel<'a>(title: &'a str, theme: &Theme) -> Block<'a> {
    Block::default()
        .title(Line::from(Span::styled(
            title,
            Style::default().fg(theme.muted),
        )))
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.edge))
        .padding(Padding::new(0, 0, 0, 0))
}

fn categories(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let [area, hint] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let label = app
        .account
        .session
        .as_ref()
        .and(app.account.profile.as_ref())
        .map(|profile| profile.email.as_str())
        .filter(|email| !email.is_empty())
        .unwrap_or("Account");
    let suffix = if app.account.conflict.is_some() || app.account.error.is_some() {
        " !"
    } else if app.account.busy {
        " …"
    } else {
        ""
    };
    let state = format!("a {label}{suffix}");
    frame.render_widget(
        Paragraph::new(state).style(Style::default().fg(theme.muted)),
        hint,
    );
    let block = panel("", theme)
        .borders(Borders::RIGHT)
        .padding(Padding::new(0, 1, 0, 0));
    let items: Vec<_> = app
        .categories
        .iter()
        .map(|category| {
            let line = Line::raw(category.title().to_owned());
            if matches!(category, Category::Favorites) {
                ListItem::new(vec![line, Line::raw("")])
            } else {
                ListItem::new(line)
            }
        })
        .collect();
    let selected = Style::default()
        .fg(theme.alert)
        .add_modifier(Modifier::BOLD);
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(selected)
            .highlight_symbol("▏"),
        area,
        &mut app.category,
    );
}

fn stations(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let block = panel("", theme).borders(Borders::NONE);
    let inner = block.inner(area);
    let visible = app.visible();
    if visible.is_empty() {
        let text = if app.loading {
            "Loading…"
        } else if app.error.is_some() && app.catalog.is_none() {
            "Stations unavailable · r retry"
        } else if matches!(app.current_category(), Category::Favorites) {
            "No favorites · f to add"
        } else {
            "No stations"
        };
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(theme.muted)),
            inner,
        );
        return;
    }
    let items: Vec<_> = visible
        .iter()
        .map(|station| {
            let is_current = app.now.as_ref().is_some_and(|now| now.id == station.id)
                && !matches!(app.playback.state, PlayerState::Idle | PlayerState::Error);
            let marker = if is_current {
                match app.playback.state {
                    PlayerState::Paused => "Ⅱ ",
                    PlayerState::Buffering => "· ",
                    _ => "▶ ",
                }
            } else {
                "  "
            };
            let line = Line::from(vec![
                Span::styled(marker, Style::default().fg(theme.accent)),
                Span::styled(
                    station.title.clone(),
                    if is_current {
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    },
                ),
                Span::styled(
                    if app.settings.favorites.contains(&station.id)
                        && !matches!(app.current_category(), Category::Favorites)
                    {
                        "  +"
                    } else {
                        ""
                    },
                    Style::default().fg(theme.alert),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(Style::default().bg(theme.selected))
            .highlight_symbol("▏"),
        area,
        &mut app.stations,
    );
}

fn history(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let block = panel("History ", theme);
    let inner = block.inner(area);
    if app.history.len() <= 1 {
        let text = if let Some(error) = &app.history_error {
            error.as_str()
        } else if app.history_loading {
            "Loading…"
        } else if app.now.is_none() {
            ""
        } else {
            "No track history"
        };
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(theme.muted)),
            inner,
        );
        return;
    }
    let items: Vec<_> = app
        .history
        .iter()
        .skip(1)
        .map(|track| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{}  ", track.time_formatted),
                    Style::default().fg(theme.muted),
                ),
                Span::styled(track.artist.clone(), Style::default().fg(theme.fg)),
                Span::styled(
                    format!(" — {}", track.song),
                    Style::default().fg(theme.muted),
                ),
            ]))
        })
        .collect();
    frame.render_widget(List::new(items).block(block), area);
    if app.history_error.is_some() && inner.height > 0 {
        frame.render_widget(
            Paragraph::new("History out of date · r retry").style(Style::default().fg(theme.alert)),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}

fn playback(frame: &mut Frame, app: &App, mut area: Rect, theme: &Theme) {
    let playing = app.playback.state == PlayerState::Playing;
    // One bar, riding the louder channel; mono sources mirror the left one.
    let level = if playing {
        app.playback.levels[0].max(app.playback.levels[1])
    } else {
        0.0
    };
    frame.render_widget(
        Paragraph::new(meter(area.width, level, theme)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    area.y += 1;
    let area = Rect::new(area.x, area.y, area.width, area.height - 1);
    let [status, volume] = Layout::horizontal([Constraint::Min(1), Constraint::Length(5)])
        .areas(Rect::new(area.x, area.y, area.width, 1));
    let symbol = match app.playback.state {
        PlayerState::Idle => "○".to_owned(),
        // Real cache progress while the stream fills; a bare dot until the engine reports progress.
        PlayerState::Buffering => match app.playback.buffering {
            Some(percent) => format!("· {percent}%"),
            None => "·".to_owned(),
        },
        PlayerState::Playing => "▶".to_owned(),
        PlayerState::Paused => "Ⅱ".to_owned(),
        PlayerState::Error => "!".to_owned(),
    };
    let name = app
        .now
        .as_ref()
        .map(|station| station.title.as_str())
        .unwrap_or("Stopped");
    let mut spans = vec![Span::styled(
        format!("{symbol}  {name}"),
        Style::default().fg(theme.accent),
    )];
    if app.show_diag {
        for segment in telemetry_segments(app) {
            spans.push(Span::styled(
                format!(" · {segment}"),
                Style::default().fg(theme.muted),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), status);
    frame.render_widget(
        Paragraph::new(format!("{}%", app.settings.volume))
            .right_aligned()
            .style(Style::default().fg(theme.muted)),
        volume,
    );
    let track = if let Some(error) = &app.playback.error {
        error.clone()
    } else if let Some(track) = app.history.first() {
        format!("{} — {}", track.artist, track.song)
    } else if app.history_loading {
        "Loading track…".into()
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(track).style(Style::default().fg(if app.playback.error.is_some() {
            theme.alert
        } else {
            theme.muted
        })),
        Rect::new(area.x, area.y + 1, area.width, 1),
    );
    let status = app.error.as_deref().unwrap_or("");
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(theme.alert)),
        Rect::new(area.x, area.y + 2, area.width, 1),
    );
}

/// Segments shown after the station name while the diagnostics overlay is open.
/// Every value comes from the player snapshot; absent data simply drops out.
fn telemetry_segments(app: &App) -> Vec<String> {
    let playback = &app.playback;
    let mut segments = Vec::new();
    if let Some(codec) = &playback.codec {
        segments.push(codec.clone());
    }
    match (playback.samplerate, playback.channels) {
        (Some(rate), Some(channels)) => segments.push(format!(
            "{} {}",
            samplerate_text(rate),
            channels_text(channels)
        )),
        (Some(rate), None) => segments.push(samplerate_text(rate)),
        (None, Some(channels)) => segments.push(channels_text(channels)),
        (None, None) => {}
    }
    if let Some(bitrate) = playback.bitrate.filter(|bitrate| *bitrate >= 1000) {
        segments.push(format!("{}k", bitrate / 1000));
    }
    segments
}

fn samplerate_text(rate: u64) -> String {
    if rate.is_multiple_of(1000) {
        format!("{}kHz", rate / 1000)
    } else {
        format!("{:.1}kHz", rate as f64 / 1000.0)
    }
}

fn channels_text(channels: u64) -> String {
    match channels {
        1 => "mono".into(),
        2 => "stereo".into(),
        other => format!("{other}ch"),
    }
}

/// Names the URL that is actually playing, using the station's own stream fields
/// and falling back to the host when the URL is not one of them.
fn stream_label(app: &App, url: &str) -> String {
    if let Some(station) = &app.now {
        for (label, stream) in [
            ("320k", &station.stream_320),
            ("HLS", &station.stream_hls),
            ("128k", &station.stream_128),
            ("64k", &station.stream_64),
        ] {
            if !stream.is_empty() && stream == url {
                return label.into();
            }
        }
    }
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', ':', '?'])
        .next()
        .unwrap_or(rest)
        .to_owned()
}

fn age_text(age: Duration) -> String {
    let seconds = age.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m{}s", seconds / 60, seconds % 60)
    }
}

fn uptime_text(uptime: Duration) -> String {
    let seconds = uptime.as_secs();
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds % 3600 / 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

// The recent level history as block characters: always a recording of real
// samples, never an interpolation. Shown only inside the diagnostics overlay.
fn sparkline(history: &VecDeque<f64>, width: usize) -> String {
    const BLOCKS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let start = history.len().saturating_sub(width);
    history
        .iter()
        .skip(start)
        .map(|level| BLOCKS[(level.clamp(0.0, 1.0) * 7.0).round() as usize])
        .collect()
}

// One thin stroke following the current audio level. The percentile
// normalization and attack/release easing upstream make the motion smooth;
// there is no history, scrolling, or peak marker here. The terminal's font
// determines the physical stroke thickness.
fn meter(width: u16, level: f64, theme: &Theme) -> Line<'static> {
    let level = if level.is_finite() {
        level.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let active = (level * f64::from(width)).round() as usize;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_style = Style::default();
    for index in 0..usize::from(width) {
        let style = if index < active {
            Style::default().fg(theme.accent)
        } else {
            Style::default().fg(theme.edge)
        };
        let symbol = "─";
        if run_style == style && !run.is_empty() {
            run.push_str(symbol);
        } else {
            if !run.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut run), run_style));
            }
            run_style = style;
            run.push_str(symbol);
        }
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, run_style));
    }
    Line::from(spans)
}

fn account(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let account = &app.account;
    let width = 66.min(area.width.saturating_sub(4));
    let height = if account.conflict.is_some() { 18 } else { 14 }.min(area.height);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let inner_width = width.saturating_sub(4) as usize;
    let clip = |text: &str| -> String {
        if text.chars().count() <= inner_width {
            text.to_owned()
        } else {
            format!(
                "{}…",
                text.chars()
                    .take(inner_width.saturating_sub(1))
                    .collect::<String>()
            )
        }
    };
    let muted = |text: String| Line::from(Span::styled(text, Style::default().fg(theme.muted)));
    let mut lines = Vec::new();
    let title;
    let hint;
    if account.logout_confirm {
        title = " Sign out ";
        hint = " Enter confirm · Esc cancel ";
        lines.push(Line::raw("Sign out of Radio Record?"));
        lines.push(Line::raw("Local favorites will be kept."));
        lines.push(muted("Pending sync will be cancelled.".into()));
    } else if let Some(conflict) = &account.conflict {
        title = " Resolve favorites ";
        hint = " ↑↓ choose · Enter apply · Esc later ";
        let local = &app.settings.favorites;
        let remote = &conflict.remote;
        let base = account.session.as_ref().and_then(|s| s.baseline.as_ref());
        let combined = merge(base, local, remote);
        lines.push(Line::raw(format!(
            "Local {} · Server {} · Shared {}",
            local.len(),
            remote.len(),
            local.intersection(remote).count()
        )));
        if height >= 16 {
            let names = |ids: Vec<i64>| -> String {
                let names: Vec<_> = ids
                    .iter()
                    .take(3)
                    .map(|id| {
                        app.catalog
                            .as_ref()
                            .and_then(|c| c.stations.iter().find(|s| s.id == *id))
                            .map(|s| s.title.clone())
                            .unwrap_or_else(|| format!("#{id}"))
                    })
                    .collect();
                if names.is_empty() {
                    "none".into()
                } else {
                    names.join(", ")
                }
            };
            lines.push(muted(clip(&format!(
                "Only local: {}",
                names(local.difference(remote).copied().collect())
            ))));
            lines.push(muted(clip(&format!(
                "Only server: {}",
                names(remote.difference(local).copied().collect())
            ))));
        }
        lines.push(Line::raw(""));
        for (choice, label, count) in [
            (Choice::Merge, "1  Merge both", combined.len()),
            (Choice::Local, "2  Use local on both", local.len()),
            (Choice::Server, "3  Use server on both", remote.len()),
        ] {
            let selected = choice == conflict.choice;
            lines.push(Line::from(Span::styled(
                format!("{} {label} ({count})", if selected { "›" } else { " " }),
                Style::default().fg(if selected { theme.accent } else { theme.fg }),
            )));
        }
        let target = match conflict.choice {
            Choice::Merge => &combined,
            Choice::Local => local,
            Choice::Server => remote,
        };
        lines.push(Line::raw(""));
        lines.push(muted(format!(
            "Local: +{} / −{}",
            target.difference(local).count(),
            local.difference(target).count()
        )));
        lines.push(muted(format!(
            "Server: +{} / −{}",
            target.difference(remote).count(),
            remote.difference(target).count()
        )));
        if height >= 16 {
            lines.push(muted(
                if base.is_some() {
                    "Merge honors deletions since last sync."
                } else {
                    "First merge keeps all favorites."
                }
                .into(),
            ));
            lines.push(muted("Changes apply only after Enter.".into()));
        }
    } else if account.session.is_some() {
        title = " Radio Record account ";
        hint = " s sync · l sign out · Esc close ";
        if let Some(profile) = &account.profile {
            lines.push(Line::raw(clip(&profile.email)));
            if !profile.name.is_empty() {
                lines.push(muted(clip(&profile.name)));
            }
            lines.push(Line::raw(if profile.premium {
                "Premium account"
            } else {
                "Free account"
            }));
        } else {
            lines.push(Line::raw("Restoring account…"));
        }
        lines.push(Line::raw(format!(
            "{} favorite stations locally",
            app.settings.favorites.len()
        )));
        let pending = account
            .session
            .as_ref()
            .and_then(|s| s.baseline.as_ref())
            .is_none_or(|base| *base != app.settings.favorites);
        let status = if account.busy {
            "Syncing favorites…".to_owned()
        } else if pending {
            "Local changes pending sync".to_owned()
        } else if let Some(instant) = account.last_sync {
            format!("Synced {}s ago", instant.elapsed().as_secs())
        } else {
            "Ready to sync".to_owned()
        };
        lines.push(muted(status));
        lines.push(Line::raw(""));
        lines.push(Line::raw("s  Sync favorites now"));
        lines.push(Line::raw("l  Sign out / change account"));
    } else {
        title = " Sign in to Radio Record ";
        hint = " Tab field · Enter sign in · Esc close ";
        lines.push(muted("Use your Radio Record email account.".into()));
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("{} Email", if !account.password_focus { "›" } else { " " }),
            Style::default().fg(theme.accent),
        )));
        // Keep the end of a long input visible while typing.
        let email: String = account
            .email
            .chars()
            .rev()
            .take(inner_width)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        lines.push(Line::raw(email));
        lines.push(Line::from(Span::styled(
            format!(
                "{} Password",
                if account.password_focus { "›" } else { " " }
            ),
            Style::default().fg(theme.accent),
        )));
        lines.push(Line::raw(
            "*".repeat(account.password.chars().count().min(inner_width)),
        ));
        lines.push(Line::raw(""));
        lines.push(muted(
            if account.busy {
                "Signing in…"
            } else {
                "Password is never saved."
            }
            .into(),
        ));
    }
    if let Some(error) = &account.error {
        lines.push(Line::from(Span::styled(
            clip(error),
            Style::default().fg(theme.alert),
        )));
    }
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.fg).bg(theme.bg))
            .block(
                Block::default()
                    .title(title)
                    .title_bottom(hint)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.alert))
                    .padding(Padding::horizontal(1)),
            ),
        popup,
    );
}

fn help(frame: &mut Frame, area: Rect, scroll: u16, theme: &Theme) {
    let width = 52.min(area.width.saturating_sub(4));
    let height = 19.min(area.height);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let lines = [
        ("↑ ↓ / j k", "select"),
        ("Tab / Shift Tab", "next / previous category"),
        ("Enter", "play station"),
        ("Space / F8", "play / pause"),
        ("F7 / F9", "previous / next station"),
        ("− / =", "player volume, 2% steps"),
        ("f", "toggle favorite"),
        ("Esc", "close"),
        ("i", "toggle history"),
        ("d", "diagnostics"),
        ("a", "account / sync favorites"),
        ("PgUp PgDn Home End", "scroll"),
        ("r", "refresh"),
        ("s", "stop"),
        ("q / Ctrl C", "quit"),
    ]
    .into_iter()
    .map(|(keys, label)| {
        Line::from(vec![
            Span::styled(format!("{keys:<19}"), Style::default().fg(theme.accent)),
            Span::raw(label),
        ])
    })
    .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((
                scroll.min(15u16.saturating_sub(height.saturating_sub(4))),
                0,
            ))
            .style(Style::default().fg(theme.fg).bg(theme.bg))
            .block(
                Block::default()
                    .title(" Keys ")
                    .title_bottom(if height < 19 {
                        " ↑↓  ? / Esc "
                    } else {
                        " ? / Esc "
                    })
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.alert))
                    .padding(Padding::uniform(1)),
            ),
        popup,
    );
}

// The diagnostics overlay exposes the machine: state machine, generation, stream
// telemetry, worker health, and a recorded level sparkline. Gated like help and
// rendered on top of the paused-underneath UI.
fn diag(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let width = 56.min(area.width.saturating_sub(4));
    let height = 13.min(area.height);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let state = match app.playback.state {
        PlayerState::Idle => "idle",
        PlayerState::Buffering => "buffering",
        PlayerState::Playing => "playing",
        PlayerState::Paused => "paused",
        PlayerState::Error => "error",
    };
    let audio = telemetry_segments(app).join(" · ");
    let stream = match &app.playback.stream {
        Some(url) => format!(
            "{} · round {}/{}",
            stream_label(app, url),
            app.playback.stream_round,
            STREAM_ROUNDS
        ),
        None => String::new(),
    };
    let lines = [
        ("State", format!("{state} · generation {}", app.generation)),
        (
            "Audio",
            if audio.is_empty() {
                "—".into()
            } else {
                audio
            },
        ),
        (
            "Stream",
            if stream.is_empty() {
                "—".into()
            } else {
                stream
            },
        ),
        (
            "Buffer",
            match (app.playback.cache_seconds, app.playback.underruns) {
                (Some(seconds), 0) => format!("{seconds:.1}s"),
                (Some(seconds), underruns) => {
                    format!("{seconds:.1}s · {underruns} underruns")
                }
                (None, underruns) if underruns > 0 => format!("{underruns} underruns"),
                _ => "—".into(),
            },
        ),
        (
            "Worker",
            if app.worker_stalled {
                "stalled".into()
            } else {
                "ok".into()
            },
        ),
        (
            "History",
            match app.history_age {
                Some(age) => format!("updated {} ago", age_text(age)),
                None => "—".into(),
            },
        ),
        ("Uptime", uptime_text(app.uptime)),
        (
            "Level",
            if app.meter_history.is_empty() {
                "—".to_owned()
            } else {
                sparkline(&app.meter_history, 36)
            },
        ),
    ]
    .into_iter()
    .map(|(label, value)| {
        Line::from(vec![
            Span::styled(format!("{label:<9}"), Style::default().fg(theme.accent)),
            Span::raw(value),
        ])
    })
    .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.fg).bg(theme.bg))
            .block(
                Block::default()
                    .title(" Diagnostics ")
                    .title_bottom(" d / Esc ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.alert))
                    .padding(Padding::uniform(1)),
            ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{api::Track, app::tests::fixture};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn favorite_marker_only_appears_outside_favorites() {
        let theme = Theme::default();
        let mut app = fixture();
        app.settings.favorites.insert(1);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render(frame, &mut app, &theme))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Record  +"));
        assert!(!text.contains(['♥', '♡']));
        assert!(!text.contains("radio-record"));
        app.category.select(Some(1));
        terminal
            .draw(|frame| render(frame, &mut app, &theme))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Favorites"));
        assert!(text.contains("Record"));
        assert!(!text.contains('+'));
    }

    #[test]
    fn themes_resolve_with_no_color_taking_precedence() {
        let default = Theme::default();
        assert_eq!(Theme::resolve(false, None), default);
        assert_eq!(Theme::resolve(false, Some("default")), default);
        assert_eq!(Theme::resolve(false, Some(" amber ")), Theme::amber());
        assert_eq!(Theme::resolve(false, Some("phosphor")), Theme::phosphor());
        assert_eq!(Theme::resolve(false, Some("paper")), Theme::paper());
        assert_eq!(Theme::resolve(false, Some("nope")), default);
        assert_eq!(Theme::resolve(false, Some("  ")), default);
        assert_ne!(Theme::amber(), default);
        let plain = Theme::resolve(true, Some("amber"));
        assert_eq!(plain, Theme::resolve(true, None));
        assert_ne!(plain, Theme::amber());
    }

    #[test]
    fn meter_strokes_follow_the_level() {
        let theme = Theme::default();
        for (level, expected) in [(0.0, 0), (0.25, 10), (0.75, 30), (1.0, 40), (f64::NAN, 0)] {
            let mut terminal = Terminal::new(TestBackend::new(40, 1)).unwrap();
            terminal
                .draw(|frame| {
                    frame.render_widget(Paragraph::new(meter(40, level, &theme)), frame.area())
                })
                .unwrap();
            let cells = terminal.backend().buffer().content();
            assert!(cells.iter().all(|cell| cell.symbol() == "─"));
            assert_eq!(
                cells.iter().filter(|cell| cell.fg == theme.accent).count(),
                expected
            );
        }
    }

    #[test]
    fn the_meter_settles_when_not_playing() {
        let theme = Theme::default();
        let mut app = fixture();
        app.now = app.selected_station().cloned();
        app.playback.state = PlayerState::Paused;
        app.playback.levels = [0.8, 0.6];
        let mut terminal = Terminal::new(TestBackend::new(60, 4)).unwrap();
        terminal
            .draw(|frame| playback(frame, &app, frame.area(), &theme))
            .unwrap();
        let buffer = terminal.backend().buffer();
        for column in 0..60u16 {
            let cell = &buffer[(column, 0)];
            if cell.symbol() == "─" {
                assert_eq!(cell.fg, theme.edge, "the bar must be at rest");
            }
        }
    }

    #[test]
    fn buffering_shows_real_progress_only_while_buffering() {
        let theme = Theme::default();
        let mut app = fixture();
        app.now = app.selected_station().cloned();
        app.playback.state = PlayerState::Buffering;
        app.playback.buffering = Some(42);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render(frame, &mut app, &theme))
            .unwrap();
        let text = cell_text(terminal.backend().buffer());
        assert!(text.contains("· 42%"), "real cache progress must show");
        app.playback.buffering = None;
        terminal
            .draw(|frame| render(frame, &mut app, &theme))
            .unwrap();
        assert!(!cell_text(terminal.backend().buffer()).contains("42%"));
        // A stale percentage must not leak into other states.
        app.playback.state = PlayerState::Playing;
        app.playback.buffering = Some(42);
        terminal
            .draw(|frame| render(frame, &mut app, &theme))
            .unwrap();
        assert!(!cell_text(terminal.backend().buffer()).contains("42%"));
    }

    #[test]
    fn diagnostics_overlay_shows_machine_state_and_sparkline() {
        let theme = Theme::default();
        let mut app = fixture();
        app.now = app.selected_station().cloned();
        app.playback.state = PlayerState::Playing;
        app.playback.levels = [0.6, 0.6];
        app.playback.codec = Some("aac".into());
        app.playback.samplerate = Some(44_100);
        app.playback.channels = Some(2);
        app.playback.bitrate = Some(128_000);
        app.playback.stream = Some("https://example.com/radio".into());
        app.playback.stream_round = 2;
        app.generation = 7;
        app.uptime = Duration::from_secs(75);
        app.history_age = Some(Duration::from_secs(3));
        for level in [0.1, 0.4, 0.8, 0.5, 0.2] {
            app.meter_history.push_back(level);
        }
        app.show_diag = true;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| render(frame, &mut app, &theme))
            .unwrap();
        let text = cell_text(terminal.backend().buffer());
        assert!(text.contains("Diagnostics"));
        assert!(text.contains("generation 7"));
        assert!(text.contains("aac · 44.1kHz stereo · 128k"));
        assert!(text.contains("320k · round 2/3"));
        assert!(text.contains("updated 3s ago"));
        assert!(text.contains("1:15"), "75 seconds formats as 1:15");
        assert!(text.contains("▄"), "the sparkline records real levels");
        // Outside diagnostics the machine stays hidden.
        app.show_diag = false;
        terminal
            .draw(|frame| render(frame, &mut app, &theme))
            .unwrap();
        let text = cell_text(terminal.backend().buffer());
        assert!(!text.contains("Diagnostics"));
        assert!(!text.contains("128k"));
        assert!(!text.contains("generation"));
    }

    #[test]
    fn account_login_masks_password_and_fits_supported_terminal_sizes() {
        for (width, height) in [(44, 12), (66, 18), (100, 30)] {
            let mut app = fixture();
            app.account.open = true;
            app.account.email = "test@example.com".into();
            app.account.password = "never-render-this".into();
            app.account.password_focus = true;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render(frame, &mut app, &Theme::default()))
                .unwrap();
            let text = cell_text(terminal.backend().buffer());
            assert!(text.contains("Sign in to Radio Record"));
            assert!(text.contains("test@example.com"));
            assert!(text.contains("*****************"));
            assert!(!text.contains("never-render-this"));
            assert!(text.contains("Enter sign in"));
        }
    }

    #[test]
    fn account_conflict_previews_choices_and_additions_and_removals() {
        for (width, height) in [(44, 12), (66, 18), (100, 30)] {
            let mut app = fixture();
            app.account.open = true;
            app.settings.favorites = [1, 2].into_iter().collect();
            app.account.conflict = Some(crate::account::Conflict {
                remote: [2, 3].into_iter().collect(),
                choice: Choice::Server,
            });
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render(frame, &mut app, &Theme::default()))
                .unwrap();
            let text = cell_text(terminal.backend().buffer());
            assert!(text.contains("Resolve favorites"));
            assert!(text.contains("Merge both (3)"));
            assert!(text.contains("Use local on both (2)"));
            assert!(text.contains("Use server on both (2)"));
            assert!(text.contains("Local: +1 / −1"));
            assert!(text.contains("Server: +0 / −0"));
            assert!(text.contains("Enter apply"));
        }
    }

    fn cell_text(buffer: &ratatui::buffer::Buffer) -> String {
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn rendering_follows_the_theme_palette() {
        for theme in [Theme::amber(), Theme::phosphor(), Theme::paper()] {
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            let mut app = fixture();
            app.now = app.selected_station().cloned();
            app.playback.state = PlayerState::Playing;
            terminal
                .draw(|frame| render(frame, &mut app, &theme))
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert!(
                buffer
                    .content()
                    .iter()
                    .any(|cell| cell.fg == theme.accent && cell.symbol() == "▶")
            );
            let background = buffer.content().first().unwrap().bg;
            assert_eq!(background, theme.bg);
        }
    }

    #[test]
    fn renders_unicode_empty_states_and_small_terminals() {
        let theme = Theme::default();
        for (width, height) in [(120, 38), (80, 24), (44, 12), (20, 6), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut app = fixture();
            app.now = app.selected_station().cloned();
            app.playback.state = PlayerState::Playing;
            app.history = (0..12)
                .map(|index| Track {
                    id: index,
                    artist: "Артист".into(),
                    song: "Очень длинное название трека 音楽 🎵".repeat(4),
                    image100: String::new(),
                    image200: String::new(),
                    time_formatted: "12:34".into(),
                })
                .collect();
            terminal
                .draw(|frame| render(frame, &mut app, &theme))
                .unwrap();
            if width >= 80 {
                let buffer = terminal.backend().buffer();
                let text = buffer
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>();
                assert!(!text.contains("radio-record"));
                assert!(text.contains("Favorites"));
                assert!(text.contains("Record"));
                assert!(
                    buffer
                        .content()
                        .iter()
                        .any(|cell| cell.fg == theme.accent && cell.symbol() == "▶")
                );
            }
            app.help = true;
            terminal
                .draw(|frame| render(frame, &mut app, &theme))
                .unwrap();
            app.help = false;
            app.show_diag = true;
            terminal
                .draw(|frame| render(frame, &mut app, &theme))
                .unwrap();
            app.show_diag = false;
            app.category.select(Some(1));
            terminal
                .draw(|frame| render(frame, &mut app, &theme))
                .unwrap();
        }
    }

    #[test]
    fn scrolling_reaches_last_station() {
        let theme = Theme::default();
        let mut app = fixture();
        let template = app.catalog.as_ref().unwrap().stations[0].clone();
        app.catalog.as_mut().unwrap().stations = (0..100)
            .map(|id| {
                let mut station = template.clone();
                station.id = id;
                station.title = format!("Station {id:03}");
                station
            })
            .collect();
        app.stations.select(Some(99));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render(frame, &mut app, &theme))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Station 099"));
        assert!(!text.contains("Station 000"));
    }
}
