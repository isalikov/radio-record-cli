mod api;
mod app;
mod engine;
mod error;
mod json;
mod media;
mod player;
mod settings;
mod ui;

use api::{Catalog, Client, Track};
use app::{Action, App};
use crossterm::event::{self, Event, KeyEvent, KeyEventKind, KeyModifiers};
use error::{Error, Result};
use player::Player;
use settings::Settings;
use std::{
    io::{self, IsTerminal},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

fn main() {
    if let Err(err) = run() {
        eprintln!("radiome: {err}");
        std::process::exit(1);
    }
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

fn fetch<T: Send + 'static>(
    job: impl FnOnce() -> Result<T> + Send + 'static,
) -> Receiver<Result<T>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(job());
    });
    rx
}

struct Network {
    catalog: Option<Receiver<Result<Catalog>>>,
    history: Option<(i64, Receiver<Result<Vec<Track>>>)>,
    history_updated: Instant,
}

impl Network {
    fn new() -> Self {
        Self {
            catalog: None,
            history: None,
            history_updated: Instant::now(),
        }
    }
    fn catalog(&mut self, app: &mut App) {
        if self.catalog.is_none() {
            app.loading = true;
            self.catalog = Some(fetch(|| Client::new().get_catalog()));
        }
    }
    fn history(&mut self, app: &mut App) {
        if let Some(station) = &app.now {
            let id = station.id;
            if self
                .history
                .as_ref()
                .is_some_and(|(pending, _)| *pending == id)
            {
                return;
            }
            app.history_loading = true;
            self.history = Some((id, fetch(move || Client::new().get_history(id, 50))));
            self.history_updated = Instant::now();
        }
    }
    fn poll(&mut self, app: &mut App) {
        if let Some(rx) = &self.catalog {
            match rx.try_recv() {
                Ok(result) => {
                    app.loading = false;
                    match result {
                        Ok(catalog) => app.set_catalog(catalog),
                        Err(_) => app.error = Some("Stations unavailable · r retry".into()),
                    }
                    self.catalog = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    app.loading = false;
                    app.error = Some("Loading interrupted · r retry".into());
                    self.catalog = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((id, rx)) = &self.history {
            match rx.try_recv() {
                Ok(result) => {
                    if app.now.as_ref().is_some_and(|station| station.id == *id) {
                        app.history_loading = false;
                        match result {
                            Ok(history) => app.set_history(*id, history),
                            Err(_) => {
                                app.history_error = Some("History unavailable · r retry".into())
                            }
                        }
                    }
                    self.history = None;
                    self.history_updated = Instant::now();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.history = None;
                    app.history_loading = false;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.history.is_none() && self.history_updated.elapsed() >= Duration::from_secs(15) {
            self.history(app);
        }
    }
}

fn run() -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(Error::new("Open a terminal and run make run"));
    }
    let settings_path = Settings::path()?;
    let settings = Settings::load(&settings_path)?;
    let mut player = Player::new(settings.volume);
    let mut app = App::new(settings);
    let mut media = media::MediaControls::new()?;
    let mut network = Network::new();
    network.catalog(&mut app);
    let theme = ui::theme_from_env();
    let mut terminal = ratatui::try_init()?;
    let _guard = TerminalGuard;
    let mut last_frame = Instant::now();
    let mut last_save = Instant::now();
    let started = Instant::now();
    loop {
        network.poll(&mut app);
        player.update();
        app.playback = player.snapshot.clone();
        app.generation = player.generation();
        app.set_worker_stalled(player.worker_stalled());
        app.uptime = started.elapsed();
        app.history_age = if app.now.is_some() {
            Some(network.history_updated.elapsed())
        } else {
            None
        };
        for direction in player.media_commands() {
            if let Action::Play(urls) = app.skip_station(direction) {
                player.play(urls);
                network.history(&mut app);
            }
        }
        media.update(&app);
        for key in media.poll() {
            let action = app.key(KeyEvent::new(key, KeyModifiers::NONE));
            dispatch(action, &mut app, &mut player, &mut network);
        }
        if last_frame.elapsed() >= Duration::from_millis(50) {
            app.push_meter_sample();
            terminal.draw(|frame| ui::render(frame, &mut app, &theme))?;
            last_frame = Instant::now();
        }
        if app.dirty && last_save.elapsed() >= Duration::from_millis(500) {
            if let Err(err) = app.settings.save(&settings_path) {
                app.error = Some(format!("Could not save settings: {err}"));
            } else {
                app.dirty = false;
            }
            last_save = Instant::now();
        }
        if event::poll(Duration::from_millis(10))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let action = app.key(key);
                    if matches!(action, Action::Quit) {
                        break;
                    }
                    dispatch(action, &mut app, &mut player, &mut network);
                }
                Event::Resize(_, _) => {
                    terminal.draw(|frame| ui::render(frame, &mut app, &theme))?;
                }
                _ => {}
            }
        }
    }
    if app.dirty {
        app.settings.save(&settings_path)?;
    }
    Ok(())
}

fn dispatch(action: Action, app: &mut App, player: &mut Player, network: &mut Network) {
    match action {
        Action::Play(urls) => {
            player.play(urls);
            network.history(app);
        }
        Action::Pause => player.toggle_pause(),
        Action::Stop => {
            player.stop();
            network.history = None;
        }
        Action::Volume(value) => player.set_volume(value),
        Action::Refresh => {
            network.catalog(app);
            network.history(app);
        }
        Action::None | Action::Quit => {}
    }
}
