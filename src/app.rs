use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MediaKeyCode};
use ratatui::widgets::ListState;
use std::{collections::VecDeque, time::Duration};

use crate::{
    api::{Catalog, Station, Stream, Track},
    cover::Cover,
    player::{PlayerState, Snapshot},
    settings::Settings,
};

// Sparkline memory: one sample per redraw, bounded independently of the terminal
// width so a resize never reallocates the history.
const METER_HISTORY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Category {
    All,
    Favorites,
    Genre(i64, String),
}

impl Category {
    pub fn title(&self) -> &str {
        match self {
            Self::All => "All",
            Self::Favorites => "Favorites",
            Self::Genre(_, name) => match name.as_str() {
                "Лето" => "Summer",
                "Новый год" => "New Year",
                "Прикол" => "Fun",
                "Радиошоу" => "Radio Shows",
                "Сон" => "Sleep",
                "Спорт" => "Sport",
                "Эпохи" => "Eras",
                _ => name,
            },
        }
    }
}

pub enum Action {
    None,
    Quit,
    Play(Vec<Stream>),
    Pause,
    Stop,
    Volume(u8),
    Refresh,
    Account(crate::account::Command),
}

pub struct App {
    pub account: crate::account::Account,
    pub catalog: Option<Catalog>,
    pub categories: Vec<Category>,
    pub category: ListState,
    pub stations: ListState,
    pub settings: Settings,
    pub dirty: bool,
    pub help: bool,
    pub help_scroll: u16,
    pub show_history: bool,
    pub show_diag: bool,
    pub generation: u64,
    pub uptime: Duration,
    pub history_age: Option<Duration>,
    pub worker_stalled: bool,
    pub meter_history: VecDeque<f64>,
    pub now: Option<Station>,
    pub history: Vec<Track>,
    pub history_loading: bool,
    pub history_error: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
    pub playback: Snapshot,
    /// Art for the track on air; `cover_url` is the one requested last, so a
    /// late download for an earlier track is dropped instead of shown.
    pub cover: Option<Cover>,
    pub cover_url: Option<String>,
    /// Wall-clock time (Unix seconds) filled by the main loop, for track progress.
    pub clock: i64,
    /// Motion mode, resolved once by the main loop; still (`Off`) in tests.
    pub fx: crate::fx::Fx,
    /// When the track on air last changed (`uptime`) and a seed for its
    /// title decode; set only by `set_history`.
    pub track_since: Option<(Duration, u64)>,
}

/// How far the track on air is, from its start time and the station's
/// recent track lengths. Live radio has no duration, so the fraction is an
/// estimate and is absent until the history shows a typical length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub elapsed: i64,
    pub fraction: Option<f64>,
}

impl App {
    pub fn new(settings: Settings) -> Self {
        Self {
            account: crate::account::Account::default(),
            catalog: None,
            categories: vec![Category::All, Category::Favorites],
            category: ListState::default().with_selected(Some(0)),
            stations: ListState::default(),
            settings,
            dirty: false,
            help: false,
            help_scroll: 0,
            show_history: true,
            show_diag: false,
            generation: 0,
            uptime: Duration::ZERO,
            history_age: None,
            worker_stalled: false,
            meter_history: VecDeque::new(),
            now: None,
            history: Vec::new(),
            history_loading: false,
            history_error: None,
            loading: true,
            error: None,
            playback: Snapshot::default(),
            cover: None,
            cover_url: None,
            clock: 0,
            fx: crate::fx::Fx::Off,
            track_since: None,
        }
    }

    /// The cover URL the panel needs but has not requested yet.
    /// The larger cover is preferred so sharp terminal pictures stay crisp.
    pub fn wanted_cover(&self) -> Option<&str> {
        let track = self.history.first()?;
        let url = [track.image200.as_str(), track.image100.as_str()]
            .into_iter()
            .find(|url| url.starts_with("https://") || url.starts_with("http://"))?;
        (self.cover_url.as_deref() != Some(url)).then_some(url)
    }

    /// Accept a finished cover download if it is still the one wanted.
    pub fn set_cover(&mut self, url: &str, cover: Option<Cover>) {
        if self.cover_url.as_deref() == Some(url) {
            self.cover = cover;
        }
    }

    fn clear_cover(&mut self) {
        self.cover = None;
        self.cover_url = None;
    }

    pub fn progress(&self) -> Option<Progress> {
        let start = self.history.first().map(|track| track.time)?;
        let elapsed = self.clock - start;
        if start <= 0 || !(0..6 * 3600).contains(&elapsed) {
            return None;
        }
        // Median gap between recent track starts, ignoring jingles and shows.
        let mut lengths: Vec<i64> = self
            .history
            .windows(2)
            .take(12)
            .map(|pair| pair[0].time - pair[1].time)
            .filter(|length| (30..=900).contains(length))
            .collect();
        lengths.sort_unstable();
        let fraction = (!lengths.is_empty())
            .then(|| (elapsed as f64 / lengths[lengths.len() / 2] as f64).min(1.0));
        Some(Progress { elapsed, fraction })
    }

    pub fn set_worker_stalled(&mut self, stalled: bool) {
        const MESSAGE: &str = "Player engine stalled · q quit";
        self.worker_stalled = stalled;
        if stalled {
            self.error = Some(MESSAGE.into());
        } else if self.error.as_deref() == Some(MESSAGE) {
            self.error = None;
        }
    }

    pub fn set_catalog(&mut self, catalog: Catalog) {
        let previous = self.current_category().clone();
        self.categories = vec![Category::All, Category::Favorites];
        // Include genres found on stations even if omitted from the top-level API list.
        for genre in catalog
            .genres
            .iter()
            .chain(catalog.stations.iter().flat_map(|station| &station.genres))
        {
            if !self
                .categories
                .iter()
                .any(|category| matches!(category, Category::Genre(id, _) if *id == genre.id))
                && catalog
                    .stations
                    .iter()
                    .any(|station| station.genres.iter().any(|g| g.id == genre.id))
            {
                self.categories
                    .push(Category::Genre(genre.id, genre.name.clone()));
            }
        }
        self.category.select(Some(
            self.categories
                .iter()
                .position(|category| *category == previous)
                .unwrap_or(0),
        ));
        self.catalog = Some(catalog);
        self.loading = false;
        self.error = None;
        self.reset_stations();
    }

    pub fn current_category(&self) -> &Category {
        &self.categories[self.category.selected().unwrap_or(0)]
    }

    fn matches_category(&self, station: &Station, category: &Category) -> bool {
        match category {
            Category::All => true,
            Category::Favorites => self.settings.favorites.contains(&station.id),
            Category::Genre(id, _) => station.genres.iter().any(|genre| genre.id == *id),
        }
    }

    pub fn visible(&self) -> Vec<&Station> {
        self.catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .stations
                    .iter()
                    .filter(|station| self.matches_category(station, self.current_category()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn selected_station(&self) -> Option<&Station> {
        self.visible().get(self.stations.selected()?).copied()
    }

    fn reset_stations(&mut self) {
        self.stations = ListState::default().with_selected(if self.visible().is_empty() {
            None
        } else {
            Some(0)
        });
    }

    pub fn replace_favorites(&mut self, favorites: std::collections::BTreeSet<i64>) {
        if self.settings.favorites != favorites {
            self.settings.favorites = favorites;
            self.dirty = true;
            let count = self.visible().len();
            clamp_selection(&mut self.stations, count);
        }
    }

    pub fn set_history(&mut self, station_id: i64, history: Vec<Track>) {
        if self.now.as_ref().map(|station| station.id) != Some(station_id) {
            return;
        }
        let on_air = |track: &Track| (track.time, track.song.clone());
        let previous = self.history.first().map(on_air);
        if let Some(current) = history.first().map(on_air)
            && previous.as_ref() != Some(&current)
        {
            let seed = crate::fx::mix(current.0 as u64, station_id as u64, current.1.len() as u64);
            self.track_since = Some((self.uptime, seed));
        }
        self.history = history;
        self.history_loading = false;
        self.history_error = None;
    }

    /// One level sample per redraw. The sparkline is a recording of worker data,
    /// never a second source of truth, and never grows past `METER_HISTORY`.
    pub fn push_meter_sample(&mut self) {
        let level = if self.playback.state == PlayerState::Playing {
            self.playback.levels[0].max(self.playback.levels[1])
        } else {
            0.0
        };
        self.meter_history.push_back(level);
        while self.meter_history.len() > METER_HISTORY {
            self.meter_history.pop_front();
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::Quit;
        }
        let transport = match key.code {
            KeyCode::F(7) | KeyCode::Media(MediaKeyCode::TrackPrevious) => Some(-1),
            KeyCode::F(9) | KeyCode::Media(MediaKeyCode::TrackNext) => Some(1),
            KeyCode::F(8) | KeyCode::Media(MediaKeyCode::PlayPause) => Some(0),
            _ => None,
        };
        if let Some(direction) = transport {
            if key.kind != KeyEventKind::Press {
                return Action::None;
            }
            return if direction == 0 {
                self.play_pause()
            } else {
                self.skip_station(direction)
            };
        }
        match key.code {
            KeyCode::Media(MediaKeyCode::Play) => {
                return if key.kind != KeyEventKind::Press
                    || matches!(
                        self.playback.state,
                        PlayerState::Playing | PlayerState::Buffering
                    ) {
                    Action::None
                } else {
                    self.play_pause()
                };
            }
            KeyCode::Media(MediaKeyCode::Pause) => {
                return if key.kind == KeyEventKind::Press
                    && matches!(
                        self.playback.state,
                        PlayerState::Playing | PlayerState::Buffering
                    ) {
                    Action::Pause
                } else {
                    Action::None
                };
            }
            _ => {}
        }
        if self.account.open {
            return self.account.key(key).map_or(Action::None, Action::Account);
        }
        if self.help {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = (self.help_scroll + 1).min(14)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                _ => {}
            }
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                self.help = false;
            } else if key.code == KeyCode::Char('q') {
                return Action::Quit;
            }
            return Action::None;
        }
        if self.show_diag {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('?')
            ) {
                self.show_diag = false;
            } else if key.code == KeyCode::Char('q') {
                return Action::Quit;
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('?') => {
                self.help = true;
                self.help_scroll = 0;
            }
            KeyCode::Char('d') => self.show_diag = true,
            KeyCode::Char('a') => self.account.open = true,
            KeyCode::Esc => self.error = None,
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => self.change_category(-1),
            KeyCode::Tab => self.change_category(1),
            KeyCode::BackTab => self.change_category(-1),
            KeyCode::Up | KeyCode::Char('k') => self.navigate(-1),
            KeyCode::Down | KeyCode::Char('j') => self.navigate(1),
            KeyCode::PageUp => self.navigate(-10),
            KeyCode::PageDown => self.navigate(10),
            KeyCode::Home => self.navigate(isize::MIN),
            KeyCode::End => self.navigate(isize::MAX),
            KeyCode::Char('i') => {
                self.show_history = !self.show_history;
            }
            KeyCode::Char('f') => {
                if let Some(id) = self.selected_station().map(|station| station.id) {
                    if !self.settings.favorites.remove(&id) {
                        self.settings.favorites.insert(id);
                    }
                    self.dirty = true;
                    let count = self.visible().len();
                    clamp_selection(&mut self.stations, count);
                }
            }
            KeyCode::Char('-') | KeyCode::Char('=') | KeyCode::Char('+') => {
                self.settings.volume = if key.code == KeyCode::Char('-') {
                    self.settings.volume.saturating_sub(2)
                } else {
                    self.settings.volume.saturating_add(2).min(100)
                };
                self.dirty = true;
                return Action::Volume(self.settings.volume);
            }
            KeyCode::Char('r') => return Action::Refresh,
            KeyCode::Char('s') => {
                self.now = None;
                self.clear_cover();
                self.history.clear();
                self.history_error = None;
                self.history_loading = false;
                self.playback = Snapshot::default();
                return Action::Stop;
            }
            KeyCode::Char(' ') => return self.play_pause(),
            KeyCode::Enter => return self.play_selected(),
            _ => {}
        }
        Action::None
    }

    pub fn play_pause(&mut self) -> Action {
        if self.now.is_some()
            && matches!(
                self.playback.state,
                PlayerState::Playing | PlayerState::Paused | PlayerState::Buffering
            )
        {
            Action::Pause
        } else {
            self.play_selected()
        }
    }

    pub fn skip_station(&mut self, direction: isize) -> Action {
        let visible = self.visible();
        if visible.is_empty() {
            return Action::None;
        }
        let current = self
            .now
            .as_ref()
            .and_then(|now| visible.iter().position(|station| station.id == now.id));
        let start = current
            .map(|index| (index as isize + direction).rem_euclid(visible.len() as isize) as usize)
            .unwrap_or(self.stations.selected().unwrap_or(0).min(visible.len() - 1));
        let target = (0..visible.len())
            .map(|step| {
                (start as isize + direction * step as isize).rem_euclid(visible.len() as isize)
                    as usize
            })
            .find(|index| visible[*index].stream_url().is_some());
        if let Some(index) = target {
            self.stations.select(Some(index));
            self.play_selected()
        } else {
            Action::None
        }
    }

    fn play_selected(&mut self) -> Action {
        let Some(station) = self.selected_station().cloned() else {
            return Action::None;
        };
        let streams = station.streams();
        if streams.is_empty() {
            self.error = Some("No stream available".into());
            return Action::None;
        }
        self.now = Some(station);
        self.clear_cover();
        self.history.clear();
        self.history_error = None;
        self.history_loading = true;
        self.error = None;
        self.playback = Snapshot {
            state: PlayerState::Buffering,
            ..Snapshot::default()
        };
        Action::Play(streams)
    }

    fn change_category(&mut self, direction: isize) {
        let index = (self.category.selected().unwrap_or(0) as isize + direction)
            .rem_euclid(self.categories.len() as isize) as usize;
        self.category.select(Some(index));
        self.reset_stations();
    }

    fn navigate(&mut self, delta: isize) {
        let count = self.visible().len();
        move_selection(&mut self.stations, count, delta);
    }
}

fn clamp_selection(state: &mut ListState, count: usize) {
    state.select(if count == 0 {
        None
    } else {
        Some(state.selected().unwrap_or(0).min(count - 1))
    });
}

fn move_selection(state: &mut ListState, count: usize, delta: isize) {
    if count == 0 {
        state.select(None);
        return;
    }
    let selected = state
        .selected()
        .unwrap_or(0)
        .saturating_add_signed(delta)
        .min(count - 1);
    state.select(Some(selected));
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::api::Genre;

    pub fn fixture() -> App {
        let genre = Genre {
            id: 7,
            name: "Электроника".into(),
        };
        let stations = ["Record", "Синтвейв", "Techno"]
            .iter()
            .enumerate()
            .map(|(i, title)| Station {
                id: i as i64 + 1,
                title: title.to_string(),
                prefix: title.to_lowercase(),
                tooltip: String::new(),
                stream_320: "https://example.com/radio".into(),
                stream_64: String::new(),
                stream_128: String::new(),
                stream_hls: String::new(),
                icon_fill: String::new(),
                genres: if i > 0 { vec![genre.clone()] } else { vec![] },
            })
            .collect();
        let mut app = App::new(Settings::default());
        app.set_catalog(Catalog {
            stations,
            genres: vec![genre],
            tags: vec![],
        });
        app
    }
    pub fn press(app: &mut App, code: KeyCode) -> Action {
        app.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn recovered_worker_clears_stall_message_without_clearing_other_errors() {
        let mut app = fixture();
        app.set_worker_stalled(true);
        assert!(app.worker_stalled);
        assert_eq!(app.error.as_deref(), Some("Player engine stalled · q quit"));
        app.set_worker_stalled(false);
        assert!(!app.worker_stalled);
        assert!(app.error.is_none());
        app.error = Some("catalog unavailable".into());
        app.set_worker_stalled(false);
        assert_eq!(app.error.as_deref(), Some("catalog unavailable"));
    }

    #[test]
    fn tab_changes_categories_and_arrows_only_select_stations() {
        let mut app = fixture();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('f'));
        press(&mut app, KeyCode::Tab);
        assert_eq!(*app.current_category(), Category::Favorites);
        assert_eq!(app.selected_station().unwrap().id, 2);
        for key in [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right] {
            press(&mut app, key);
            assert_eq!(*app.current_category(), Category::Favorites);
        }
        press(&mut app, KeyCode::Char('f'));
        assert!(app.selected_station().is_none());
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.visible().len(), 2);
        press(&mut app, KeyCode::Tab);
        assert_eq!(*app.current_category(), Category::All);
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.visible().len(), 2);
        assert!(app.now.is_none());
    }

    #[test]
    fn media_keys_control_playback_and_wrap_within_the_category() {
        let mut app = fixture();
        assert!(matches!(press(&mut app, KeyCode::F(8)), Action::Play(_)));
        app.playback.state = PlayerState::Playing;
        assert!(matches!(press(&mut app, KeyCode::F(8)), Action::Pause));
        press(&mut app, KeyCode::Down); // Browse independently of the playing station.
        press(&mut app, KeyCode::F(9));
        assert_eq!(app.now.as_ref().unwrap().id, 2);
        press(&mut app, KeyCode::F(7));
        assert_eq!(app.now.as_ref().unwrap().id, 1);
        press(&mut app, KeyCode::Media(MediaKeyCode::TrackPrevious));
        assert_eq!(app.now.as_ref().unwrap().id, 3);
        press(&mut app, KeyCode::Media(MediaKeyCode::TrackNext));
        assert_eq!(app.now.as_ref().unwrap().id, 1);
        app.help = true;
        assert!(matches!(
            press(&mut app, KeyCode::Media(MediaKeyCode::PlayPause)),
            Action::Pause
        ));
        let mut repeat = KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE);
        repeat.kind = KeyEventKind::Repeat;
        assert!(matches!(app.key(repeat), Action::None));
    }

    #[test]
    fn separate_media_play_and_pause_are_idempotent_including_buffering() {
        let mut app = fixture();
        let play = KeyCode::Media(MediaKeyCode::Play);
        let pause = KeyCode::Media(MediaKeyCode::Pause);
        assert!(matches!(press(&mut app, play), Action::Play(_)));
        assert_eq!(app.playback.state, PlayerState::Buffering);
        assert!(matches!(press(&mut app, play), Action::None));
        assert!(matches!(press(&mut app, pause), Action::Pause));
        app.playback.state = PlayerState::Paused;
        assert!(matches!(press(&mut app, pause), Action::None));
        assert!(matches!(press(&mut app, play), Action::Pause));
        app.playback.state = PlayerState::Playing;
        app.help = true;
        assert!(matches!(press(&mut app, play), Action::None));
        assert!(matches!(press(&mut app, pause), Action::Pause));
        for code in [play, pause] {
            let mut repeat = KeyEvent::new(code, KeyModifiers::NONE);
            repeat.kind = KeyEventKind::Repeat;
            assert!(matches!(app.key(repeat), Action::None));
            repeat.kind = KeyEventKind::Release;
            assert!(matches!(app.key(repeat), Action::None));
        }
    }

    #[test]
    fn transport_handles_empty_categories_and_missing_streams() {
        let mut app = fixture();
        press(&mut app, KeyCode::Tab);
        for code in [KeyCode::F(7), KeyCode::F(8), KeyCode::F(9)] {
            assert!(matches!(press(&mut app, code), Action::None));
        }
        press(&mut app, KeyCode::Tab); // Genre contains stations 2 and 3.
        app.catalog.as_mut().unwrap().stations[1].stream_320.clear();
        press(&mut app, KeyCode::F(9));
        assert_eq!(app.now.as_ref().unwrap().id, 3);
    }

    #[test]
    fn browsing_and_stale_history_never_change_playing_station() {
        let mut app = fixture();
        assert!(matches!(press(&mut app, KeyCode::Enter), Action::Play(_)));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.now.as_ref().unwrap().id, 1);
        app.set_history(2, vec![]);
        assert!(app.history_loading);
        app.set_history(1, vec![]);
        assert!(!app.history_loading);
        assert_eq!(app.selected_station().unwrap().id, 2);
    }

    #[test]
    fn volume_is_bounded_and_search_is_removed() {
        let mut app = fixture();
        app.settings.volume = 65;
        assert!(matches!(
            press(&mut app, KeyCode::Char('=')),
            Action::Volume(67)
        ));
        assert!(matches!(
            press(&mut app, KeyCode::Char('-')),
            Action::Volume(65)
        ));
        for _ in 0..60 {
            press(&mut app, KeyCode::Char('='));
        }
        assert_eq!(app.settings.volume, 100);
        for _ in 0..60 {
            press(&mut app, KeyCode::Char('-'));
        }
        assert_eq!(app.settings.volume, 0);
        press(&mut app, KeyCode::Char('/'));
        assert!(matches!(press(&mut app, KeyCode::Char('q')), Action::Quit));
    }

    #[test]
    fn diag_overlay_opens_on_d_and_closes_on_esc() {
        let mut app = fixture();
        assert!(!app.show_diag);
        press(&mut app, KeyCode::Char('d'));
        assert!(app.show_diag);
        assert!(matches!(press(&mut app, KeyCode::Char('q')), Action::Quit));
        press(&mut app, KeyCode::Char('d'));
        assert!(!app.show_diag, "'d' closes the overlay");
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Esc);
        assert!(!app.show_diag, "Esc closes the overlay");
        // Transport keys keep working while the overlay is open.
        press(&mut app, KeyCode::Enter);
        app.playback.state = PlayerState::Playing;
        app.show_diag = true;
        assert!(matches!(press(&mut app, KeyCode::F(8)), Action::Pause));
    }

    #[test]
    fn meter_history_is_bounded_and_records_only_playback() {
        let mut app = fixture();
        for _ in 0..1000 {
            app.push_meter_sample();
        }
        assert_eq!(app.meter_history.len(), METER_HISTORY);
        assert!(app.meter_history.iter().all(|level| *level == 0.0));
        app.playback.state = PlayerState::Playing;
        app.playback.levels = [0.7, 0.4];
        app.push_meter_sample();
        assert_eq!(*app.meter_history.back().unwrap(), 0.7);
    }

    pub fn track(time: i64, image: &str) -> Track {
        Track {
            id: time,
            artist: "Artist".into(),
            song: "Song".into(),
            image100: image.into(),
            image200: String::new(),
            time,
            time_formatted: String::new(),
        }
    }

    #[test]
    fn progress_follows_the_stations_typical_track_length() {
        let mut app = fixture();
        assert_eq!(app.progress(), None);
        // Tracks started 120, 100 (a jingle gap is ignored), and 110 seconds apart.
        app.history = vec![
            track(10_000, ""),
            track(9_880, ""),
            track(9_870, ""),
            track(9_770, ""),
            track(9_660, ""),
        ];
        app.clock = 10_055;
        let progress = app.progress().unwrap();
        assert_eq!(progress.elapsed, 55);
        assert_eq!(progress.fraction, Some(0.5), "median of 100, 110, 120");
        app.clock = 11_000;
        assert_eq!(
            app.progress().unwrap().fraction,
            Some(1.0),
            "never past full"
        );
        app.history.truncate(1);
        assert_eq!(
            app.progress().unwrap().fraction,
            None,
            "no typical length yet"
        );
        app.clock = 9_000;
        assert_eq!(
            app.progress(),
            None,
            "a clock behind the start is not progress"
        );
        app.history[0].time = 0;
        assert_eq!(app.progress(), None, "unknown start time");
    }

    #[test]
    fn covers_are_requested_once_and_late_downloads_are_dropped() {
        let mut app = fixture();
        press(&mut app, KeyCode::Enter);
        let id = app.now.as_ref().unwrap().id;
        assert_eq!(app.wanted_cover(), None, "no track yet");
        app.set_history(id, vec![track(1, "https://example.com/a.jpg")]);
        assert_eq!(app.wanted_cover(), Some("https://example.com/a.jpg"));
        app.cover_url = Some("https://example.com/a.jpg".into());
        assert_eq!(app.wanted_cover(), None, "already requested");
        app.set_history(id, vec![track(2, "https://example.com/b.jpg")]);
        app.cover_url = Some("https://example.com/b.jpg".into());
        let late = crate::cover::tests::cover("https://example.com/a.jpg");
        app.set_cover("https://example.com/a.jpg", Some(late));
        assert_eq!(app.cover, None, "the previous track's art must not appear");
        let fresh = crate::cover::tests::cover("https://example.com/b.jpg");
        app.set_cover("https://example.com/b.jpg", Some(fresh.clone()));
        assert_eq!(app.cover, Some(fresh));
        press(&mut app, KeyCode::Char('s'));
        assert_eq!((app.cover.as_ref(), app.cover_url.as_ref()), (None, None));
        app.history = vec![track(3, "data:image/png;base64,AAAA")];
        assert_eq!(app.wanted_cover(), None, "only http(s) covers are fetched");
        app.history[0].image200 = "https://example.com/c200.jpg".into();
        assert_eq!(
            app.wanted_cover(),
            Some("https://example.com/c200.jpg"),
            "the larger cover is preferred"
        );
    }

    #[test]
    fn a_new_track_on_air_starts_its_title_reveal_once() {
        let mut app = fixture();
        press(&mut app, KeyCode::Enter);
        let id = app.now.as_ref().unwrap().id;
        app.uptime = Duration::from_secs(5);
        app.set_history(id, vec![track(100, "")]);
        let (since, seed) = app.track_since.unwrap();
        assert_eq!(since, Duration::from_secs(5));
        app.uptime = Duration::from_secs(20);
        app.set_history(id, vec![track(100, ""), track(40, "")]);
        assert_eq!(
            app.track_since,
            Some((since, seed)),
            "same track, no replay"
        );
        app.set_history(id + 1, vec![track(200, "")]);
        assert_eq!(
            app.track_since,
            Some((since, seed)),
            "stale station ignored"
        );
        app.set_history(id, vec![track(200, ""), track(100, "")]);
        assert_eq!(app.track_since.unwrap().0, Duration::from_secs(20));
    }
}
