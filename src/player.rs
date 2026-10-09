use crate::api::Stream;
use std::{
    collections::VecDeque,
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const TICK: Duration = Duration::from_millis(50);
// Rounds over a station's full stream list before giving up. Stream hosts sit behind
// rotating DNS pools, so a later lookup can hand out an address the first one did not.
pub const STREAM_ROUNDS: u32 = 3;
const STREAM_RETRY_DELAY: Duration = Duration::from_secs(2);
// A live engine reports every tick; a longer silence while audio should be alive
// means the worker is gone, and the UI must say so instead of freezing.
const WORKER_STALL_AFTER: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayerState {
    #[default]
    Idle,
    Buffering,
    Playing,
    Paused,
    Error,
}

/// Everything the engine knows, published to the main thread once per tick.
/// New fields must stay `Default` + `Clone` and reset on `play`, `stop`,
/// and the engine's stream-start equivalent, or the previous stream's data
/// leaks into the next one.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub state: PlayerState,
    pub levels: [f64; 2],
    pub error: Option<String>,
    pub codec: Option<String>,
    pub samplerate: Option<u64>,
    pub channels: Option<u64>,
    pub bitrate: Option<u64>,
    pub buffering: Option<u8>,
    pub cache_seconds: Option<f64>,
    pub stream: Option<String>,
    pub stream_round: u32,
    pub underruns: u32,
}

pub(crate) enum Request {
    Play(u64, Vec<Stream>),
    Pause,
    Stop(u64),
    Volume(u8),
    Quit,
}

pub struct Player {
    tx: Sender<Request>,
    rx: Receiver<(u64, Snapshot)>,
    generation: u64,
    last_update: Instant,
    worker: Option<JoinHandle<()>>,
    pub snapshot: Snapshot,
}

impl Player {
    pub fn new(volume: u8) -> Self {
        let (tx, commands) = mpsc::channel();
        let (updates, rx) = mpsc::channel();
        let worker = thread::spawn(move || worker(commands, updates, volume));
        Self {
            tx,
            rx,
            generation: 0,
            last_update: Instant::now(),
            worker: Some(worker),
            snapshot: Snapshot::default(),
        }
    }
    // `streams` is in priority order; later entries are tried when earlier ones fail to load.
    pub fn play(&mut self, streams: Vec<Stream>) {
        self.generation += 1;
        self.last_update = Instant::now();
        self.snapshot = Snapshot {
            state: PlayerState::Buffering,
            ..Snapshot::default()
        };
        let _ = self.tx.send(Request::Play(self.generation, streams));
    }
    pub fn toggle_pause(&self) {
        let _ = self.tx.send(Request::Pause);
    }
    pub fn stop(&mut self) {
        self.generation += 1;
        self.snapshot = Snapshot::default();
        let _ = self.tx.send(Request::Stop(self.generation));
    }
    pub fn set_volume(&self, volume: u8) {
        let _ = self.tx.send(Request::Volume(volume.min(100)));
    }
    pub fn update(&mut self) {
        let mut received = false;
        for (generation, update) in self.rx.try_iter() {
            received = true;
            if generation == self.generation {
                self.snapshot = update;
            }
        }
        if received {
            self.last_update = Instant::now();
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The worker sends a snapshot every tick while an engine is alive. No message
    /// for `WORKER_STALL_AFTER` during buffering, playback, or pause means it
    /// panicked or hung; the main thread surfaces that instead of a frozen UI.
    pub fn worker_stalled(&self) -> bool {
        matches!(
            self.snapshot.state,
            PlayerState::Buffering | PlayerState::Playing | PlayerState::Paused
        ) && self.last_update.elapsed() > WORKER_STALL_AFTER
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.tx.send(Request::Quit);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn worker(commands: Receiver<Request>, updates: Sender<(u64, Snapshot)>, volume: u8) {
    worker_with(commands, updates, volume, crate::engine::open);
}

fn worker_with(
    commands: Receiver<Request>,
    updates: Sender<(u64, Snapshot)>,
    mut volume: u8,
    open: impl Fn(u8, &[Stream]) -> crate::error::Result<Box<dyn crate::engine::EngineImpl>>,
) {
    let mut engine: Option<Box<dyn crate::engine::EngineImpl>> = None;
    let mut generation = 0;
    loop {
        match commands.recv_timeout(TICK) {
            Ok(Request::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Request::Volume(value)) => {
                volume = value;
                if let Some(engine) = &mut engine {
                    engine.set_volume(volume);
                }
            }
            Ok(Request::Play(request_generation, streams)) => {
                generation = request_generation;
                // Retire the previous pipeline before opening the new station.
                engine = None;
                match open(volume, &streams) {
                    Ok(opened) => engine = Some(opened),
                    Err(err) => {
                        let _ = updates.send((
                            generation,
                            Snapshot {
                                state: PlayerState::Error,
                                error: Some(err.to_string()),
                                ..Snapshot::default()
                            },
                        ));
                    }
                }
                if let Some(engine) = &mut engine {
                    engine.play(streams);
                }
            }
            Ok(Request::Pause) => {
                if let Some(engine) = &mut engine {
                    engine.pause();
                }
            }
            Ok(Request::Stop(request_generation)) => {
                generation = request_generation;
                engine = None;
                let _ = updates.send((generation, Snapshot::default()));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some(active) = &mut engine {
            if let Err(err) = active.tick() {
                let _ = updates.send((
                    generation,
                    Snapshot {
                        state: PlayerState::Error,
                        error: Some(err.to_string()),
                        ..Snapshot::default()
                    },
                ));
                engine = None;
            } else {
                if updates.send((generation, active.snapshot())).is_err() {
                    break;
                }
            }
        }
    }
}

// The current station's streams in priority order. A failed stream falls through to the
// next one; when the list runs out it is retried after a pause, up to STREAM_ROUNDS times.
#[derive(Default)]
pub(crate) struct Streams {
    streams: Vec<Stream>,
    queue: VecDeque<Stream>,
    current: Option<Stream>,
    unreachable: Vec<String>,
    round: u32,
    retry_at: Option<Instant>,
}

pub(crate) enum Failure {
    Load(Stream),
    Wait,
    GiveUp(String),
}

impl Streams {
    pub(crate) fn new(streams: Vec<Stream>) -> Self {
        Self {
            queue: streams.iter().cloned().collect(),
            streams,
            ..Self::default()
        }
    }
    pub(crate) fn next(&mut self) -> Option<Stream> {
        let stream = self.queue.pop_front()?;
        self.current = Some(stream.clone());
        Some(stream)
    }
    pub(crate) fn round(&self) -> u32 {
        self.round
    }
    pub(crate) fn fail(&mut self, error: &str, now: Instant) -> Failure {
        if self.retry_at.is_some() {
            return Failure::Wait;
        }
        if let Some(stream) = self.current.take() {
            let host = host(&stream.url).to_owned();
            if !self.unreachable.contains(&host) {
                self.unreachable.push(host);
            }
        }
        if let Some(stream) = self.next() {
            return Failure::Load(stream);
        }
        if self.round + 1 < STREAM_ROUNDS && !self.streams.is_empty() {
            self.round += 1;
            self.queue = self.streams.iter().cloned().collect();
            self.retry_at = Some(now + STREAM_RETRY_DELAY);
            return Failure::Wait;
        }
        Failure::GiveUp(
            if error == "loading failed" && !self.unreachable.is_empty() {
                format!(
                    "{} unreachable · Enter to retry",
                    self.unreachable.join(", ")
                )
            } else {
                format!("Stream unavailable: {error} · Enter to retry")
            },
        )
    }
    pub(crate) fn due(&mut self, now: Instant) -> Option<Stream> {
        if self.retry_at.is_none_or(|at| now < at) {
            return None;
        }
        self.retry_at = None;
        self.unreachable.clear();
        self.next()
    }
}

pub(crate) fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', ':', '?']).next().unwrap_or(rest)
}

/// A per-channel loudness meter fed with 20 Hz RMS readings in dB. The
/// normalization (rolling percentiles, attack/release easing) is
/// fed by decoded samples consumed by the native output callback.
#[derive(Default)]
pub(crate) struct AudioMeter {
    channels: [ChannelMeter; 2],
}

#[derive(Default)]
pub(crate) struct ChannelMeter {
    samples: VecDeque<f64>,
    level: f64,
}

impl AudioMeter {
    /// Feed one RMS reading per channel (dB). `None` on the right means a mono
    /// source: it mirrors the left channel instead of staying dead.
    pub(crate) fn update(&mut self, left_db: Option<f64>, right_db: Option<f64>) -> [f64; 2] {
        let left = self.channels[0].update(left_db);
        match right_db {
            Some(db) => {
                let right = self.channels[1].update(Some(db));
                [left, right]
            }
            None => [left, left],
        }
    }
}

impl ChannelMeter {
    fn update(&mut self, db: Option<f64>) -> f64 {
        let Some(db) = db.filter(|db| db.is_finite() && *db > -60.0) else {
            self.level *= 0.65;
            if self.level < 0.005 {
                self.level = 0.0;
            }
            return self.level;
        };
        self.samples.push_back(db);
        if self.samples.len() > 80 {
            self.samples.pop_front();
        }
        // Adapt to the station's recent loudness instead of clipping mastered radio at 100%.
        // Percentiles ignore isolated peaks; a minimum 4 dB window avoids amplifying noise.
        let mut sorted: Vec<_> = self.samples.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        let low = sorted[(sorted.len() - 1) / 10];
        let high = sorted[(sorted.len() - 1) * 9 / 10];
        let center = (low + high) / 2.0;
        let span = (high - low).max(4.0);
        let target = (0.5 + (db - center) / span).clamp(0.0, 1.0) * 0.9 + 0.05;
        // At 20 Hz: quick attack, softer release. No motion is synthesized without audio.
        let easing = if target > self.level { 0.5 } else { 0.25 };
        self.level += (target - self.level) * easing;
        self.level
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::api::StreamKind;

    pub(crate) fn test_stream(url: &str) -> Stream {
        Stream {
            kind: StreamKind::Main,
            url: url.to_owned(),
        }
    }

    fn player_without_worker() -> (Player, Sender<(u64, Snapshot)>) {
        let (tx, _commands) = mpsc::channel();
        let (updates, rx) = mpsc::channel();
        (
            Player {
                tx,
                rx,
                generation: 0,
                last_update: Instant::now(),
                worker: None,
                snapshot: Snapshot::default(),
            },
            updates,
        )
    }

    #[test]
    fn late_audio_updates_cannot_restore_a_stopped_station() {
        let (mut player, updates) = player_without_worker();
        player.play(vec![test_stream("https://example.com/first")]);
        updates
            .send((
                1,
                Snapshot {
                    state: PlayerState::Playing,
                    ..Snapshot::default()
                },
            ))
            .unwrap();
        player.stop();
        player.update();
        assert_eq!(player.snapshot.state, PlayerState::Idle);
        player.play(vec![test_stream("https://example.com/second")]);
        updates
            .send((
                1,
                Snapshot {
                    state: PlayerState::Error,
                    ..Snapshot::default()
                },
            ))
            .unwrap();
        player.update();
        assert_eq!(player.snapshot.state, PlayerState::Buffering);
    }

    #[test]
    fn starting_playback_gets_a_fresh_worker_heartbeat_deadline() {
        let (mut player, _updates) = player_without_worker();
        player.last_update -= WORKER_STALL_AFTER + Duration::from_secs(1);
        player.play(vec![test_stream("https://example.com/audio")]);
        assert_eq!(player.snapshot.state, PlayerState::Buffering);
        assert!(!player.worker_stalled());
    }

    #[test]
    fn worker_stall_is_detected_only_while_audio_should_be_alive() {
        let (mut player, updates) = player_without_worker();
        player.snapshot.state = PlayerState::Playing;
        player.last_update -= WORKER_STALL_AFTER + Duration::from_millis(50);
        assert!(player.worker_stalled());
        updates
            .send((
                0,
                Snapshot {
                    state: PlayerState::Playing,
                    ..Snapshot::default()
                },
            ))
            .unwrap();
        player.update();
        assert!(!player.worker_stalled(), "a heartbeat clears the stall");
        player.snapshot.state = PlayerState::Idle;
        player.last_update -= WORKER_STALL_AFTER;
        assert!(!player.worker_stalled(), "an idle player owes no heartbeat");
    }

    #[test]
    fn streams_fall_back_retry_and_name_unreachable_hosts() {
        let mut streams = Streams::new(vec![
            test_stream("https://a.example.com/96.aacp"),
            Stream {
                kind: StreamKind::Hls,
                url: "https://hls.example.com:443/playlist.m3u8".into(),
            },
        ]);
        let mut now = Instant::now();
        let mut first = streams.next();
        for round in 1..=STREAM_ROUNDS {
            assert_eq!(
                first.as_ref().map(|s| s.url.as_str()),
                Some("https://a.example.com/96.aacp")
            );
            assert!(
                matches!(&streams.fail("loading failed", now), Failure::Load(url) if url.url.contains("hls"))
            );
            if round == STREAM_ROUNDS {
                break;
            }
            assert!(matches!(streams.fail("loading failed", now), Failure::Wait));
            // A late error while waiting must not cut the pause short.
            assert!(matches!(streams.fail("loading failed", now), Failure::Wait));
            assert!(streams.due(now).is_none());
            now += STREAM_RETRY_DELAY;
            first = streams.due(now);
        }
        match streams.fail("loading failed", now) {
            Failure::GiveUp(message) => assert_eq!(
                message,
                "a.example.com, hls.example.com unreachable · Enter to retry"
            ),
            _ => panic!("must give up after the last round"),
        }

        let mut streams = Streams::new(vec![test_stream("https://a.example.com/x")]);
        streams.round = STREAM_ROUNDS - 1;
        streams.next();
        match streams.fail("unrecognized file format", now) {
            Failure::GiveUp(message) => assert_eq!(
                message,
                "Stream unavailable: unrecognized file format · Enter to retry"
            ),
            _ => panic!("must give up after the last round"),
        }
    }

    #[test]
    fn meter_rejects_missing_and_non_finite_levels() {
        let mut meter = AudioMeter::default();
        for value in [f64::NEG_INFINITY, f64::NAN, f64::INFINITY] {
            assert_eq!(meter.update(Some(value), Some(value)), [0.0, 0.0]);
        }
        assert_eq!(meter.update(None, None), [0.0, 0.0]);
    }

    #[test]
    fn loud_radio_has_headroom_and_a_smooth_wide_range() {
        let mut meter = AudioMeter::default();
        for _ in 0..80 {
            meter.update(Some(-6.0), Some(-6.0));
        }
        assert!((meter.channels[0].level - 0.5).abs() < 0.01);
        assert!((meter.channels[1].level - 0.5).abs() < 0.01);
        let mut values = Vec::new();
        for db in [-4.0, -4.0, -4.0, -8.0, -8.0, -8.0, -8.0, -8.0, -8.0] {
            let levels = meter.update(Some(db), Some(db));
            values.push(levels[0]);
        }
        assert!(values[2] > 0.8 && values[8] < 0.3, "{values:?}");
        assert!(
            values
                .windows(2)
                .all(|pair| (pair[1] - pair[0]).abs() < 0.3)
        );
        assert!(values.iter().all(|value| *value < 0.96));
        for _ in 0..30 {
            meter.update(Some(f64::NEG_INFINITY), Some(f64::NEG_INFINITY));
        }
        assert_eq!(meter.channels[0].level, 0.0);
        assert_eq!(meter.channels[1].level, 0.0);
    }

    #[test]
    fn mono_sources_mirror_the_left_channel() {
        let mut meter = AudioMeter::default();
        for _ in 0..80 {
            meter.update(Some(-6.0), None);
        }
        let levels = meter.update(Some(-4.0), None);
        assert_eq!(levels[0], levels[1]);
        assert!(levels[0] > 0.5);
    }

    #[test]
    fn switching_stations_reopens_engine_with_current_streams_and_volume() {
        use crate::api::StreamKind;
        use crate::engine::EngineImpl;

        struct FakeEngine(Sender<String>);
        impl EngineImpl for FakeEngine {
            fn play(&mut self, _: Vec<Stream>) {}
            fn pause(&mut self) {}
            fn set_volume(&mut self, _: u8) {}
            fn tick(&mut self) -> crate::error::Result<()> {
                Ok(())
            }
            fn snapshot(&self) -> Snapshot {
                Snapshot {
                    state: PlayerState::Playing,
                    ..Snapshot::default()
                }
            }
        }
        impl Drop for FakeEngine {
            fn drop(&mut self) {
                self.0.send("drop".into()).unwrap();
            }
        }
        let (commands_tx, commands_rx) = mpsc::channel();
        let (updates_tx, updates_rx) = mpsc::channel();
        let (events_tx, events_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            worker_with(commands_rx, updates_tx, 80, |volume, streams| {
                events_tx
                    .send(format!("open:{:?}:{volume}", streams[0].kind))
                    .unwrap();
                Ok(Box::new(FakeEngine(events_tx.clone())))
            });
        });
        commands_tx
            .send(Request::Play(
                1,
                vec![Stream {
                    kind: StreamKind::Main,
                    url: "lc".into(),
                }],
            ))
            .unwrap();
        assert_eq!(
            updates_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
            1
        );
        commands_tx.send(Request::Volume(35)).unwrap();
        commands_tx
            .send(Request::Play(
                2,
                vec![Stream {
                    kind: StreamKind::High,
                    url: "he".into(),
                }],
            ))
            .unwrap();
        loop {
            if updates_rx.recv_timeout(Duration::from_secs(1)).unwrap().0 == 2 {
                break;
            }
        }
        commands_tx.send(Request::Quit).unwrap();
        worker.join().unwrap();
        assert_eq!(
            events_rx.try_iter().collect::<Vec<_>>(),
            ["open:Main:80", "drop", "open:High:35", "drop"]
        );
    }
}
