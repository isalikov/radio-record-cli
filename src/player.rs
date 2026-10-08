use crate::error::{Error, Result};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    os::unix::net::UnixStream,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const TICK: Duration = Duration::from_millis(50);
// How long to wait for mpv to open its IPC socket. The first launch of a freshly
// installed mpv can take well over five seconds on macOS while the system verifies
// the binary and its libraries; later launches take a fraction of a second.
const IPC_STARTUP_GRACE: Duration = Duration::from_secs(20);
// Rounds over a station's full stream list before giving up. Stream hosts sit behind
// rotating DNS pools, so a later lookup can hand out an address the first one did not.
pub const STREAM_ROUNDS: u32 = 3;
const STREAM_RETRY_DELAY: Duration = Duration::from_secs(2);
// A live engine reports every tick; a longer silence while audio should be alive
// means the worker is gone, and the UI must say so instead of freezing.
const WORKER_STALL_AFTER: Duration = Duration::from_secs(2);
// Per-channel levels plus the Overall fallback: the stereo meter rides the same
// single af-metadata reply, with no extra IPC traffic; mono degrades to a mirror.
const METER: &str = "--af-add=@radiome_meter:lavfi=[astats=metadata=1:reset=1:measure_perchannel=RMS_level:measure_overall=RMS_level]";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayerState {
    #[default]
    Idle,
    Buffering,
    Playing,
    Paused,
    Error,
}

/// Everything the worker knows, published to the main thread once per tick.
/// New fields must stay `Default` + `Clone` and reset on `play`, `stop`,
/// and `start-file`, or the previous stream's data leaks into the next one.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub state: PlayerState,
    pub levels: [f64; 2],
    pub peaks: [f64; 2],
    pub error: Option<String>,
    pub codec: Option<String>,
    pub samplerate: Option<u64>,
    pub channels: Option<u64>,
    pub bitrate: Option<u64>,
    pub buffering: Option<u8>,
    pub cache_seconds: Option<f64>,
    pub stream: Option<String>,
    pub stream_round: u32,
}

enum Request {
    Play(u64, Vec<String>),
    Pause,
    Stop(u64),
    Volume(u8),
    Quit,
}

pub struct Player {
    tx: Sender<Request>,
    rx: Receiver<(u64, Snapshot)>,
    media_rx: Receiver<isize>,
    generation: u64,
    last_update: Instant,
    worker: Option<JoinHandle<()>>,
    pub snapshot: Snapshot,
}

impl Player {
    pub fn new(volume: u8) -> Self {
        let (tx, commands) = mpsc::channel();
        let (updates, rx) = mpsc::channel();
        let (media_tx, media_rx) = mpsc::channel();
        let worker = thread::spawn(move || worker(commands, updates, media_tx, volume));
        Self {
            tx,
            rx,
            media_rx,
            generation: 0,
            last_update: Instant::now(),
            worker: Some(worker),
            snapshot: Snapshot::default(),
        }
    }
    // `urls` is in priority order; later entries are tried when earlier ones fail to load.
    pub fn play(&mut self, urls: Vec<String>) {
        self.generation += 1;
        self.snapshot = Snapshot {
            state: PlayerState::Buffering,
            ..Snapshot::default()
        };
        let _ = self.tx.send(Request::Play(self.generation, urls));
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

    pub fn media_commands(&self) -> Vec<isize> {
        self.media_rx.try_iter().collect()
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

fn worker(
    commands: Receiver<Request>,
    updates: Sender<(u64, Snapshot)>,
    media_tx: Sender<isize>,
    mut volume: u8,
) {
    let mut engine: Option<Engine> = None;
    let mut generation = 0;
    loop {
        match commands.recv_timeout(TICK) {
            Ok(Request::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Request::Volume(value)) => {
                volume = value;
                if let Some(engine) = &mut engine {
                    engine.command(json!(["set_property", "volume", volume]));
                }
            }
            Ok(Request::Play(request_generation, urls)) => {
                generation = request_generation;
                if engine.is_none() {
                    match Engine::new(volume, false) {
                        Ok(value) => engine = Some(value),
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
                }
                if let Some(engine) = &mut engine {
                    engine.snapshot = Snapshot {
                        state: PlayerState::Buffering,
                        ..Snapshot::default()
                    };
                    engine.paused = false;
                    engine.command(json!(["set_property", "pause", false]));
                    engine.streams = Streams::new(urls);
                    if let Some(url) = engine.streams.next() {
                        engine.snapshot.stream = Some(url.clone());
                        engine.snapshot.stream_round = engine.streams.round + 1;
                        engine.load(&url);
                    }
                }
            }
            Ok(Request::Pause) => {
                if let Some(engine) = &mut engine {
                    engine.command(json!(["cycle", "pause"]));
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
            } else if updates.send((generation, active.snapshot.clone())).is_err() {
                break;
            } else {
                for direction in active.media.drain(..) {
                    let _ = media_tx.send(direction);
                }
            }
        }
    }
}

// mpv and IPC live on a worker so buffering never blocks keyboard input or rendering.
struct Engine {
    media: Vec<isize>,
    streams: Streams,
    meter: AudioMeter,
    meter_received: Instant,
    child: Child,
    directory: tempfile::TempDir,
    socket: Option<UnixStream>,
    input: Vec<u8>,
    output: Vec<u8>,
    started: Instant,
    metered: Instant,
    buffering_polled: Instant,
    cache_polled: Instant,
    paused: bool,
    snapshot: Snapshot,
}

impl Engine {
    fn new(volume: u8, null_audio: bool) -> Result<Self> {
        // macOS Unix socket paths must fit in 104 bytes.
        let directory = tempfile::Builder::new()
            .prefix("radiome-")
            .tempdir_in("/tmp")?;
        // macOS RemoteCommandCenter routes hardware media keys through mpv's input bindings.
        // The Rust app owns the station list and decides what Previous/Next should play.
        let bindings = directory.path().join("input.conf");
        std::fs::write(
            &bindings,
            concat!(
                "PREV script-message radiome-previous\n",
                "NEXT script-message radiome-next\n",
                "PLAY cycle pause\n",
                "PLAYPAUSE cycle pause\n",
                "PLAYONLY set pause no\n",
                "PAUSEONLY set pause yes\n",
            ),
        )?;
        let mut command = Command::new("mpv");
        command
            .args([
                "--no-config",
                "--no-video",
                "--no-terminal",
                "--idle=yes",
                "--input-default-bindings=no",
                "--input-vo-keyboard=no",
                "--audio-display=no",
                "--volume-max=100",
                "--network-timeout=10",
                METER,
            ])
            .arg(format!("--volume={volume}"))
            .arg(format!("--input-conf={}", bindings.display()))
            .arg(format!(
                "--input-ipc-server={}",
                directory.path().join("ipc").display()
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if null_audio {
            command.arg("--ao=null");
        }
        let child = command
            .spawn()
            .map_err(|err| Error::new(format!("Could not start mpv: {err}")))?;
        Ok(Self {
            media: Vec::new(),
            streams: Streams::default(),
            meter: AudioMeter::default(),
            meter_received: Instant::now(),
            child,
            directory,
            socket: None,
            input: Vec::new(),
            output: Vec::new(),
            started: Instant::now(),
            metered: Instant::now(),
            buffering_polled: Instant::now(),
            cache_polled: Instant::now(),
            paused: false,
            snapshot: Snapshot::default(),
        })
    }
    fn load(&mut self, url: &str) {
        self.command(json!(["loadfile", url, "replace"]));
    }
    fn command(&mut self, command: Value) {
        self.request(command, 0);
    }
    fn request(&mut self, command: Value, id: u64) {
        self.output.extend(
            json!({"command": command, "request_id": id})
                .to_string()
                .bytes(),
        );
        self.output.push(b'\n');
    }
    fn tick(&mut self) -> Result<()> {
        if let Some(status) = self.child.try_wait()? {
            return Err(Error::new(format!("mpv exited: {status}")));
        }
        if self.socket.is_none() {
            match UnixStream::connect(self.directory.path().join("ipc")) {
                Ok(socket) => {
                    socket.set_nonblocking(true)?;
                    self.socket = Some(socket);
                    for (id, property) in [
                        (1, "pause"),
                        (2, "core-idle"),
                        (3, "audio-codec-name"),
                        (4, "audio-params/samplerate"),
                        (5, "audio-params/channel-count"),
                        (6, "audio-bitrate"),
                    ] {
                        self.command(json!(["observe_property", id, property]));
                    }
                }
                Err(_) if self.started.elapsed() < IPC_STARTUP_GRACE => return Ok(()),
                Err(err) => {
                    return Err(Error::new(format!(
                        "mpv did not open its IPC socket in {}s ({err}) · Enter to retry",
                        IPC_STARTUP_GRACE.as_secs()
                    )));
                }
            }
        }
        if let Some(url) = self.streams.due(Instant::now()) {
            self.snapshot.stream = Some(url.clone());
            self.snapshot.stream_round = self.streams.round + 1;
            self.load(&url);
        }
        if self.metered.elapsed() >= TICK {
            self.request(json!(["get_property", "af-metadata/radiome_meter"]), 10);
            self.metered = Instant::now();
            if self.meter_received.elapsed() > Duration::from_millis(250) {
                for level in &mut self.snapshot.levels {
                    *level *= 0.8;
                }
                for peak in &mut self.snapshot.peaks {
                    *peak *= 0.8;
                }
            }
        }
        // Volatile properties are polled at a low rate and only while their state is
        // active, so a healthy stream costs no extra requests at all.
        if self.snapshot.state == PlayerState::Buffering
            && self.buffering_polled.elapsed() >= Duration::from_millis(500)
        {
            self.request(json!(["get_property", "cache-buffering-state"]), 11);
            self.buffering_polled = Instant::now();
        }
        if self.snapshot.state == PlayerState::Playing
            && self.cache_polled.elapsed() >= Duration::from_millis(1000)
        {
            self.request(json!(["get_property", "demuxer-cache-duration"]), 12);
            self.cache_polled = Instant::now();
        }
        // Mirror of the input cap: an mpv that stopped reading its socket must be
        // reported, not allowed to grow the pending request queue without bound.
        if self.output.len() > 1_048_576 {
            return Err(Error::new("mpv IPC requests are backing up"));
        }
        let socket = self.socket.as_mut().unwrap();
        while !self.output.is_empty() {
            match socket.write(&self.output) {
                Ok(0) => return Err(Error::new("mpv IPC closed")),
                Ok(n) => {
                    self.output.drain(..n);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => return Err(err.into()),
            }
        }
        let mut buffer = [0u8; 8192];
        loop {
            match socket.read(&mut buffer) {
                Ok(0) => return Err(Error::new("mpv IPC closed")),
                Ok(n) => self.input.extend_from_slice(&buffer[..n]),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => return Err(err.into()),
            }
            if self.input.len() > 1_048_576 {
                return Err(Error::new("mpv response is too large"));
            }
        }
        while let Some(end) = self.input.iter().position(|byte| *byte == b'\n') {
            let line: Vec<_> = self.input.drain(..=end).collect();
            if let Ok(value) = serde_json::from_slice(&line) {
                self.message(&value);
            }
        }
        if self.snapshot.state != PlayerState::Playing {
            self.snapshot.levels = [0.0; 2];
            self.snapshot.peaks = [0.0; 2];
            self.snapshot.cache_seconds = None;
            self.meter = AudioMeter::default();
        }
        if self.snapshot.state != PlayerState::Buffering {
            self.snapshot.buffering = None;
        }
        Ok(())
    }
    fn message(&mut self, value: &Value) {
        match value["event"].as_str() {
            Some("client-message") => match value["args"][0].as_str() {
                Some("radiome-previous") => self.media.push(-1),
                Some("radiome-next") => self.media.push(1),
                _ => {}
            },
            Some("start-file") => {
                self.meter = AudioMeter::default();
                self.snapshot.levels = [0.0; 2];
                self.snapshot.peaks = [0.0; 2];
                self.snapshot.state = PlayerState::Buffering;
                self.snapshot.error = None;
                // Telemetry belongs to the stream that is starting, never the one before it.
                self.snapshot.codec = None;
                self.snapshot.samplerate = None;
                self.snapshot.channels = None;
                self.snapshot.bitrate = None;
                self.snapshot.buffering = None;
                self.snapshot.cache_seconds = None;
            }
            Some("property-change") => match value["name"].as_str() {
                Some("pause") => {
                    self.paused = value["data"].as_bool().unwrap_or(false);
                    if self.paused {
                        self.snapshot.state = PlayerState::Paused;
                    } else if self.snapshot.state == PlayerState::Paused {
                        self.snapshot.state = PlayerState::Buffering;
                    }
                }
                Some("core-idle")
                    if !matches!(self.snapshot.state, PlayerState::Error | PlayerState::Idle) =>
                {
                    self.snapshot.state = if self.paused {
                        PlayerState::Paused
                    } else if value["data"] == false {
                        PlayerState::Playing
                    } else {
                        PlayerState::Buffering
                    };
                }
                Some("audio-codec-name") => {
                    self.snapshot.codec = value["data"].as_str().map(str::to_owned);
                }
                Some("audio-params/samplerate") => {
                    self.snapshot.samplerate = positive(value["data"].as_i64());
                }
                Some("audio-params/channel-count") => {
                    self.snapshot.channels = positive(value["data"].as_i64());
                }
                Some("audio-bitrate") => {
                    self.snapshot.bitrate = positive(value["data"].as_i64());
                }
                _ => {}
            },
            Some("end-file") if value["reason"] == "error" => {
                let error = value["file_error"].as_str().unwrap_or("mpv error");
                match self.streams.fail(error, Instant::now()) {
                    Failure::Load(url) => {
                        self.snapshot.stream = Some(url.clone());
                        self.snapshot.stream_round = self.streams.round + 1;
                        self.load(&url);
                        self.snapshot.state = PlayerState::Buffering;
                    }
                    Failure::Wait => self.snapshot.state = PlayerState::Buffering,
                    Failure::GiveUp(message) => {
                        self.snapshot.state = PlayerState::Error;
                        self.snapshot.error = Some(message);
                    }
                }
            }
            Some("end-file") if value["reason"] == "eof" => self.snapshot.state = PlayerState::Idle,
            _ => {}
        }
        if value["request_id"] == 10 && value["error"] == "success" {
            let (levels, peaks) = self.meter.update(&value["data"]);
            self.snapshot.levels = levels;
            self.snapshot.peaks = peaks;
            self.meter_received = Instant::now();
        }
        if value["request_id"] == 11 && value["error"] == "success" {
            self.snapshot.buffering = value["data"]
                .as_i64()
                .map(|percent| percent.clamp(0, 100) as u8);
        }
        if value["request_id"] == 12 && value["error"] == "success" {
            self.snapshot.cache_seconds = value["data"]
                .as_f64()
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0);
        }
    }
}

// The current station's streams in priority order. A failed stream falls through to the
// next one; when the list runs out it is retried after a pause, up to STREAM_ROUNDS times.
#[derive(Default)]
struct Streams {
    urls: Vec<String>,
    queue: VecDeque<String>,
    current: Option<String>,
    unreachable: Vec<String>,
    round: u32,
    retry_at: Option<Instant>,
}

enum Failure {
    Load(String),
    Wait,
    GiveUp(String),
}

impl Streams {
    fn new(urls: Vec<String>) -> Self {
        Self {
            queue: urls.iter().cloned().collect(),
            urls,
            ..Self::default()
        }
    }
    fn next(&mut self) -> Option<String> {
        let url = self.queue.pop_front()?;
        self.current = Some(url.clone());
        Some(url)
    }
    fn fail(&mut self, error: &str, now: Instant) -> Failure {
        if self.retry_at.is_some() {
            return Failure::Wait;
        }
        if let Some(url) = self.current.take() {
            let host = host(&url).to_owned();
            if !self.unreachable.contains(&host) {
                self.unreachable.push(host);
            }
        }
        if let Some(url) = self.next() {
            return Failure::Load(url);
        }
        if self.round + 1 < STREAM_ROUNDS && !self.urls.is_empty() {
            self.round += 1;
            self.queue = self.urls.iter().cloned().collect();
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
    fn due(&mut self, now: Instant) -> Option<String> {
        if self.retry_at.is_none_or(|at| now < at) {
            return None;
        }
        self.retry_at = None;
        self.unreachable.clear();
        self.next()
    }
}

fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', ':', '?']).next().unwrap_or(rest)
}

fn positive(value: Option<i64>) -> Option<u64> {
    value.filter(|value| *value > 0).map(|value| value as u64)
}

#[derive(Default)]
struct AudioMeter {
    channels: [ChannelMeter; 2],
}

#[derive(Default)]
struct ChannelMeter {
    samples: VecDeque<f64>,
    level: f64,
    peak: f64,
}

impl AudioMeter {
    /// Reads both channels from a single af-metadata reply. A mono source has no
    /// second channel, so the right side mirrors the left instead of staying dead.
    fn update(&mut self, metadata: &Value) -> ([f64; 2], [f64; 2]) {
        let overall = rms(metadata, "Overall");
        let left = self.channels[0].update(rms(metadata, "1").or(overall));
        match rms(metadata, "2") {
            Some(db) => {
                let right = self.channels[1].update(Some(db));
                self.channels[0].hold_peak();
                self.channels[1].hold_peak();
                (
                    [left, right],
                    [self.channels[0].peak, self.channels[1].peak],
                )
            }
            None => {
                self.channels[0].hold_peak();
                ([left, left], [self.channels[0].peak, self.channels[0].peak])
            }
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
    // The peak marker lingers after the beat: it never rises without real audio
    // and falls back slowly, about one percent per update.
    fn hold_peak(&mut self) {
        self.peak *= 0.99;
        if self.level > self.peak {
            self.peak = self.level;
        }
    }
}

fn rms(metadata: &Value, channel: &str) -> Option<f64> {
    let key = format!("lavfi.astats.{channel}.RMS_level");
    metadata[key.as_str()]
        .as_str()
        .and_then(|value| value.parse::<f64>().ok())
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player_without_worker() -> (Player, Sender<(u64, Snapshot)>) {
        let (tx, _commands) = mpsc::channel();
        let (updates, rx) = mpsc::channel();
        (
            Player {
                tx,
                rx,
                media_rx: mpsc::channel().1,
                generation: 0,
                last_update: Instant::now(),
                worker: None,
                snapshot: Snapshot::default(),
            },
            updates,
        )
    }

    // message() only touches Engine fields, so a throwaway child process stands in
    // for mpv; it is killed by the Drop guard like the real one.
    fn engine_without_mpv() -> Engine {
        Engine {
            media: Vec::new(),
            streams: Streams::default(),
            meter: AudioMeter::default(),
            meter_received: Instant::now(),
            child: Command::new("true").spawn().unwrap(),
            directory: tempfile::Builder::new()
                .prefix("radiome-test-")
                .tempdir_in("/tmp")
                .unwrap(),
            socket: None,
            input: Vec::new(),
            output: Vec::new(),
            started: Instant::now(),
            metered: Instant::now(),
            buffering_polled: Instant::now(),
            cache_polled: Instant::now(),
            paused: false,
            snapshot: Snapshot::default(),
        }
    }

    #[test]
    fn late_audio_updates_cannot_restore_a_stopped_station() {
        let (mut player, updates) = player_without_worker();
        player.play(vec!["https://example.com/first".into()]);
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
        player.play(vec!["https://example.com/second".into()]);
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
    fn telemetry_parses_properties_and_resets_between_streams() {
        let mut engine = engine_without_mpv();
        engine.message(&json!({"event": "start-file"}));
        assert_eq!(engine.snapshot.state, PlayerState::Buffering);
        engine.message(
            &json!({"event": "property-change", "name": "audio-codec-name", "data": "aac"}),
        );
        engine.message(
            &json!({"event": "property-change", "name": "audio-params/samplerate", "data": 44100}),
        );
        engine.message(
            &json!({"event": "property-change", "name": "audio-params/channel-count", "data": 2}),
        );
        engine
            .message(&json!({"event": "property-change", "name": "audio-bitrate", "data": 128000}));
        assert_eq!(engine.snapshot.codec.as_deref(), Some("aac"));
        assert_eq!(engine.snapshot.samplerate, Some(44_100));
        assert_eq!(engine.snapshot.channels, Some(2));
        assert_eq!(engine.snapshot.bitrate, Some(128_000));
        engine.message(&json!({"request_id": 11, "error": "success", "data": 42}));
        assert_eq!(engine.snapshot.buffering, Some(42));
        engine.message(&json!({"request_id": 11, "error": "success", "data": 250}));
        assert_eq!(engine.snapshot.buffering, Some(100));
        engine.message(&json!({"request_id": 12, "error": "success", "data": 1.5}));
        assert_eq!(engine.snapshot.cache_seconds, Some(1.5));
        // The next stream must not inherit the previous stream's telemetry.
        engine.message(&json!({"event": "start-file"}));
        assert_eq!(engine.snapshot.codec, None);
        assert_eq!(engine.snapshot.samplerate, None);
        assert_eq!(engine.snapshot.channels, None);
        assert_eq!(engine.snapshot.bitrate, None);
        assert_eq!(engine.snapshot.buffering, None);
        assert_eq!(engine.snapshot.cache_seconds, None);
    }

    #[test]
    fn stream_failures_update_the_active_stream_and_round() {
        let mut engine = engine_without_mpv();
        let first = "https://a.example.com/96.aacp".to_owned();
        let second = "https://hls.example.com/playlist.m3u8".to_owned();
        engine.streams = Streams::new(vec![first.clone(), second.clone()]);
        engine.streams.next();
        engine.snapshot.stream = Some(first);
        engine.snapshot.stream_round = 1;
        engine.message(
            &json!({"event": "end-file", "reason": "error", "file_error": "loading failed"}),
        );
        assert_eq!(engine.snapshot.stream.as_deref(), Some(second.as_str()));
        assert_eq!(engine.snapshot.stream_round, 1);
        assert_eq!(engine.snapshot.state, PlayerState::Buffering);
    }

    #[test]
    fn streams_fall_back_retry_and_name_unreachable_hosts() {
        let mut streams = Streams::new(vec![
            "https://a.example.com/96.aacp".into(),
            "https://hls.example.com:443/playlist.m3u8".into(),
        ]);
        let mut now = Instant::now();
        let mut first = streams.next();
        for round in 1..=STREAM_ROUNDS {
            assert_eq!(first.as_deref(), Some("https://a.example.com/96.aacp"));
            assert!(
                matches!(streams.fail("loading failed", now), Failure::Load(url) if url.contains("hls"))
            );
            if round == STREAM_ROUNDS {
                break;
            }
            assert!(matches!(streams.fail("loading failed", now), Failure::Wait));
            // A late error while waiting must not cut the pause short.
            assert!(matches!(streams.fail("loading failed", now), Failure::Wait));
            assert_eq!(streams.due(now), None);
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

        let mut streams = Streams::new(vec!["https://a.example.com/x".into()]);
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
        for value in ["-inf", "NaN", "inf", "broken"] {
            let (levels, peaks) = meter.update(&json!({"lavfi.astats.Overall.RMS_level": value}));
            assert_eq!(levels, [0.0, 0.0]);
            assert_eq!(peaks, [0.0, 0.0]);
        }
        let (levels, peaks) = meter.update(&json!({}));
        assert_eq!(levels, [0.0, 0.0]);
        assert_eq!(peaks, [0.0, 0.0]);
    }

    #[test]
    fn loud_radio_has_headroom_and_a_smooth_wide_range() {
        let mut meter = AudioMeter::default();
        for _ in 0..80 {
            meter.update(
                &json!({"lavfi.astats.1.RMS_level": "-6", "lavfi.astats.2.RMS_level": "-6"}),
            );
        }
        assert!((meter.channels[0].level - 0.5).abs() < 0.01);
        assert!((meter.channels[1].level - 0.5).abs() < 0.01);
        let mut values = Vec::new();
        for db in [-4, -4, -4, -8, -8, -8, -8, -8, -8] {
            let (levels, _) = meter.update(&json!({
                "lavfi.astats.1.RMS_level": db.to_string(),
                "lavfi.astats.2.RMS_level": db.to_string(),
            }));
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
            meter.update(
                &json!({"lavfi.astats.1.RMS_level": "-inf", "lavfi.astats.2.RMS_level": "-inf"}),
            );
        }
        assert_eq!(meter.channels[0].level, 0.0);
        assert_eq!(meter.channels[1].level, 0.0);
    }

    #[test]
    fn peak_holds_above_the_level_and_falls_slowly() {
        let mut meter = AudioMeter::default();
        for _ in 0..80 {
            meter.update(&json!({"lavfi.astats.1.RMS_level": "-6"}));
        }
        meter.update(&json!({"lavfi.astats.1.RMS_level": "-2"}));
        let (levels, peaks) = meter.update(&json!({"lavfi.astats.1.RMS_level": "-70"}));
        assert!(peaks[0] > levels[0], "the marker must outlive the dip");
        for _ in 0..10 {
            meter.update(&json!({"lavfi.astats.1.RMS_level": "-70"}));
        }
        let (levels, peaks) = meter.update(&json!({"lavfi.astats.1.RMS_level": "-70"}));
        assert!(peaks[0] > 0.1, "the marker must fall slowly, not instantly");
        assert!(peaks[0] > levels[0]);
    }

    #[test]
    fn mono_sources_mirror_the_left_channel() {
        let mut meter = AudioMeter::default();
        for _ in 0..80 {
            meter.update(&json!({
                "lavfi.astats.Overall.RMS_level": "-6",
                "lavfi.astats.1.RMS_level": "-6",
            }));
        }
        let (levels, peaks) = meter.update(&json!({
            "lavfi.astats.Overall.RMS_level": "-4",
            "lavfi.astats.1.RMS_level": "-4",
        }));
        assert_eq!(levels[0], levels[1]);
        assert_eq!(peaks[0], peaks[1]);
        assert!(levels[0] > 0.5);
    }

    #[test]
    #[ignore = "requires mpv and local Unix sockets; uses silent audio output"]
    fn mpv_falls_back_measures_real_audio_and_accepts_volume_and_pause() {
        let mut engine = Engine::new(65, true).unwrap();
        // The first stream cannot be opened, so the engine must fall back to the second.
        engine.streams = Streams::new(vec![
            "/nonexistent/radiome-stream.aacp".to_owned(),
            "av://lavfi:sine=frequency=440:sample_rate=44100".to_owned(),
        ]);
        let url = engine.streams.next().unwrap();
        engine.load(&url);
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            engine.tick().unwrap();
            if engine.snapshot.state == PlayerState::Playing && engine.snapshot.levels[0] > 0.1 {
                break;
            }
            thread::sleep(TICK);
        }
        assert_eq!(engine.snapshot.state, PlayerState::Playing);
        assert!(
            engine.snapshot.levels[0] > 0.1,
            "must measure the decoded sine wave"
        );
        assert_eq!(
            engine.snapshot.levels[1], engine.snapshot.levels[0],
            "a mono source must mirror the left channel"
        );
        engine.command(json!(["set_property", "volume", 20]));
        engine.command(json!(["keypress", "PLAY"]));
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && engine.snapshot.state != PlayerState::Paused {
            engine.tick().unwrap();
            thread::sleep(TICK);
        }
        assert_eq!(engine.snapshot.state, PlayerState::Paused);
        assert_eq!(engine.snapshot.levels, [0.0; 2]);
        engine.command(json!(["keypress", "PLAYPAUSE"]));
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && engine.snapshot.state != PlayerState::Playing {
            engine.tick().unwrap();
            thread::sleep(TICK);
        }
        assert_eq!(engine.snapshot.state, PlayerState::Playing);
        engine.command(json!(["keypress", "PREV"]));
        engine.command(json!(["keypress", "NEXT"]));
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && engine.media.len() < 2 {
            engine.tick().unwrap();
            thread::sleep(TICK);
        }
        assert_eq!(
            engine.media,
            vec![-1, 1],
            "media bindings must reach Rust over IPC"
        );
        let mut socket = UnixStream::connect(engine.directory.path().join("ipc")).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .write_all(b"{\"command\":[\"get_property\",\"volume\"],\"request_id\":42}\n")
            .unwrap();
        let mut reader = std::io::BufReader::new(socket);
        let mut line = String::new();
        loop {
            use std::io::BufRead;
            line.clear();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["request_id"] == 42 {
                assert_eq!(value["data"].as_f64(), Some(20.0));
                break;
            }
        }
    }
}
