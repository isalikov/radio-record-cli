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
const METER: &str = "--af-add=@radiome_meter:lavfi=[astats=metadata=1:reset=1:measure_perchannel=none:measure_overall=RMS_level]";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayerState {
    #[default]
    Idle,
    Buffering,
    Playing,
    Paused,
    Error,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub state: PlayerState,
    pub level: f64,
    pub error: Option<String>,
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
        for (generation, update) in self.rx.try_iter() {
            if generation == self.generation {
                self.snapshot = update;
            }
        }
    }

    pub fn media_commands(&self) -> Vec<isize> {
        self.media_rx.try_iter().collect()
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
                                    level: 0.0,
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
                    engine.fallbacks = urls.into();
                    engine.load_next();
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
                        level: 0.0,
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
    fallbacks: VecDeque<String>,
    meter: AudioMeter,
    meter_received: Instant,
    child: Child,
    directory: tempfile::TempDir,
    socket: Option<UnixStream>,
    input: Vec<u8>,
    output: Vec<u8>,
    started: Instant,
    metered: Instant,
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
            fallbacks: VecDeque::new(),
            meter: AudioMeter::default(),
            meter_received: Instant::now(),
            child,
            directory,
            socket: None,
            input: Vec::new(),
            output: Vec::new(),
            started: Instant::now(),
            metered: Instant::now(),
            paused: false,
            snapshot: Snapshot::default(),
        })
    }
    fn load_next(&mut self) -> bool {
        let Some(url) = self.fallbacks.pop_front() else {
            return false;
        };
        self.command(json!(["loadfile", url, "replace"]));
        true
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
                    for (id, property) in [(1, "pause"), (2, "core-idle")] {
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
        if self.metered.elapsed() >= TICK {
            self.request(json!(["get_property", "af-metadata/radiome_meter"]), 10);
            self.metered = Instant::now();
            if self.meter_received.elapsed() > Duration::from_millis(250) {
                self.snapshot.level *= 0.8;
            }
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
            self.snapshot.level = 0.0;
            self.meter = AudioMeter::default();
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
                self.snapshot.level = 0.0;
                self.snapshot.state = PlayerState::Buffering;
                self.snapshot.error = None;
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
                _ => {}
            },
            Some("end-file") if value["reason"] == "error" => {
                if self.load_next() {
                    self.snapshot.state = PlayerState::Buffering;
                    return;
                }
                self.snapshot.state = PlayerState::Error;
                self.snapshot.error = Some(format!(
                    "Stream unavailable: {}",
                    value["file_error"].as_str().unwrap_or("mpv error")
                ));
            }
            Some("end-file") if value["reason"] == "eof" => self.snapshot.state = PlayerState::Idle,
            _ => {}
        }
        if value["request_id"] == 10 && value["error"] == "success" {
            self.snapshot.level = self.meter.update(&value["data"]);
            self.meter_received = Instant::now();
        }
    }
}

#[derive(Default)]
struct AudioMeter {
    samples: VecDeque<f64>,
    level: f64,
}

impl AudioMeter {
    fn update(&mut self, metadata: &Value) -> f64 {
        let db = metadata["lavfi.astats.Overall.RMS_level"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok());
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

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_audio_updates_cannot_restore_a_stopped_station() {
        let (tx, _commands) = mpsc::channel();
        let (updates, rx) = mpsc::channel();
        let mut player = Player {
            tx,
            rx,
            media_rx: mpsc::channel().1,
            generation: 0,
            worker: None,
            snapshot: Snapshot::default(),
        };
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
    fn meter_rejects_missing_and_non_finite_levels() {
        let mut meter = AudioMeter::default();
        for value in ["-inf", "NaN", "inf", "broken"] {
            assert_eq!(
                meter.update(&json!({"lavfi.astats.Overall.RMS_level": value})),
                0.0
            );
        }
        assert_eq!(meter.update(&json!({})), 0.0);
    }

    #[test]
    fn loud_radio_has_headroom_and_a_smooth_wide_range() {
        let mut meter = AudioMeter::default();
        for _ in 0..80 {
            meter.update(&json!({"lavfi.astats.Overall.RMS_level": "-6"}));
        }
        assert!((meter.level - 0.5).abs() < 0.01);
        let mut values = Vec::new();
        for db in [-4, -4, -4, -8, -8, -8, -8, -8, -8] {
            values.push(meter.update(&json!({"lavfi.astats.Overall.RMS_level": db.to_string()})));
        }
        assert!(values[2] > 0.8 && values[8] < 0.3, "{values:?}");
        assert!(
            values
                .windows(2)
                .all(|pair| (pair[1] - pair[0]).abs() < 0.3)
        );
        assert!(values.iter().all(|value| *value < 0.96));
        for _ in 0..30 {
            meter.update(&json!({"lavfi.astats.Overall.RMS_level": "-inf"}));
        }
        assert_eq!(meter.level, 0.0);
    }
    #[test]
    #[ignore = "requires mpv and local Unix sockets; uses silent audio output"]
    fn mpv_falls_back_measures_real_audio_and_accepts_volume_and_pause() {
        let mut engine = Engine::new(65, true).unwrap();
        // The first stream cannot be opened, so the engine must fall back to the second.
        engine.fallbacks = VecDeque::from([
            "/nonexistent/radiome-stream.aacp".to_owned(),
            "av://lavfi:sine=frequency=440:sample_rate=44100".to_owned(),
        ]);
        engine.load_next();
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            engine.tick().unwrap();
            if engine.snapshot.state == PlayerState::Playing && engine.snapshot.level > 0.1 {
                break;
            }
            thread::sleep(TICK);
        }
        assert_eq!(engine.snapshot.state, PlayerState::Playing);
        assert!(
            engine.snapshot.level > 0.1,
            "must measure the decoded sine wave"
        );
        engine.command(json!(["set_property", "volume", 20]));
        engine.command(json!(["keypress", "PLAY"]));
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && engine.snapshot.state != PlayerState::Paused {
            engine.tick().unwrap();
            thread::sleep(TICK);
        }
        assert_eq!(engine.snapshot.state, PlayerState::Paused);
        assert_eq!(engine.snapshot.level, 0.0);
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
