use crate::{
    account::{Choice, merge},
    app::{App, Category},
    cover::Cover,
    fx,
    player::{PlayerState, STREAM_ROUNDS},
    songs::Service,
};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph, Widget},
};
use ratatui_image::{
    Image, Resize,
    picker::{Picker, ProtocolType},
    protocol::Protocol,
};
use std::{collections::VecDeque, time::Duration};

/// Color palette for the whole UI ("NEON-77" by default). `accent` (neon
/// cyan) marks playback and live audio, `alert` (neon magenta) the selected
/// category, favorites, live badges, and errors, `wave` (violet) is the
/// midpoint of the cyan-to-magenta gradients, `band` fills title tabs and
/// buttons, `warn` (acid yellow) marks buffering, stalls, and peak caps, and
/// `glow` is a dim neon used beside bright cells to suggest light bleed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    bg: Color,
    fg: Color,
    muted: Color,
    edge: Color,
    accent: Color,
    alert: Color,
    selected: Color,
    wave: Color,
    band: Color,
    warn: Color,
    glow: Color,
    /// Streaming services in their own neon colors; other palettes keep
    /// to their own hues.
    brands: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: Color::Rgb(7, 7, 13),
            fg: Color::Rgb(232, 228, 255),
            muted: Color::Rgb(108, 104, 147),
            edge: Color::Rgb(43, 36, 69),
            accent: Color::Rgb(0, 240, 255),
            alert: Color::Rgb(255, 42, 109),
            selected: Color::Rgb(42, 15, 51),
            wave: Color::Rgb(157, 77, 255),
            band: Color::Rgb(19, 17, 31),
            warn: Color::Rgb(252, 238, 10),
            glow: Color::Rgb(11, 58, 68),
            brands: true,
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
            wave: Color::Rgb(214, 140, 20),
            band: Color::Rgb(48, 28, 0),
            warn: Color::Rgb(255, 241, 194),
            glow: Color::Rgb(58, 36, 0),
            brands: false,
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
            wave: Color::Rgb(60, 200, 100),
            band: Color::Rgb(6, 34, 14),
            warn: Color::Rgb(232, 255, 176),
            glow: Color::Rgb(10, 58, 24),
            brands: false,
        }
    }

    /// Light theme for paper-white terminals.
    fn paper() -> Self {
        Self {
            bg: Color::Rgb(250, 249, 244),
            fg: Color::Rgb(52, 49, 44),
            muted: Color::Rgb(132, 127, 117),
            edge: Color::Rgb(213, 208, 196),
            accent: Color::Rgb(0, 124, 134),
            alert: Color::Rgb(196, 22, 111),
            selected: Color::Rgb(228, 225, 242),
            wave: Color::Rgb(110, 70, 190),
            band: Color::Rgb(234, 231, 220),
            warn: Color::Rgb(154, 107, 0),
            glow: Color::Rgb(217, 240, 242),
            brands: false,
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
            wave: Color::Reset,
            band: Color::Reset,
            warn: Color::Reset,
            glow: Color::Reset,
            brands: false,
        }
    }

    /// False for the `NO_COLOR` palette, where pictures cannot be drawn.
    fn colorful(&self) -> bool {
        self.bg != Color::Reset
    }

    /// The accent of one streaming service in the link card.
    fn service(&self, service: Service) -> Color {
        match (self.brands, service) {
            (true, Service::YouTube) => Color::Rgb(255, 0, 51),
            (true, Service::Spotify) => Color::Rgb(30, 215, 96),
            (true, Service::Yandex) => Color::Rgb(255, 204, 0),
            (true, Service::Apple) => Color::Rgb(250, 87, 193),
            (false, Service::YouTube) => self.alert,
            (false, Service::Spotify) => self.accent,
            (false, Service::Yandex) => self.warn,
            (false, Service::Apple) => self.wave,
            (_, Service::Record) => self.alert,
        }
    }

    /// The neon gradient at `t` in 0..1: accent, then wave, then alert.
    /// Without RGB colors it degrades to the nearest stop.
    fn gradient(&self, t: f32) -> Color {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if t < 0.5 {
            mix(self.accent, self.wave, t * 2.0)
        } else {
            mix(self.wave, self.alert, (t - 0.5) * 2.0)
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

/// Linear blend of two RGB colors; anything else snaps to the nearer one.
fn mix(from: Color, to: Color, t: f32) -> Color {
    match (from, to) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let blend =
                |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
            Color::Rgb(blend(r1, r2), blend(g1, g2), blend(b1, b2))
        }
        _ if t < 0.5 => from,
        _ => to,
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

/// Cover drawing mode from `RADIO_RECORD_COVER`: `pixel` (default) draws
/// half-block pixel art everywhere; `sharp` asks the terminal for a picture
/// protocol (kitty, sixel, iTerm2). Read before the terminal takes over.
pub fn sharp_covers_from_env() -> bool {
    let value = std::env::var("RADIO_RECORD_COVER").unwrap_or_default();
    match value.trim() {
        "sharp" => true,
        "" | "pixel" => false,
        other => {
            eprintln!("radio-record: unknown RADIO_RECORD_COVER '{other}', using pixel");
            false
        }
    }
}

/// Terminal picture support for sharp covers. Without a picker (the default,
/// and in tests) every cover is drawn in half-blocks by `CoverArt`.
#[derive(Default)]
pub struct Graphics {
    picker: Option<Picker>,
    /// The encoded picture for one cover URL at one size; rebuilt only when
    /// either changes, never per frame.
    cached: Option<(String, Rect, Protocol)>,
}

impl Graphics {
    /// Ask the terminal which picture protocol it speaks. Must run after the
    /// alternate screen is entered and before any terminal event is read, or
    /// the event reader swallows the reply. Half-block-only terminals keep
    /// the built-in pixel art.
    pub fn query() -> Self {
        let picker = Picker::from_query_stdio()
            .ok()
            .filter(|picker| picker.protocol_type() != ProtocolType::Halfblocks);
        Self {
            picker,
            cached: None,
        }
    }

    /// Draw `cover` as a real picture if the terminal can. Returns false when
    /// the caller should fall back to half-blocks.
    fn draw(&mut self, frame: &mut Frame, cover: &Cover, area: Rect) -> bool {
        let Some(picker) = &self.picker else {
            return false;
        };
        let fresh = self
            .cached
            .as_ref()
            .is_some_and(|(url, cached, _)| *url == cover.url && *cached == area);
        if !fresh {
            let image = image::DynamicImage::ImageRgb8(cover.image.clone());
            let filter = image::imageops::FilterType::Triangle;
            self.cached = picker
                .new_protocol(image, area.as_size(), Resize::Fit(Some(filter)))
                .ok()
                .map(|protocol| (cover.url.clone(), area, protocol));
        }
        match &self.cached {
            Some((_, _, protocol)) if protocol.needs_placeholder(area).is_none() => {
                frame.render_widget(Image::new(protocol), area);
                true
            }
            _ => false,
        }
    }
}

pub fn render(frame: &mut Frame, app: &mut App, theme: &Theme, graphics: &mut Graphics) {
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
    let [header_area, inner] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)])
        .areas(area.inner(Margin::new(1, 0)));
    header(frame, app, header_area, theme);
    let panel = Panel::fit(inner.height, inner.width);
    let footer_height = match panel {
        Some(panel) => panel.height(),
        // Small terminals keep the compact footer: visualizer row(s) + 3 lines.
        None if area.height >= 20 => 5,
        None => 4,
    };
    let [body, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(footer_height)]).areas(inner);
    let category_width = if area.width >= 90 { 25 } else { 18 };
    let [sidebar, right] =
        Layout::horizontal([Constraint::Length(category_width), Constraint::Min(1)]).areas(body);
    if app.vault.open {
        // The vault takes the whole body; header and now playing stay.
        vault(frame, app, body, theme);
    } else if app.show_history && body.height >= 12 {
        categories(frame, app, sidebar, theme);
        let [stations_area, history_area] = Layout::vertical([
            Constraint::Min(6),
            Constraint::Length((body.height / 3).clamp(5, 8)),
        ])
        .areas(right);
        stations(frame, app, stations_area, theme);
        history(frame, app, history_area, theme);
    } else {
        categories(frame, app, sidebar, theme);
        stations(frame, app, right, theme);
    }
    match panel {
        Some(panel) => now_playing(frame, app, footer, panel, theme, graphics),
        None => playback(frame, app, footer.inner(Margin::new(1, 0)), theme),
    }
    if app.vault.open && app.vault.detail.is_some() {
        link_card(frame, area, app, theme);
    }
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

/// A HUD frame: heavy borders with the title notched into the top edge,
/// as in `┏━ // CHANNELS ━━━ 09/112 ━┓`.
fn hud<'a>(label: &'a str, theme: &Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(theme.edge))
        .title(Line::from(vec![
            Span::styled("━ ", Style::default().fg(theme.edge)),
            Span::styled("// ", Style::default().fg(theme.muted)),
            Span::styled(label, Style::default().fg(theme.fg)),
            Span::raw(" "),
        ]))
}

/// A notch on the right end of a HUD border: ` text ━`.
fn notch(spans: Vec<Span<'static>>, theme: &Theme) -> Line<'static> {
    let mut line = vec![Span::raw(" ")];
    line.extend(spans);
    line.push(Span::styled(" ━", Style::default().fg(theme.edge)));
    Line::from(line).right_aligned()
}

/// The top line: the screen title and machine state, all from real data.
fn header(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    // The one continuous decoration: a slow pulse while on air.
    let on_air = if app.fx.decorative() && !fx::blink(app.uptime, fx::PULSE) {
        mix(theme.alert, theme.bg, 0.55)
    } else {
        theme.alert
    };
    let (pill, color) = match app.playback.state {
        PlayerState::Playing => ("◉ ON AIR".to_owned(), on_air),
        PlayerState::Buffering => (
            format!("◉ SYNC {}%", app.playback.buffering.unwrap_or(0)),
            theme.warn,
        ),
        PlayerState::Paused => ("▮▮ HOLD".to_owned(), theme.muted),
        PlayerState::Error => ("! FAULT".to_owned(), theme.warn),
        PlayerState::Idle => ("○ STANDBY".to_owned(), theme.muted),
    };
    let dot = || Span::styled(" ▪ ", Style::default().fg(theme.edge));
    let right = Line::from(vec![
        Span::raw(" "),
        Span::styled(pill, Style::default().fg(color)),
        dot(),
        Span::styled(
            format!("{} FAV", app.settings.favorites.len()),
            Style::default().fg(theme.muted),
        ),
        dot(),
        Span::styled("♥ ", Style::default().fg(theme.alert)),
        Span::styled(
            app.settings.songs.len().to_string(),
            Style::default().fg(theme.muted),
        ),
        dot(),
        Span::styled(
            format!("UP {}", uptime_text(app.uptime)),
            Style::default().fg(theme.muted),
        ),
        Span::styled(" ◣", Style::default().fg(theme.alert)),
    ]);
    let left = Line::from(vec![
        Span::styled("◢ ", Style::default().fg(theme.alert)),
        Span::styled(
            "RADIO",
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("//", Style::default().fg(theme.alert)),
        Span::styled(
            "RECORD ",
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
    ]);
    let rule = usize::from(area.width).saturating_sub(left.width() + right.width());
    let mut spans = left.spans;
    spans.push(Span::styled(
        "━".repeat(rule),
        Style::default().fg(theme.edge),
    ));
    spans.extend(right.spans);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn categories(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
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
    let block = hud("SECTOR", theme).title_bottom(Line::from(vec![
        Span::styled("━ ", Style::default().fg(theme.edge)),
        Span::styled("a ", Style::default().fg(theme.accent)),
        Span::styled(
            format!("{label}{suffix} "),
            Style::default().fg(theme.muted),
        ),
    ]));
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
            .highlight_symbol("▌"),
        area,
        &mut app.category,
    );
}

fn stations(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let visible = app.visible();
    let count = format!(
        "{:02}/{:02}",
        app.stations.selected().map_or(0, |index| index + 1),
        visible.len()
    );
    let block = hud("CHANNELS", theme).title(notch(
        vec![Span::styled(count, Style::default().fg(theme.muted))],
        theme,
    ));
    let inner = block.inner(area);
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
    // One column goes to the highlight symbol.
    let width = usize::from(inner.width.saturating_sub(1));
    let items: Vec<_> = visible
        .iter()
        .map(|station| {
            let is_current = app.now.as_ref().is_some_and(|now| now.id == station.id)
                && !matches!(app.playback.state, PlayerState::Idle | PlayerState::Error);
            let marker = if is_current {
                match app.playback.state {
                    PlayerState::Paused => "▮▮ ",
                    PlayerState::Buffering => "·  ",
                    _ => "▶  ",
                }
            } else {
                "   "
            };
            let mut spans = vec![
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
            ];
            if is_current && app.playback.state == PlayerState::Playing {
                let tag = "◉ LIVE ";
                let used: usize = spans.iter().map(Span::width).sum();
                if used + tag.chars().count() < width {
                    let gap = width - used - tag.chars().count();
                    spans.push(Span::raw(" ".repeat(gap)));
                    spans.push(Span::styled(tag, Style::default().fg(theme.alert)));
                }
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(Style::default().bg(theme.selected))
            .highlight_symbol("▌"),
        area,
        &mut app.stations,
    );
}

fn history(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let block = hud("INTERCEPT LOG", theme);
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
                    format!(" {} ", track.time_formatted),
                    Style::default().fg(theme.muted),
                ),
                Span::styled(">> ", Style::default().fg(theme.edge)),
                Span::styled(track.artist.clone(), Style::default().fg(theme.fg)),
                Span::styled(" :: ", Style::default().fg(theme.edge)),
                Span::styled(track.song.clone(), Style::default().fg(theme.muted)),
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

/// The footer: the spectrum visualizer on top, then status, track, and the
/// global error on the last three rows.
fn playback(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let rows = area.height.saturating_sub(3).max(1);
    frame.render_widget(
        Visualizer::new(spectrum_levels(app), theme),
        Rect::new(area.x, area.y, area.width, rows),
    );
    let area = Rect::new(
        area.x,
        area.y + rows,
        area.width,
        area.height.saturating_sub(rows),
    );
    if area.height < 3 {
        return;
    }
    let [status, volume] = Layout::horizontal([Constraint::Min(1), Constraint::Length(5)])
        .areas(Rect::new(area.x, area.y, area.width, 1));
    let spans = status_spans(app, theme);
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
        let heart = if app.on_air_liked() { "♥ " } else { "" };
        format!("{heart}{} — {}", track.artist, track.song)
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
    let status = match app.active_toast() {
        Some(_) => Line::from(toast_spans(app, theme)),
        None => Line::styled(
            app.error.as_deref().unwrap_or("").to_owned(),
            Style::default().fg(theme.alert),
        ),
    };
    frame.render_widget(
        Paragraph::new(status),
        Rect::new(area.x, area.y + 2, area.width, 1),
    );
}

/// The playback state symbol and station name, plus telemetry segments while
/// the diagnostics overlay is open.
fn status_spans(app: &App, theme: &Theme) -> Vec<Span<'static>> {
    let symbol = match app.playback.state {
        PlayerState::Idle => "○".to_owned(),
        // Real cache progress while the stream fills; a bare dot until the engine reports progress.
        PlayerState::Buffering => match app.playback.buffering {
            Some(percent) => format!("· {percent}%"),
            None => "·".to_owned(),
        },
        PlayerState::Playing => "▶".to_owned(),
        PlayerState::Paused => "▮▮".to_owned(),
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
    spans
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

/// Size of the now-playing panel: a heavy frame with the status notched
/// into its borders, the cover beside the track details, and the visualizer
/// beside them at full cover height (or below them when the panel is narrow).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Panel {
    cover_rows: u16,
    /// Visualizer rows under the cover; zero when it sits beside the details.
    below_rows: u16,
}

/// The details column never shrinks below this; the side visualizer needs
/// at least `SIDE_VISUALIZER` columns or moves below.
const DETAILS_MIN: u16 = 36;
const DETAILS_MAX: u16 = 56;
const SIDE_VISUALIZER: u16 = 16;

impl Panel {
    /// The largest panel that still leaves the station list room, if any.
    fn fit(height: u16, width: u16) -> Option<Self> {
        let cover_rows = match height {
            30.. => 7,
            20.. => 5,
            _ => return None,
        };
        // Borders, a padding column, the cover, gaps, details, visualizer.
        let side = width >= 2 + 1 + cover_rows * 2 + 2 + DETAILS_MIN + 2 + SIDE_VISUALIZER;
        Some(Self {
            cover_rows,
            below_rows: if side { 0 } else { 2 },
        })
    }

    fn height(self) -> u16 {
        2 + self.cover_rows + self.below_rows
    }

    /// Room for a metadata line and a telemetry line of their own.
    fn roomy(self) -> bool {
        self.cover_rows >= 7
    }
}

fn now_playing(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    panel: Panel,
    theme: &Theme,
    graphics: &mut Graphics,
) {
    let edge = Style::default().fg(theme.edge);
    // The frame breathes with the bass: six steps from edge toward accent,
    // so a steady level repaints nothing.
    let glow = if app.fx.reactive() {
        let (levels, _) = spectrum_levels(app);
        let bass = levels.iter().take(12).copied().fold(0.0, f32::max);
        mix(
            theme.edge,
            theme.accent,
            (bass.clamp(0.0, 1.0) * 6.0).floor() / 6.0 * 0.8,
        )
    } else {
        theme.edge
    };
    let mut top = vec![
        Span::styled("━", edge),
        Span::styled("▌", Style::default().fg(theme.accent)),
        Span::styled(
            " NOW PLAYING ",
            Style::default()
                .fg(theme.fg)
                .bg(theme.band)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("▐", Style::default().fg(theme.accent)),
        Span::styled("━ ", edge),
        Span::styled("♫ ", Style::default().fg(theme.wave)),
        Span::styled(window_title(app), Style::default().fg(theme.fg)),
        Span::raw(" "),
    ];
    if app.playback.state == PlayerState::Playing {
        top.push(Span::styled("[LIVE] ", Style::default().fg(theme.alert)));
    }
    let mut bottom = vec![Span::styled("━ ", edge)];
    bottom.extend(status_spans(app, theme));
    bottom.push(Span::raw(" "));
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(glow))
        .title(Line::from(top))
        .title(notch(
            match app.active_toast() {
                Some(_) => toast_spans(app, theme),
                None => volume_meter(app.settings.volume, theme),
            },
            theme,
        ))
        .title_bottom(Line::from(bottom));
    let signal = signal(app, theme);
    if !signal.is_empty() {
        block = block.title_bottom(notch(signal, theme));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    let cover =
        Rect::new(inner.x + 1, inner.y, panel.cover_rows * 2, panel.cover_rows).intersection(inner);
    // Terminal pictures sit above the text layer, so popups would not cover
    // them: any open overlay switches back to half-blocks, which it can cover.
    let overlay = app.help || app.show_diag || app.account.open || app.vault.detail.is_some();
    let sharp = match &app.cover {
        Some(art) if theme.colorful() && !overlay => graphics.draw(frame, art, cover),
        _ => false,
    };
    if !sharp {
        frame.render_widget(CoverArt::new(app.cover.as_ref(), theme), cover);
    }
    let text_x = cover.right() + 2;
    let room = inner.right().saturating_sub(text_x + 1);
    let (details_width, visualizer) = if panel.below_rows == 0 {
        let width = room
            .saturating_sub(2 + SIDE_VISUALIZER)
            .clamp(DETAILS_MIN.min(room), DETAILS_MAX);
        let x = text_x + width + 2;
        (
            width,
            Rect::new(
                x,
                inner.y,
                inner.right().saturating_sub(x + 1),
                panel.cover_rows,
            ),
        )
    } else {
        (
            room,
            Rect::new(
                inner.x + 1,
                inner.y + panel.cover_rows,
                inner.width.saturating_sub(2),
                panel.below_rows,
            ),
        )
    };
    let details_area =
        Rect::new(text_x, inner.y, details_width, panel.cover_rows).intersection(inner);
    frame.render_widget(
        Paragraph::new(details(app, details_area.width, panel, theme)),
        details_area,
    );
    let visualizer = visualizer.intersection(inner);
    match app.playback.buffering {
        Some(percent) if app.playback.state == PlayerState::Buffering && app.fx.reactive() => {
            let sweep = app.fx.decorative().then_some(app.uptime);
            frame.render_widget(Acquire::new(percent, sweep, theme), visualizer);
        }
        _ => frame.render_widget(Visualizer::new(spectrum_levels(app), theme), visualizer),
    }
}

/// While a stream buffers, the visualizer area shows the real fill toward
/// playback: `SIGNAL ACQUIRING 42%` over a row of warning pills. In full
/// motion a bright head sweeps across the filled part; the fill itself only
/// ever moves with the buffer.
struct Acquire<'a> {
    percent: u8,
    sweep: Option<Duration>,
    theme: &'a Theme,
}

impl<'a> Acquire<'a> {
    fn new(percent: u8, sweep: Option<Duration>, theme: &'a Theme) -> Self {
        Self {
            percent: percent.min(100),
            sweep,
            theme,
        }
    }
}

impl Widget for Acquire<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let theme = self.theme;
        let columns = area.width.div_ceil(2);
        let lit = (u32::from(columns) * u32::from(self.percent) / 100) as u16;
        let head = self
            .sweep
            .filter(|_| lit > 0)
            .map(|now| ((now.as_millis() / 60) % u128::from(lit)) as u16);
        let bottom = area.bottom() - 1;
        for column in 0..columns {
            let color = if Some(column) == head {
                theme.fg
            } else if column < lit {
                theme.warn
            } else {
                theme.glow
            };
            buf[(area.x + column * 2, bottom)]
                .set_symbol("▬")
                .set_fg(color);
        }
        if area.height >= 2 {
            let label = format!("SIGNAL ACQUIRING {}%", self.percent);
            let y = area.y + (area.height - 1) / 2;
            buf.set_stringn(
                area.x,
                y,
                &label,
                usize::from(area.width),
                Style::default().fg(theme.warn).add_modifier(Modifier::BOLD),
            );
        }
    }
}

/// `VOL ━━━─────── 30%`: ten cells shading cyan to magenta as volume rises.
fn volume_meter(volume: u8, theme: &Theme) -> Vec<Span<'static>> {
    let filled = usize::from(volume.min(100)).div_ceil(10);
    let mut spans = vec![Span::styled("VOL ", Style::default().fg(theme.muted))];
    for cell in 0..10 {
        spans.push(if cell < filled {
            Span::styled("━", Style::default().fg(theme.gradient(cell as f32 / 9.0)))
        } else {
            Span::styled("─", Style::default().fg(theme.edge))
        });
    }
    spans.push(Span::styled(
        format!(" {volume}%"),
        Style::default().fg(theme.muted),
    ));
    spans
}

/// `SIG ▂▃▅▇ BUF 2.4s`: four bars lit by how much audio is queued, out of
/// the ring's three seconds. Absent when nothing is loading or playing.
fn signal(app: &App, theme: &Theme) -> Vec<Span<'static>> {
    let Some(seconds) = app
        .playback
        .cache_seconds
        .filter(|seconds| seconds.is_finite())
    else {
        return Vec::new();
    };
    let lit = ((seconds / 3.0 * 4.0).ceil() as usize).min(4);
    let on = if app.playback.state == PlayerState::Buffering {
        theme.warn
    } else {
        theme.accent
    };
    let mut spans = vec![Span::styled("SIG ", Style::default().fg(theme.muted))];
    for (index, bar) in ["▂", "▃", "▅", "▇"].into_iter().enumerate() {
        spans.push(Span::styled(
            bar,
            Style::default().fg(if index < lit { on } else { theme.edge }),
        ));
    }
    spans.push(Span::styled(
        format!(" BUF {seconds:.1}s"),
        Style::default().fg(theme.muted),
    ));
    spans
}

/// The panel's window title, a shell-script name after the station on air.
fn window_title(app: &App) -> String {
    let name = app
        .now
        .as_ref()
        .map(|station| station.prefix.trim())
        .filter(|prefix| !prefix.is_empty())
        .unwrap_or("music");
    format!("{name}.sh")
}

/// Title, artist, metadata, progress, transport buttons, telemetry, and
/// errors beside the cover. Status and volume live in the panel's borders.
fn details(app: &App, width: u16, panel: Panel, theme: &Theme) -> Vec<Line<'static>> {
    let (title, artist) = match app.history.first() {
        Some(track) => (track.song.clone(), track.artist.clone()),
        None => (
            app.now
                .as_ref()
                .map_or("Stopped", |station| station.title.as_str())
                .to_owned(),
            if app.history_loading {
                "Loading track…".to_owned()
            } else {
                String::new()
            },
        ),
    };
    let bold = Style::default().fg(theme.fg).add_modifier(Modifier::BOLD);
    // A new track decodes into place with a typing cursor; then it is still.
    let reveal = app
        .track_since
        .filter(|_| app.fx.decorative() && !app.history.is_empty())
        .and_then(|(since, seed)| {
            fx::progress(app.uptime, since, fx::REVEAL).map(|progress| (progress, seed))
        });
    let mut title = match reveal {
        Some((progress, seed)) => Line::from(vec![
            Span::styled(fx::scramble(&title, progress, seed), bold),
            Span::styled("█", Style::default().fg(theme.accent)),
        ]),
        None => Line::styled(title, bold),
    };
    if app.on_air_liked() {
        title
            .spans
            .insert(0, Span::styled("♥ ", Style::default().fg(theme.alert)));
    }
    let mut lines = vec![
        title,
        Line::styled(artist, Style::default().fg(theme.muted)),
    ];
    if panel.roomy() {
        lines.push(metadata_line(app, theme));
    }
    lines.push(progress_line(app, width, theme));
    let mut transport = buttons(app, theme);
    let telemetry = telemetry_line(app, theme);
    if panel.roomy() {
        lines.push(Line::from(transport));
        lines.push(telemetry);
    } else {
        // Narrow panels keep only the values: `AAC/44.1KHZ STEREO/96K`.
        transport.push(Span::raw("  "));
        transport.extend(telemetry.spans.into_iter().skip(2));
        lines.push(Line::from(transport));
    }
    let error = app.playback.error.as_deref().or(app.error.as_deref());
    lines.push(Line::styled(
        error.unwrap_or("").to_owned(),
        Style::default().fg(theme.alert),
    ));
    lines
}

/// `TRANCE ▪ CH-061 ▪ AIRED 16:31:26`: genre, station id, and track start.
fn metadata_line(app: &App, theme: &Theme) -> Line<'static> {
    let mut parts = Vec::new();
    if let Some(station) = &app.now {
        if let Some(genre) = station.genres.first() {
            parts.push(genre.name.to_uppercase());
        }
        parts.push(format!("CH-{:03}", station.id));
    }
    if let Some(track) = app
        .history
        .first()
        .filter(|track| !track.time_formatted.is_empty())
    {
        parts.push(format!("AIRED {}", track.time_formatted));
    }
    Line::styled(parts.join(" ▪ "), Style::default().fg(theme.muted))
}

/// `▶ STREAMING :: AAC/44.1KHZ/STEREO/128K` from the engine's telemetry.
fn telemetry_line(app: &App, theme: &Theme) -> Line<'static> {
    let (label, color) = match app.playback.state {
        PlayerState::Playing => ("STREAMING", theme.accent),
        PlayerState::Buffering => ("BUFFERING", theme.warn),
        PlayerState::Paused => ("HOLD", theme.muted),
        PlayerState::Error => ("FAULT", theme.warn),
        PlayerState::Idle => return Line::default(),
    };
    let mut spans = vec![Span::styled(label, Style::default().fg(color))];
    let segments = telemetry_segments(app);
    if !segments.is_empty() {
        spans.push(Span::styled(" :: ", Style::default().fg(theme.edge)));
        spans.push(Span::styled(
            segments.join("/").to_uppercase(),
            Style::default().fg(theme.muted),
        ));
    }
    Line::from(spans)
}

/// How far the track on air is, as a neon bar with its elapsed time. Radio
/// has no duration, so the bar fills against the station's typical track
/// length and stays empty when that is unknown.
fn progress_line(app: &App, width: u16, theme: &Theme) -> Line<'static> {
    let progress = app.progress();
    let label = progress
        .map(|progress| format!(" {}:{:02}", progress.elapsed / 60, progress.elapsed % 60))
        .unwrap_or_default();
    let bar = usize::from(width).saturating_sub(label.chars().count());
    let filled = progress
        .and_then(|progress| progress.fraction)
        .map_or(0, |fraction| (fraction * bar as f64).round() as usize)
        .min(bar);
    let mut spans: Vec<Span<'static>> = (0..filled)
        .map(|cell| {
            let t = cell as f32 / bar.max(2).saturating_sub(1) as f32;
            Span::styled("━", Style::default().fg(theme.gradient(t)))
        })
        .collect();
    spans.push(Span::styled(
        "─".repeat(bar - filled),
        Style::default().fg(theme.edge),
    ));
    spans.push(Span::styled(label, Style::default().fg(theme.muted)));
    Line::from(spans)
}

/// Previous, play/pause, and next as bracketed keys. They mirror the
/// F7/F8/F9 and media keys; the middle one shows what it would do.
fn buttons(app: &App, theme: &Theme) -> Vec<Span<'static>> {
    let active = matches!(
        app.playback.state,
        PlayerState::Playing | PlayerState::Buffering
    );
    let bracket =
        |text: &'static str| Span::styled(text, Style::default().fg(theme.edge).bg(theme.band));
    let key = |label: &'static str, fg: Color| {
        Span::styled(label, Style::default().fg(fg).bg(theme.band))
    };
    let (middle, color) = if active {
        ("▮▮", theme.accent)
    } else {
        ("▶ ", theme.fg)
    };
    vec![
        bracket("["),
        key("|◀", theme.muted),
        bracket("]"),
        Span::raw(" "),
        bracket("["),
        key(middle, color),
        bracket("]"),
        Span::raw(" "),
        bracket("["),
        key("▶|", theme.muted),
        bracket("]"),
    ]
}

/// The cover in half-blocks: each cell shows two pixels, the top one as the
/// foreground of "▀" and the bottom one as its background. Without a cover
/// (or colors) it draws a quiet placeholder note.
struct CoverArt<'a> {
    cover: Option<&'a Cover>,
    theme: &'a Theme,
}

impl<'a> CoverArt<'a> {
    fn new(cover: Option<&'a Cover>, theme: &'a Theme) -> Self {
        Self { cover, theme }
    }
}

impl Widget for CoverArt<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let Some(cover) = self.cover.filter(|_| self.theme.colorful()) else {
            buf.set_style(area, Style::default().bg(self.theme.band));
            buf[(area.x + area.width / 2, area.y + area.height / 2)]
                .set_symbol("♫")
                .set_fg(self.theme.muted);
            return;
        };
        let pixels = cover.sample(area.width, area.height * 2);
        let width = usize::from(area.width);
        let color = |[r, g, b]: [u8; 3]| Color::Rgb(r, g, b);
        for row in 0..area.height {
            for column in 0..area.width {
                let top = usize::from(row) * 2 * width + usize::from(column);
                buf[(area.x + column, area.y + row)]
                    .set_symbol("▀")
                    .set_fg(color(pixels[top]))
                    .set_bg(color(pixels[top + width]));
            }
        }
    }
}

/// Spectrum levels and peak caps to draw: live bands while audio is heard,
/// nothing (all columns at rest) in every other state.
fn spectrum_levels(app: &App) -> (&[f32], &[f32]) {
    if app.playback.state == PlayerState::Playing {
        (&app.playback.spectrum, &app.playback.spectrum_peaks)
    } else {
        (&[], &[])
    }
}

/// The spectrum as columns of stacked pills ("▬") two cells apart, low
/// frequencies on the left. Every column keeps one dim resting pill so the
/// row reads as a floor; louder bands stack higher, shading from cyan at the
/// bottom through violet to magenta at the top, and a peak cap floats where
/// the column last reached before falling back. Levels and caps are drawn
/// exactly as analyzed: no motion is invented here.
struct Visualizer<'a> {
    levels: &'a [f32],
    peaks: &'a [f32],
    theme: &'a Theme,
}

impl<'a> Visualizer<'a> {
    fn new((levels, peaks): (&'a [f32], &'a [f32]), theme: &'a Theme) -> Self {
        Self {
            levels,
            peaks,
            theme,
        }
    }
}

/// The value of column `index` out of `columns`: max-pooling when there are
/// at least as many bands as columns, interpolating between neighbors when
/// there are fewer, so no band is ever drawn twice side by side.
fn pooled(values: &[f32], index: usize, columns: usize) -> f32 {
    let bands = values.len();
    if bands == 0 || columns == 0 {
        return 0.0;
    }
    let clean = |value: f32| {
        if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    if bands >= columns {
        let first = index * bands / columns;
        let last = ((index + 1) * bands)
            .div_ceil(columns)
            .clamp(first + 1, bands);
        values[first..last]
            .iter()
            .copied()
            .map(clean)
            .fold(0.0, f32::max)
    } else if columns == 1 {
        clean(values[0])
    } else {
        let position = index as f32 * (bands - 1) as f32 / (columns - 1) as f32;
        let low = position.floor() as usize;
        let high = (low + 1).min(bands - 1);
        let t = position - low as f32;
        clean(values[low]) * (1.0 - t) + clean(values[high]) * t
    }
}

impl Widget for Visualizer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let rows = area.height;
        let columns = usize::from(area.width.div_ceil(2));
        let left = area.x + (area.width - (columns as u16 * 2 - 1)) / 2;
        let theme = self.theme;
        for index in 0..columns {
            let level = pooled(self.levels, index, columns);
            let x = left + index as u16 * 2;
            // Each row is a step of 1/rows; the resting pill is always there.
            let stack = (1 + (level * f32::from(rows)) as u16).min(rows);
            for row in 0..stack {
                let color = if rows == 1 {
                    // A single row still shows loudness through color.
                    if level < 0.2 {
                        theme.glow
                    } else {
                        theme.gradient(level)
                    }
                } else if stack == 1 {
                    theme.glow
                } else {
                    theme.gradient(f32::from(row) / f32::from(rows - 1))
                };
                buf[(x, area.bottom() - 1 - row)]
                    .set_symbol("▬")
                    .set_fg(color);
            }
            if rows > 1 {
                let peak = pooled(self.peaks, index, columns);
                let cap = (peak * f32::from(rows)) as u16;
                if cap >= stack && cap < rows {
                    buf[(x, area.bottom() - 1 - cap)]
                        .set_symbol("▬")
                        .set_fg(if peak >= 0.85 { theme.warn } else { theme.fg });
                }
            }
        }
    }
}

/// `♥ SAVED TO VAULT`: decodes in once with full motion, still otherwise.
fn toast_spans(app: &App, theme: &Theme) -> Vec<Span<'static>> {
    let Some(toast) = app.active_toast() else {
        return Vec::new();
    };
    let text = match fx::progress(app.uptime, toast.since, fx::REVEAL) {
        Some(progress) if app.fx.decorative() => {
            fx::scramble(&toast.text, progress, toast.since.as_millis() as u64)
        }
        _ => toast.text.clone(),
    };
    let color = if toast.good { theme.alert } else { theme.warn };
    vec![Span::styled(
        text,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )]
}

/// How long ago a song was saved: `now`, `5m ago`, `3h ago`, `2d ago`, or
/// the UTC date for older ones.
fn saved_text(clock: i64, saved: i64) -> String {
    let age = clock - saved;
    match age {
        _ if saved <= 0 => "—".to_owned(),
        ..60 => "now".to_owned(),
        60..3600 => format!("{}m ago", age / 60),
        3600..86_400 => format!("{}h ago", age / 3600),
        86_400..604_800 => format!("{}d ago", age / 86_400),
        _ => {
            // Civil date from days since 1970-01-01 (Howard Hinnant).
            let days = saved.div_euclid(86_400) + 719_468;
            let era = days.div_euclid(146_097);
            let doe = days - era * 146_097;
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let day = doy - (153 * mp + 2) / 5 + 1;
            let month = if mp < 10 { mp + 3 } else { mp - 9 };
            let year = yoe + era * 400 + i64::from(month <= 2);
            format!("{year:04}-{month:02}-{day:02}")
        }
    }
}

/// `text` cut or padded to exactly `width` columns, with `…` when cut.
fn fit(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    let total = Span::raw(text).width();
    for character in text.chars() {
        let size = Span::raw(character.to_string()).width();
        let reserve = usize::from(total > width);
        if used + size + reserve > width {
            if total > width && used < width {
                out.push('…');
                used += 1;
            }
            break;
        }
        out.push(character);
        used += size;
    }
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}

/// The track vault: every liked song, filtered and sorted, in place of the
/// station browser.
fn vault(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let songs = app.vault_songs();
    let total = app.settings.songs.len();
    let count = format!(
        "{:02}/{:02}",
        app.vault.list.selected().map_or(0, |index| index + 1),
        songs.len()
    );
    let key = |text: &str| Span::styled(text.to_owned(), Style::default().fg(theme.accent));
    let muted = |text: &str| Span::styled(text.to_owned(), Style::default().fg(theme.muted));
    let dot = || Span::styled(" ▪ ", Style::default().fg(theme.edge));
    let mut hints = vec![Span::styled("━ ", Style::default().fg(theme.edge))];
    if app.vault.filtering {
        hints.extend([
            key("/"),
            Span::styled(app.vault.filter.clone(), Style::default().fg(theme.fg)),
            Span::styled("█", Style::default().fg(theme.accent)),
            dot(),
            key("Enter "),
            muted("keep"),
            dot(),
            key("Esc "),
            muted("clear"),
        ]);
    } else if app.vault.armed.is_some() {
        hints.push(Span::styled(
            "DELETE? x again · Esc cancel",
            Style::default()
                .fg(theme.alert)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        if !app.vault.filter.is_empty() {
            hints.extend([
                key("/"),
                Span::styled(app.vault.filter.clone(), Style::default().fg(theme.fg)),
                dot(),
            ]);
        }
        hints.extend([
            key("Enter "),
            muted("links"),
            dot(),
            key("/ "),
            muted("filter"),
        ]);
        hints.extend([
            dot(),
            key("o "),
            muted("sort"),
            dot(),
            key("x "),
            muted("del"),
        ]);
        hints.extend([dot(), key("v "), muted("back")]);
    }
    hints.push(Span::raw(" "));
    let block = hud("TRACK VAULT", theme)
        .title(notch(
            vec![
                muted("SORT "),
                Span::styled(
                    format!("{} ▾", app.vault.sort.label()),
                    Style::default().fg(theme.accent),
                ),
                dot(),
                muted(&count),
            ],
            theme,
        ))
        .title_bottom(Line::from(hints));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if songs.is_empty() {
        let text = if total == 0 {
            "No saved tracks · l to like what's on air"
        } else {
            "No matches · Esc clear"
        };
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(theme.muted)),
            inner.inner(Margin::new(1, 0)),
        );
        return;
    }
    // One column for the highlight symbol, two for the heart.
    let width = usize::from(inner.width.saturating_sub(3));
    let wide = width >= 64 && inner.height >= 4;
    let (station_width, saved_width) = if wide { (20, 11) } else { (0, 0) };
    let label_width = width.saturating_sub(station_width + saved_width);
    let mut list_area = inner;
    if wide {
        let header = format!(
            "   {}{}{}",
            fit("ARTIST — TITLE", label_width),
            fit("STATION", station_width),
            fit("SAVED", saved_width)
        );
        frame.render_widget(
            Paragraph::new(header).style(Style::default().fg(theme.muted)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        list_area = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
    }
    let armed = app.vault.armed.as_deref();
    let items: Vec<_> = songs
        .iter()
        .map(|song| {
            let doomed = armed == Some(song.key().as_str());
            let mut spans = vec![
                Span::styled("♥ ", Style::default().fg(theme.alert)),
                Span::styled(
                    fit(&song.label(), label_width),
                    Style::default().fg(if doomed { theme.alert } else { theme.fg }),
                ),
            ];
            if wide {
                spans.push(Span::styled(
                    fit(&song.station, station_width.saturating_sub(1)) + " ",
                    Style::default().fg(theme.muted),
                ));
                spans.push(Span::styled(
                    fit(&saved_text(app.clock, song.saved), saved_width),
                    Style::default().fg(theme.muted),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::default().bg(theme.selected))
            .highlight_symbol("▌"),
        list_area,
        &mut app.vault.list,
    );
}

/// One song's links to streaming services, over the vault.
fn link_card(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let Some(song) = app.detail_song() else {
        return;
    };
    let links = song.links();
    let width = 62.min(area.width.saturating_sub(4));
    // Borders, three lines of song, gaps around the links, and the URL.
    let height = (links.len() as u16 + 8).min(area.height);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let key = |text: &str| Span::styled(text.to_owned(), Style::default().fg(theme.accent));
    let muted = |text: &str| Span::styled(text.to_owned(), Style::default().fg(theme.muted));
    let dot = || Span::styled(" ▪ ", Style::default().fg(theme.edge));
    let armed = app.vault.armed.as_deref() == Some(song.key().as_str());
    let hints = if armed {
        Line::from(vec![
            Span::styled("━ ", Style::default().fg(theme.edge)),
            Span::styled(
                "DELETE? x again · Esc cancel ",
                Style::default()
                    .fg(theme.alert)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("━ ", Style::default().fg(theme.edge)),
            key("Enter "),
            muted("open"),
            dot(),
            key("c "),
            muted("copy"),
            dot(),
            key("a "),
            muted("all"),
            dot(),
            key("x "),
            muted("del"),
            dot(),
            key("Esc "),
            muted("back "),
        ])
    };
    let block = hud("TRACK", theme)
        .style(Style::default().bg(theme.bg))
        .title(notch(
            vec![
                Span::styled("♥ ", Style::default().fg(theme.alert)),
                muted(&saved_text(app.clock, song.saved).to_uppercase()),
            ],
            theme,
        ))
        .title_bottom(hints)
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    let room = usize::from(inner.width);
    let selected = app.vault.link.min(links.len() - 1);
    let mut lines = vec![
        Line::styled(
            fit(&song.song, room),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Line::styled(fit(&song.artist, room), Style::default().fg(theme.muted)),
        Line::styled(
            fit(
                &format!(
                    "{} ▪ SAVED {}",
                    song.station.to_uppercase(),
                    saved_text(app.clock, song.saved).to_uppercase()
                ),
                room,
            ),
            Style::default().fg(theme.muted),
        ),
        Line::raw(""),
    ];
    let pending = app.vault.pending.contains(&song.key());
    for (index, link) in links.iter().enumerate() {
        let color = theme.service(link.service);
        let current = index == selected;
        let (tag, tag_color) = if link.direct {
            ("direct", theme.accent)
        } else if link.service == Service::Yandex && pending {
            ("searching…", theme.warn)
        } else {
            ("search", theme.muted)
        };
        let row = Style::default().bg(if current { theme.selected } else { theme.bg });
        let mut name = Style::default().fg(theme.fg);
        if current {
            name = name.add_modifier(Modifier::BOLD);
        }
        let used = 2 + 4 + 2 + 16;
        lines.push(
            Line::from(vec![
                Span::styled(
                    if current { "▌ " } else { "  " },
                    Style::default().fg(color),
                ),
                Span::styled(
                    format!("[{}] ", index + 1),
                    Style::default().fg(theme.accent),
                ),
                Span::styled("↗ ", Style::default().fg(color)),
                Span::styled(fit(link.service.name(), 16), name),
                Span::styled(
                    fit(tag, room.saturating_sub(used)),
                    Style::default().fg(tag_color),
                ),
            ])
            .style(row),
        );
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        fit(&links[selected].url, room),
        Style::default().fg(theme.muted),
    ));
    frame.render_widget(Paragraph::new(lines).block(block), popup);
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
    let height = 21.min(area.height);
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
        ("l", "like / unlike song on air"),
        ("v", "track vault: liked songs"),
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
                scroll.min(17u16.saturating_sub(height.saturating_sub(4))),
                0,
            ))
            .style(Style::default().fg(theme.fg).bg(theme.bg))
            .block(
                Block::default()
                    .title(" Keys ")
                    .title_bottom(if height < 21 {
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
            .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Record  +"));
        // Hearts belong to liked songs: only the header's song count has one.
        assert!(!text.contains('♡'));
        assert_eq!(text.matches('♥').count(), 1);
        assert!(!text.contains("radio-record"));
        app.category.select(Some(1));
        terminal
            .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
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

    fn pill_heights(buffer: &Buffer, area: Rect) -> Vec<u16> {
        (area.left()..area.right())
            .step_by(2)
            .map(|x| {
                (area.top()..area.bottom())
                    .filter(|y| buffer[(x, *y)].symbol() == "▬")
                    .count() as u16
            })
            .collect()
    }

    fn draw_visualizer(width: u16, height: u16, levels: &[f32], peaks: &[f32]) -> Buffer {
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(Visualizer::new((levels, peaks), &theme), frame.area())
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn visualizer_stacks_neon_pills_by_level_with_peak_caps() {
        let theme = Theme::default();
        let levels = [0.0, 0.3, 0.6, 1.0, f32::NAN, -1.0, 7.0];
        let peaks = [0.55, 0.9, 0.6, 1.0, f32::NAN, 0.0, 7.0];
        let buffer = draw_visualizer(13, 4, &levels, &peaks);
        let caps = |x: u16| (0..4).filter(|y| buffer[(x, *y)].symbol() == "▬").count();
        assert_eq!(caps(0), 2, "resting pill plus its floating cap");
        assert_eq!(caps(2), 3, "two pills plus a cap above them");
        assert_eq!(
            pill_heights(&buffer, Rect::new(4, 0, 9, 4)),
            [3, 4, 1, 1, 4]
        );
        // Gaps between columns stay empty.
        assert!((0..4).all(|y| buffer[(1, y)].symbol() == " "));
        // Pills shade from cyan at the bottom to magenta at the top.
        assert_eq!(buffer[(6, 3)].fg, theme.accent);
        assert_eq!(buffer[(6, 0)].fg, theme.alert);
        assert_eq!(buffer[(0, 3)].fg, theme.glow, "a silent column rests dim");
        assert_eq!(buffer[(0, 1)].fg, theme.fg, "a cap below 0.85 is bright");
        assert_eq!(
            buffer[(2, 0)].fg,
            theme.warn,
            "a high cap glows warning yellow"
        );
        assert_eq!(
            buffer[(4, 0)].symbol(),
            " ",
            "a cap inside its column is hidden"
        );
    }

    #[test]
    fn visualizer_pools_or_interpolates_without_repeating_bands() {
        let mut levels = vec![0.0; 96];
        levels[95] = 1.0;
        let buffer = draw_visualizer(9, 3, &levels, &[]);
        assert_eq!(pill_heights(&buffer, buffer.area), [1, 1, 1, 1, 3]);
        let buffer = draw_visualizer(9, 4, &[0.0, 1.0], &[]);
        assert_eq!(
            pill_heights(&buffer, buffer.area),
            [1, 2, 3, 4, 4],
            "few bands spread smoothly across many columns"
        );
        for (width, height) in [(1, 1), (2, 1), (1, 5)] {
            draw_visualizer(width, height, &levels, &levels);
        }
    }

    #[test]
    fn the_visualizer_settles_when_not_playing() {
        let theme = Theme::default();
        let mut app = fixture();
        app.now = app.selected_station().cloned();
        app.playback.state = PlayerState::Paused;
        app.playback.levels = [0.8, 0.6];
        app.playback.spectrum = vec![1.0; 96];
        app.playback.spectrum_peaks = vec![1.0; 96];
        let mut terminal = Terminal::new(TestBackend::new(60, 6)).unwrap();
        terminal
            .draw(|frame| playback(frame, &app, frame.area(), &theme))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let area = Rect::new(0, 0, 60, 3);
        assert!(pill_heights(buffer, area).iter().all(|height| *height == 1));
        for x in 0..60u16 {
            let cell = &buffer[(x, 2)];
            if cell.symbol() == "▬" {
                assert_eq!(cell.fg, theme.glow, "the floor must be at rest");
            }
        }
        app.playback.state = PlayerState::Playing;
        terminal
            .draw(|frame| playback(frame, &app, frame.area(), &theme))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!(pill_heights(buffer, area).iter().all(|height| *height == 3));
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
            .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
            .unwrap();
        let text = cell_text(terminal.backend().buffer());
        assert!(text.contains("· 42%"), "real cache progress must show");
        app.playback.buffering = None;
        terminal
            .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
            .unwrap();
        assert!(!cell_text(terminal.backend().buffer()).contains("42%"));
        // A stale percentage must not leak into other states.
        app.playback.state = PlayerState::Playing;
        app.playback.buffering = Some(42);
        terminal
            .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
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
            .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
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
            .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
            .unwrap();
        let text = cell_text(terminal.backend().buffer());
        assert!(!text.contains("Diagnostics"));
        // The status line drops its telemetry segments; the panel's own HUD
        // line keeps showing the stream format in caps.
        assert!(!text.contains("· 128k"));
        assert!(text.contains("AAC/44.1KHZ STEREO/128K"));
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
                .draw(|frame| render(frame, &mut app, &Theme::default(), &mut Graphics::default()))
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
                .draw(|frame| render(frame, &mut app, &Theme::default(), &mut Graphics::default()))
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
                .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
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
                    time: 0,
                    time_formatted: "12:34".into(),
                    ..Track::default()
                })
                .collect();
            terminal
                .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
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
                .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
                .unwrap();
            app.help = false;
            app.show_diag = true;
            terminal
                .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
                .unwrap();
            app.show_diag = false;
            app.category.select(Some(1));
            terminal
                .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
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
            .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
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

    fn playing_app() -> App {
        let mut app = fixture();
        app.now = app.selected_station().cloned();
        app.playback.state = PlayerState::Playing;
        app.playback.spectrum = (0..48).map(|i| (i as f32 / 47.0).powi(2)).collect();
        app.history = vec![
            crate::app::tests::track(10_000, ""),
            crate::app::tests::track(9_900, ""),
        ];
        app.clock = 10_050;
        app
    }

    #[test]
    fn the_now_playing_panel_appears_when_there_is_room() {
        let theme = Theme::default();
        for (height, cover_rows) in [(40u16, 7u16), (24, 5)] {
            let mut app = playing_app();
            app.cover = Some(crate::cover::tests::cover("https://example.com/a.jpg"));
            let mut terminal = Terminal::new(TestBackend::new(80, height)).unwrap();
            terminal
                .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let text = cell_text(buffer);
            assert!(text.contains("♫ record.sh"), "{height}: window title");
            assert!(text.contains("Song") && text.contains("Artist"));
            assert!(text.contains(" 0:50"), "elapsed time of the track on air");
            assert!(text.contains("|◀") && text.contains("▮▮") && text.contains("▶|"));
            assert!(text.contains("▶  Record"), "status stays visible");
            let art = buffer
                .content()
                .iter()
                .filter(|cell| {
                    // The resize filter may shift a channel by one step.
                    matches!(cell.fg, Color::Rgb(r, g, b) if r > 240 && (110..130).contains(&g) && b < 40)
                        && cell.symbol() == "▀"
                })
                .count();
            assert_eq!(art, usize::from(cover_rows * cover_rows * 2), "{height}");
            assert!(
                buffer
                    .content()
                    .iter()
                    .any(|cell| cell.symbol() == "▬" && cell.fg == theme.alert),
                "the visualizer runs inside the panel"
            );
        }
        let mut app = playing_app();
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
            .unwrap();
        let text = cell_text(terminal.backend().buffer());
        assert!(
            !text.contains(".sh"),
            "short terminals keep the compact footer"
        );
        assert!(text.contains("▬") && text.contains("▶  Record"));
    }

    #[test]
    fn the_panel_shows_a_placeholder_without_art_or_colors() {
        for theme in [Theme::default(), Theme::plain()] {
            let mut app = playing_app();
            if !theme.colorful() {
                app.cover = Some(crate::cover::tests::cover("https://example.com/a.jpg"));
            }
            let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
            terminal
                .draw(|frame| render(frame, &mut app, &theme, &mut Graphics::default()))
                .unwrap();
            let text = cell_text(terminal.backend().buffer());
            assert!(!text.contains("▀"), "no picture is drawn");
            assert!(
                text.matches('♫').count() >= 2,
                "title and placeholder notes"
            );
        }
        let mut app = playing_app();
        app.now = None;
        app.history.clear();
        app.playback = Default::default();
        let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
        terminal
            .draw(|frame| render(frame, &mut app, &Theme::default(), &mut Graphics::default()))
            .unwrap();
        let text = cell_text(terminal.backend().buffer());
        assert!(text.contains("♫ music.sh") && text.contains("Stopped"));
    }

    #[test]
    fn sharp_covers_use_the_terminal_protocol_until_an_overlay_opens() {
        let theme = Theme::default();
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut graphics = Graphics {
            picker: Some(picker),
            cached: None,
        };
        let mut app = playing_app();
        app.cover = Some(crate::cover::tests::cover("https://example.com/a.jpg"));
        let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
        let halfblocks = |terminal: &Terminal<TestBackend>| {
            cell_text(terminal.backend().buffer()).matches('▀').count()
        };
        terminal
            .draw(|frame| render(frame, &mut app, &theme, &mut graphics))
            .unwrap();
        assert_eq!(halfblocks(&terminal), 0, "the picture replaces pixel art");
        let (url, area) = graphics
            .cached
            .as_ref()
            .map(|(url, area, _)| (url.clone(), *area))
            .unwrap();
        assert_eq!(url, "https://example.com/a.jpg");
        app.help = true;
        terminal
            .draw(|frame| render(frame, &mut app, &theme, &mut graphics))
            .unwrap();
        assert!(
            halfblocks(&terminal) > 0,
            "popups cover pixel art, not pictures"
        );
        app.help = false;
        terminal
            .draw(|frame| render(frame, &mut app, &theme, &mut graphics))
            .unwrap();
        assert_eq!(halfblocks(&terminal), 0);
        assert_eq!(
            graphics.cached.as_ref().map(|(_, area, _)| *area),
            Some(area)
        );
    }

    fn render_text(app: &mut App, width: u16, height: u16) -> (String, Buffer) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render(frame, app, &Theme::default(), &mut Graphics::default()))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (cell_text(&buffer), buffer)
    }

    #[test]
    fn the_hud_header_reports_machine_state() {
        let mut app = playing_app();
        let (text, _) = render_text(&mut app, 100, 30);
        assert!(text.contains("RADIO//RECORD") && text.contains("◉ ON AIR"));
        assert!(text.contains("CHANNELS") && text.contains("01/03"));
        assert!(text.contains("◉ LIVE"), "the playing row is tagged");
        app.playback.state = PlayerState::Buffering;
        app.playback.buffering = Some(42);
        assert!(render_text(&mut app, 100, 30).0.contains("◉ SYNC 42%"));
        app.playback = Default::default();
        app.now = None;
        let (text, _) = render_text(&mut app, 100, 30);
        assert!(text.contains("○ STANDBY") && !text.contains("LIVE"));
    }

    #[test]
    fn panel_fits_the_visualizer_beside_or_below_the_details() {
        let wide = Panel::fit(39, 123).unwrap();
        assert_eq!((wide.cover_rows, wide.below_rows, wide.height()), (7, 0, 9));
        let narrow = Panel::fit(23, 60).unwrap();
        assert_eq!(
            (narrow.cover_rows, narrow.below_rows, narrow.height()),
            (5, 2, 9)
        );
        assert_eq!(Panel::fit(19, 200), None);
        let volume: String = volume_meter(65, &Theme::default())
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(volume, "VOL ━━━━━━━─── 65%");
        let mut app = playing_app();
        assert!(
            signal(&app, &Theme::default()).is_empty(),
            "no cache, no meter"
        );
        app.playback.cache_seconds = Some(1.6);
        let signal: String = signal(&app, &Theme::default())
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(signal, "SIG ▂▃▅▇ BUF 1.6s");
    }

    #[test]
    fn a_new_title_decodes_only_with_full_motion() {
        let mut app = playing_app();
        app.track_since = Some((Duration::from_secs(10), 42));
        app.uptime = Duration::from_millis(10_100);
        assert!(
            render_text(&mut app, 100, 30).0.contains("Song"),
            "still when off"
        );
        app.fx = fx::Fx::Full;
        let (text, _) = render_text(&mut app, 100, 30);
        assert!(
            text.contains('█') && !text.contains("Song█"),
            "decoding: {text}"
        );
        app.uptime = Duration::from_secs(11);
        let (text, _) = render_text(&mut app, 100, 30);
        assert!(text.contains("Song") && !text.contains("Song█"), "resolved");
        app.fx = fx::Fx::Calm;
        app.uptime = Duration::from_millis(10_100);
        assert!(render_text(&mut app, 100, 30).0.contains("Song"));
    }

    #[test]
    fn the_frame_glows_with_the_bass_unless_motion_is_off() {
        let theme = Theme::default();
        let mut app = playing_app();
        app.playback.spectrum = vec![1.0; 96];
        let corner = |buffer: &Buffer| {
            buffer
                .content()
                .iter()
                .rev()
                .find(|cell| cell.symbol() == "┛")
                .map(|cell| cell.fg)
                .unwrap()
        };
        let (_, buffer) = render_text(&mut app, 100, 30);
        assert_eq!(corner(&buffer), theme.edge, "off: a still frame");
        app.fx = fx::Fx::Calm;
        let (_, loud) = render_text(&mut app, 100, 30);
        assert_ne!(corner(&loud), theme.edge, "bass lights the frame");
        app.playback.spectrum = vec![0.0; 96];
        let (_, quiet) = render_text(&mut app, 100, 30);
        assert_eq!(corner(&quiet), theme.edge, "silence rests");
    }

    #[test]
    fn buffering_shows_the_real_fill_as_signal_acquiring() {
        let theme = Theme::default();
        let mut app = playing_app();
        app.playback.state = PlayerState::Buffering;
        app.playback.buffering = Some(50);
        assert!(
            !render_text(&mut app, 100, 30)
                .0
                .contains("SIGNAL ACQUIRING")
        );
        app.fx = fx::Fx::Calm;
        let (text, buffer) = render_text(&mut app, 100, 30);
        assert!(text.contains("SIGNAL ACQUIRING 50%"));
        let warn = buffer
            .content()
            .iter()
            .filter(|cell| cell.symbol() == "▬" && cell.fg == theme.warn)
            .count();
        let rest = buffer
            .content()
            .iter()
            .filter(|cell| cell.symbol() == "▬" && cell.fg == theme.glow)
            .count();
        assert!(
            warn > 0 && warn.abs_diff(rest) <= 1,
            "half full: {warn} vs {rest}"
        );
    }

    #[test]
    fn liking_shows_a_toast_and_a_heart() {
        let mut app = playing_app();
        app.uptime = Duration::from_secs(10);
        crate::app::tests::press(&mut app, crossterm::event::KeyCode::Char('l'));
        let (text, _) = render_text(&mut app, 100, 30);
        assert!(text.contains("♥ SAVED TO VAULT"));
        assert!(text.contains("♥ 1"), "the header counts liked songs");
        assert!(!text.contains("VOL "), "the toast takes the volume notch");
        app.uptime += crate::app::TOAST;
        let (text, _) = render_text(&mut app, 100, 30);
        assert!(text.contains("VOL ") && !text.contains("SAVED TO VAULT"));
        let (text, _) = render_text(&mut app, 60, 16);
        assert!(
            text.contains("♥ "),
            "the compact footer marks the liked track"
        );
    }

    #[test]
    fn the_vault_replaces_the_browser_and_the_card_shows_links() {
        let mut app = fixture();
        app.clock = 2_000_000_000;
        for (artist, song) in [("Kölsch", "Grey"), ("Ласковый Май", "Белые розы")]
        {
            app.settings.songs.push(crate::songs::Song {
                artist: artist.into(),
                song: song.into(),
                station: "Deep".into(),
                saved: 2_000_000_000 - 7200,
                ..Default::default()
            });
        }
        crate::app::tests::press(&mut app, crossterm::event::KeyCode::Char('v'));
        for (width, height) in [(100, 30), (44, 12)] {
            let (text, _) = render_text(&mut app, width, height);
            assert!(text.contains("TRACK VAULT"));
            assert!(text.contains("Ласковый Май — Белые розы"));
            assert!(!text.contains("CHANNELS") && !text.contains("SECTOR"));
        }
        let (text, _) = render_text(&mut app, 100, 30);
        assert!(text.contains("SORT NEW ▾") && text.contains("01/02"));
        assert!(text.contains("STATION") && text.contains("2h ago"));
        crate::app::tests::press(&mut app, crossterm::event::KeyCode::Enter);
        for (width, height) in [(100, 30), (44, 12)] {
            let (text, _) = render_text(&mut app, width, height);
            assert!(text.contains("YouTube Music") && text.contains("Spotify"));
            assert!(text.contains("Yandex Music") && text.contains("searching…"));
        }
        app.settings.songs.clear();
        app.vault.detail = None;
        let (text, _) = render_text(&mut app, 100, 30);
        assert!(text.contains("No saved tracks · l to like"));
    }

    #[test]
    fn saved_dates_and_fitted_text() {
        assert_eq!(saved_text(100, 90), "now");
        assert_eq!(saved_text(10_000, 10_000 - 300), "5m ago");
        assert_eq!(saved_text(1_791_720_000, 1_700_000_000), "2023-11-14");
        assert_eq!(saved_text(5, 0), "—");
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("音楽音楽", 5), "音楽…");
        assert_eq!(fit("abc", 0), "");
    }
}
