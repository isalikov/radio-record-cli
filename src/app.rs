use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MediaKeyCode};
use ratatui::widgets::ListState;
use std::{
    collections::{HashSet, VecDeque},
    time::Duration,
};

use crate::{
    api::{Catalog, Station, Stream, Track},
    cover::{Cover, CoverMode},
    player::{PlayerState, Snapshot},
    settings::Settings,
    songs::Song,
};

// Sparkline memory: one sample per redraw, bounded independently of the terminal
// width so a resize never reallocates the history.
const METER_HISTORY: usize = 256;
/// How long a toast stays in the now-playing border.
pub const TOAST: Duration = Duration::from_millis(1600);

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
    /// Open each link in the browser.
    Open(Vec<String>),
    /// Copy a link to the clipboard.
    Copy(String),
    /// Look up a direct Yandex Music link for a saved song.
    Lookup(Song),
}

/// Order of the track vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SongSort {
    #[default]
    Newest,
    Artist,
    Station,
}

impl SongSort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Newest => "NEW",
            Self::Artist => "ARTIST",
            Self::Station => "STATION",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Newest => Self::Artist,
            Self::Artist => Self::Station,
            Self::Station => Self::Newest,
        }
    }
}

/// The track vault screen: saved songs in place of the station browser, and
/// the link card of one song on top of it.
#[derive(Debug, Default)]
pub struct Vault {
    pub open: bool,
    pub list: ListState,
    pub sort: SongSort,
    pub filter: String,
    /// Typing goes into the filter.
    pub filtering: bool,
    /// Key of the song armed for deletion; a second `x` removes it.
    pub armed: Option<String>,
    /// Key of the song whose link card is open, and its selected link.
    pub detail: Option<String>,
    pub link: usize,
    /// Yandex lookups started this session, so a miss is not repeated.
    pub looked_up: HashSet<String>,
    /// Lookups still running.
    pub pending: HashSet<String>,
}

/// A short notice in the now-playing border, e.g. `♥ SAVED TO VAULT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub since: Duration,
    pub text: String,
    /// False for refusals and failures.
    pub good: bool,
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
    /// Cover detail for this session: the saved setting, or
    /// `RADIO_RECORD_COVER` when the main loop finds it set.
    pub cover_mode: CoverMode,
    /// Wall-clock time (Unix seconds) filled by the main loop, for track progress.
    pub clock: i64,
    /// Motion mode, resolved once by the main loop; still (`Off`) in tests.
    pub fx: crate::fx::Fx,
    /// When the track on air last changed (`uptime`) and a seed for its
    /// title decode; set only by `set_history`.
    pub track_since: Option<(Duration, u64)>,
    pub vault: Vault,
    pub toast: Option<Toast>,
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
        let cover_mode = settings.cover;
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
            cover_mode,
            clock: 0,
            fx: crate::fx::Fx::Off,
            track_since: None,
            vault: Vault::default(),
            toast: None,
        }
    }

    /// Show a toast from now; it fades by itself after `TOAST`.
    pub fn notify(&mut self, text: impl Into<String>, good: bool) {
        self.toast = Some(Toast {
            since: self.uptime,
            text: text.into(),
            good,
        });
    }

    /// The toast still on screen, if any.
    pub fn active_toast(&self) -> Option<&Toast> {
        self.toast
            .as_ref()
            .filter(|toast| crate::fx::progress(self.uptime, toast.since, TOAST).is_some())
    }

    /// The song on air, if it is one that can be liked.
    fn song_on_air(&self) -> Option<&Track> {
        self.now.as_ref()?;
        self.history.first().filter(|track| {
            !track.no_fav && !(track.artist.trim().is_empty() && track.song.trim().is_empty())
        })
    }

    /// Whether the song on air is in the vault.
    pub fn on_air_liked(&self) -> bool {
        self.song_on_air().is_some_and(|track| {
            let key = crate::songs::key(&track.artist, &track.song);
            self.settings.songs.iter().any(|song| song.key() == key)
        })
    }

    /// Like or unlike the song on air.
    fn like(&mut self) -> Action {
        let Some(track) = self.song_on_air() else {
            let text = if self.now.is_some() && !self.history.is_empty() {
                "! NOT A SONG"
            } else {
                "! NO TRACK ON AIR"
            };
            self.notify(text, false);
            return Action::None;
        };
        let station = self.now.as_ref().map_or("", |now| now.title.as_str());
        let song = Song::from_track(track, station, self.clock);
        let key = song.key();
        self.dirty = true;
        if let Some(index) = self
            .settings
            .songs
            .iter()
            .position(|saved| saved.key() == key)
        {
            self.settings.songs.remove(index);
            self.notify("♡ REMOVED FROM VAULT", true);
            self.clamp_vault();
            return Action::None;
        }
        self.settings.songs.push(song.clone());
        self.notify("♥ SAVED TO VAULT", true);
        self.clamp_vault();
        self.lookup(song)
    }

    /// Start a Yandex lookup once per song and session.
    fn lookup(&mut self, song: Song) -> Action {
        let key = song.key();
        if !song.yandex_url.is_empty() || !self.vault.looked_up.insert(key.clone()) {
            return Action::None;
        }
        self.vault.pending.insert(key);
        Action::Lookup(song)
    }

    /// Accept a finished Yandex lookup. A failed request (as opposed to no
    /// match) may be retried the next time the song's card opens.
    pub fn set_yandex(&mut self, key: &str, result: crate::error::Result<Option<String>>) {
        self.vault.pending.remove(key);
        let Ok(url) = result else {
            self.vault.looked_up.remove(key);
            return;
        };
        if let Some(url) = url
            && let Some(song) = self
                .settings
                .songs
                .iter_mut()
                .find(|song| song.key() == key)
        {
            song.yandex_url = url;
            self.dirty = true;
        }
    }

    /// Saved songs as the vault shows them: filtered, then sorted.
    pub fn vault_songs(&self) -> Vec<&Song> {
        let filter = self.vault.filter.to_lowercase();
        let mut songs: Vec<&Song> = self
            .settings
            .songs
            .iter()
            .filter(|song| {
                filter.is_empty()
                    || [&song.artist, &song.song, &song.station]
                        .iter()
                        .any(|text| text.to_lowercase().contains(&filter))
            })
            .collect();
        match self.vault.sort {
            SongSort::Newest => songs.reverse(),
            SongSort::Artist => songs.sort_by_cached_key(|song| song.key()),
            SongSort::Station => {
                songs.reverse();
                songs.sort_by_cached_key(|song| song.station.to_lowercase());
            }
        }
        songs
    }

    pub fn selected_song(&self) -> Option<&Song> {
        self.vault_songs().get(self.vault.list.selected()?).copied()
    }

    /// The song whose link card is open.
    pub fn detail_song(&self) -> Option<&Song> {
        let key = self.vault.detail.as_ref()?;
        self.settings.songs.iter().find(|song| song.key() == *key)
    }

    fn clamp_vault(&mut self) {
        let count = self.vault_songs().len();
        clamp_selection(&mut self.vault.list, count);
    }

    fn remove_song(&mut self, key: &str) {
        self.settings.songs.retain(|song| song.key() != key);
        self.dirty = true;
        self.vault.armed = None;
        self.notify("♡ REMOVED FROM VAULT", true);
        self.clamp_vault();
    }

    /// Keys while the vault is open. Playback keys that make sense anywhere
    /// fall through to the main keymap.
    fn vault_key(&mut self, key: KeyEvent) -> Option<Action> {
        if self.vault.detail.is_some() {
            return self.detail_key(key);
        }
        if self.vault.filtering {
            match key.code {
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.vault.filter.push(character);
                    self.vault.list.select(Some(0));
                    self.clamp_vault();
                    return Some(Action::None);
                }
                KeyCode::Backspace => {
                    self.vault.filter.pop();
                    self.clamp_vault();
                    return Some(Action::None);
                }
                KeyCode::Enter => {
                    self.vault.filtering = false;
                    return Some(Action::None);
                }
                KeyCode::Esc => {
                    self.vault.filtering = false;
                    self.vault.filter.clear();
                    self.clamp_vault();
                    return Some(Action::None);
                }
                _ => {}
            }
        }
        let count = self.vault_songs().len();
        let delta = match key.code {
            KeyCode::Up | KeyCode::Char('k') => Some(-1),
            KeyCode::Down | KeyCode::Char('j') => Some(1),
            KeyCode::PageUp => Some(-10),
            KeyCode::PageDown => Some(10),
            KeyCode::Home => Some(isize::MIN),
            KeyCode::End => Some(isize::MAX),
            _ => None,
        };
        if let Some(delta) = delta {
            self.vault.armed = None;
            move_selection(&mut self.vault.list, count, delta);
            return Some(Action::None);
        }
        match key.code {
            KeyCode::Esc if self.vault.armed.is_some() => self.vault.armed = None,
            KeyCode::Esc if !self.vault.filter.is_empty() => {
                self.vault.filter.clear();
                self.clamp_vault();
            }
            KeyCode::Esc | KeyCode::Char('v') => {
                self.vault.open = false;
                self.vault.armed = None;
            }
            KeyCode::Char('/') => self.vault.filtering = true,
            KeyCode::Char('o') => {
                self.vault.sort = self.vault.sort.next();
                self.vault.list.select(Some(0));
                self.clamp_vault();
            }
            KeyCode::Enter => {
                let Some(song) = self.selected_song().cloned() else {
                    return Some(Action::None);
                };
                self.vault.armed = None;
                self.vault.detail = Some(song.key());
                self.vault.link = 0;
                return Some(self.lookup(song));
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                let Some(key) = self.selected_song().map(Song::key) else {
                    return Some(Action::None);
                };
                if self.vault.armed.as_deref() == Some(key.as_str()) {
                    self.remove_song(&key);
                } else {
                    self.vault.armed = Some(key);
                }
            }
            KeyCode::Char('-' | '=' | '+' | ' ' | 's' | 'l' | 'r' | 'q' | '?' | 'd' | 'c') => {
                return None;
            }
            _ => {}
        }
        Some(Action::None)
    }

    /// Keys in a song's link card.
    fn detail_key(&mut self, key: KeyEvent) -> Option<Action> {
        let Some(song) = self.detail_song() else {
            self.vault.detail = None;
            return Some(Action::None);
        };
        let links = song.links();
        let song_key = song.key();
        let open = |app: &mut Self, index: usize| {
            let link = &links[index];
            app.notify(format!("↗ {}", link.service.name().to_uppercase()), true);
            Action::Open(vec![link.url.clone()])
        };
        let action = match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.vault.link = self.vault.link.saturating_sub(1);
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.vault.link = (self.vault.link + 1).min(links.len() - 1);
                Action::None
            }
            KeyCode::Enter => open(self, self.vault.link.min(links.len() - 1)),
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if index < links.len() {
                    self.vault.link = index;
                    open(self, index)
                } else {
                    Action::None
                }
            }
            KeyCode::Char('a') => {
                self.notify("↗ ALL LINKS", true);
                Action::Open(links.iter().map(|link| link.url.clone()).collect())
            }
            KeyCode::Char('c') => {
                self.notify("» LINK COPIED", true);
                Action::Copy(links[self.vault.link.min(links.len() - 1)].url.clone())
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if self.vault.armed.as_deref() == Some(song_key.as_str()) {
                    self.remove_song(&song_key);
                    self.vault.detail = None;
                } else {
                    self.vault.armed = Some(song_key);
                }
                Action::None
            }
            KeyCode::Esc | KeyCode::Backspace => {
                if self.vault.armed.take().is_none() {
                    self.vault.detail = None;
                }
                Action::None
            }
            KeyCode::Char('-' | '=' | '+' | ' ' | 's' | 'l' | 'q') => return None,
            _ => Action::None,
        };
        Some(action)
    }

    /// The cover URL the panel needs but has not requested yet.
    /// The larger cover is preferred so sharp terminal pictures stay crisp.
    pub fn wanted_cover(&self) -> Option<&str> {
        if self.cover_mode == CoverMode::Off {
            return None;
        }
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
                    self.help_scroll = (self.help_scroll + 1).min(17)
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
        if self.vault.open
            && let Some(action) = self.vault_key(key)
        {
            return action;
        }
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('?') => {
                self.help = true;
                self.help_scroll = 0;
            }
            KeyCode::Char('d') => self.show_diag = true,
            KeyCode::Char('a') => self.account.open = true,
            KeyCode::Char('l') => return self.like(),
            KeyCode::Char('v') => {
                self.vault.open = true;
                self.clamp_vault();
            }
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
            KeyCode::Char('c') => {
                self.cover_mode = self.cover_mode.next();
                self.settings.cover = self.cover_mode;
                self.dirty = true;
                self.notify(
                    format!("▣ COVER {}", self.cover_mode.name().to_uppercase()),
                    true,
                );
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
            ..Track::default()
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
    fn c_cycles_the_cover_mode_saves_it_and_off_skips_the_download() {
        let mut app = fixture();
        press(&mut app, KeyCode::Enter);
        let id = app.now.as_ref().unwrap().id;
        app.set_history(id, vec![track(1, "https://example.com/a.jpg")]);
        assert_eq!(app.cover_mode, CoverMode::Pixel);
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(
            (app.cover_mode, app.settings.cover, app.dirty),
            (CoverMode::Fine, CoverMode::Fine, true)
        );
        assert_eq!(app.active_toast().unwrap().text, "▣ COVER FINE");
        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(app.cover_mode, CoverMode::Off);
        assert_eq!(app.wanted_cover(), None, "off downloads nothing");
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(app.cover_mode, CoverMode::Chunky);
        assert_eq!(app.wanted_cover(), Some("https://example.com/a.jpg"));
        press(&mut app, KeyCode::Char('v'));
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(
            app.cover_mode,
            CoverMode::Pixel,
            "the vault list passes c through"
        );
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

    fn on_air(app: &mut App, artist: &str, song: &str) {
        if app.now.is_none() {
            press(app, KeyCode::Enter);
        }
        let id = app.now.as_ref().unwrap().id;
        let mut track = track(100, "");
        track.artist = artist.into();
        track.song = song.into();
        app.set_history(id, vec![track]);
    }

    #[test]
    fn l_likes_and_unlikes_the_song_on_air_only() {
        let mut app = fixture();
        assert!(matches!(press(&mut app, KeyCode::Char('l')), Action::None));
        assert_eq!(app.toast.as_ref().unwrap().text, "! NO TRACK ON AIR");
        assert!(app.settings.songs.is_empty());
        on_air(&mut app, "Armin van Buuren", "Blue Fear");
        app.clock = 1_000;
        press(&mut app, KeyCode::Down); // Browsing does not change what is liked.
        assert!(matches!(
            press(&mut app, KeyCode::Char('l')),
            Action::Lookup(_)
        ));
        assert!(app.on_air_liked() && app.dirty);
        let saved = &app.settings.songs[0];
        assert_eq!((saved.station.as_str(), saved.saved), ("Record", 1_000));
        assert_eq!(app.toast.as_ref().unwrap().text, "♥ SAVED TO VAULT");
        press(&mut app, KeyCode::Char('l'));
        assert!(app.settings.songs.is_empty() && !app.on_air_liked());
        // A second like of the same song starts no second lookup.
        assert!(matches!(press(&mut app, KeyCode::Char('l')), Action::None));
        assert_eq!(app.settings.songs.len(), 1);
        app.history[0].no_fav = true;
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.settings.songs.len(), 1);
        assert_eq!(app.toast.as_ref().unwrap().text, "! NOT A SONG");
    }

    #[test]
    fn toasts_expire() {
        let mut app = fixture();
        app.uptime = Duration::from_secs(3);
        app.notify("hi", true);
        assert!(app.active_toast().is_some());
        app.uptime += TOAST;
        assert!(app.active_toast().is_none());
    }

    fn vault_app() -> App {
        let mut app = fixture();
        for (index, (artist, song, station)) in [
            ("Kölsch", "Grey", "Deep"),
            ("Armin van Buuren", "Blue Fear", "Trance"),
            ("Ласковый Май", "Белые розы", "Гоп FM"),
        ]
        .into_iter()
        .enumerate()
        {
            app.settings.songs.push(Song {
                artist: artist.into(),
                song: song.into(),
                station: station.into(),
                saved: index as i64,
                ..Song::default()
            });
        }
        app
    }

    fn titles(app: &App) -> Vec<String> {
        app.vault_songs()
            .iter()
            .map(|song| song.song.clone())
            .collect()
    }

    #[test]
    fn the_vault_sorts_filters_and_keeps_browsing_separate() {
        let mut app = vault_app();
        press(&mut app, KeyCode::Char('v'));
        assert!(app.vault.open);
        assert_eq!(
            titles(&app),
            ["Белые розы", "Blue Fear", "Grey"],
            "newest first"
        );
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(
            titles(&app),
            ["Blue Fear", "Grey", "Белые розы"],
            "by artist"
        );
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(
            titles(&app),
            ["Grey", "Blue Fear", "Белые розы"],
            "by station"
        );
        press(&mut app, KeyCode::Char('o'));
        for character in "/РОЗ".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        assert_eq!(titles(&app), ["Белые розы"]);
        // While filtering, letters are text, not commands.
        press(&mut app, KeyCode::Char('q'));
        assert!(titles(&app).is_empty());
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Enter);
        assert!(!app.vault.filtering && app.vault.detail.is_none());
        press(&mut app, KeyCode::Esc);
        assert_eq!(titles(&app).len(), 3, "Esc clears the filter first");
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected_song().unwrap().song, "Blue Fear");
        assert_eq!(app.stations.selected(), Some(0), "stations stay put");
        assert!(matches!(
            press(&mut app, KeyCode::Char('=')),
            Action::Volume(_)
        ));
        press(&mut app, KeyCode::Esc);
        assert!(!app.vault.open);
        assert!(app.now.is_none());
    }

    #[test]
    fn vault_deletes_only_on_a_second_press() {
        let mut app = vault_app();
        press(&mut app, KeyCode::Char('v'));
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.settings.songs.len(), 3);
        press(&mut app, KeyCode::Esc);
        assert!(app.vault.open && app.vault.armed.is_none(), "Esc disarms");
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.settings.songs.len(), 3, "moving disarms");
        press(&mut app, KeyCode::Delete);
        assert_eq!(app.settings.songs.len(), 2);
        assert!(
            app.settings
                .songs
                .iter()
                .all(|song| song.song != "Blue Fear")
        );
        assert_eq!(app.vault.list.selected(), Some(1));
        app.settings.songs.clear();
        app.vault.list.select(None);
        assert!(matches!(press(&mut app, KeyCode::Enter), Action::None));
        assert!(app.now.is_none(), "Enter in an empty vault plays nothing");
    }

    #[test]
    fn the_link_card_opens_copies_and_looks_up_once() {
        let mut app = vault_app();
        press(&mut app, KeyCode::Char('v'));
        let Action::Lookup(song) = press(&mut app, KeyCode::Enter) else {
            panic!("opening a card looks up the Yandex link");
        };
        assert_eq!(song.song, "Белые розы");
        assert!(app.vault.pending.contains(&song.key()));
        let Action::Open(urls) = press(&mut app, KeyCode::Enter) else {
            panic!("Enter opens the selected link");
        };
        assert!(urls[0].starts_with("https://music.youtube.com/search?q="));
        let Action::Open(urls) = press(&mut app, KeyCode::Char('2')) else {
            panic!("digits open a service");
        };
        assert!(urls[0].starts_with("https://open.spotify.com/search/"));
        assert_eq!(app.vault.link, 1);
        press(&mut app, KeyCode::Down);
        let Action::Copy(url) = press(&mut app, KeyCode::Char('c')) else {
            panic!("c copies");
        };
        assert!(url.starts_with("https://music.yandex.ru/search?text="));
        let Action::Open(all) = press(&mut app, KeyCode::Char('a')) else {
            panic!("a opens all");
        };
        assert_eq!(all.len(), 4);
        app.set_yandex(&song.key(), Err(crate::error::Error::new("429")));
        assert!(app.vault.pending.is_empty());
        assert!(!app.vault.looked_up.contains(&song.key()), "failures retry");
        app.set_yandex(
            &song.key(),
            Ok(Some("https://music.yandex.ru/album/1/track/2".into())),
        );
        assert!(app.vault.pending.is_empty() && app.dirty);
        let Action::Copy(url) = press(&mut app, KeyCode::Char('c')) else {
            panic!("c copies");
        };
        assert_eq!(url, "https://music.yandex.ru/album/1/track/2");
        press(&mut app, KeyCode::Esc);
        assert!(app.vault.detail.is_none() && app.vault.open);
        assert!(
            matches!(press(&mut app, KeyCode::Enter), Action::None),
            "no second lookup"
        );
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.settings.songs.len(), 2);
        assert!(app.vault.detail.is_none());
    }
}
