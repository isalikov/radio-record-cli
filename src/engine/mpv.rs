//! The mpv-based engine: spawns mpv, speaks JSON IPC over a private Unix
//! socket, and feeds the audio meter from an `astats` lavfi filter. Kept as
//! the fallback engine; the native engine in `engine/native.rs` replaces it
//! when no mpv is installed.

use crate::api::Stream;
use crate::engine::EngineImpl;
use crate::error::{Error, Result};
use crate::player::{AudioMeter, PlayerState, Snapshot, Streams};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const TICK: Duration = Duration::from_millis(50);
// How long to wait for mpv to open its IPC socket. The first launch of a freshly
// installed mpv can take well over five seconds on macOS while the system verifies
// the binary and its libraries; later launches take a fraction of a second.
const IPC_STARTUP_GRACE: Duration = Duration::from_secs(20);
// Per-channel levels plus the Overall fallback: the stereo meter rides the same
// single af-metadata reply, with no extra IPC traffic; mono degrades to a mirror.
const METER: &str = "--af-add=@radiome_meter:lavfi=[astats=metadata=1:reset=1:measure_perchannel=RMS_level:measure_overall=RMS_level]";

pub(crate) struct MpvEngine {
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

impl EngineImpl for MpvEngine {
    fn play(&mut self, streams: Vec<Stream>) {
        self.snapshot = Snapshot {
            state: PlayerState::Buffering,
            ..Snapshot::default()
        };
        self.paused = false;
        self.command(json!(["set_property", "pause", false]));
        self.streams = Streams::new(streams);
        if let Some(stream) = self.streams.next() {
            self.snapshot.stream = Some(stream.url.clone());
            self.snapshot.stream_round = self.streams.round() + 1;
            self.load(&stream.url);
        }
    }

    fn pause(&mut self) {
        self.command(json!(["cycle", "pause"]));
    }

    fn set_volume(&mut self, volume: u8) {
        self.command(json!(["set_property", "volume", volume]));
    }

    fn tick(&mut self) -> Result<()> {
        self.tick_inner()
    }

    fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }

    fn take_media(&mut self) -> Vec<isize> {
        std::mem::take(&mut self.media)
    }
}

impl MpvEngine {
    pub(crate) fn new(volume: u8) -> Result<Self> {
        Self::build(volume, false)
    }

    fn build(volume: u8, null_audio: bool) -> Result<Self> {
        // macOS Unix socket paths must fit in 104 bytes.
        let directory = tempfile::Builder::new()
            .prefix("radiome-")
            .tempdir_in("/tmp")?;
        // Keep IPC keypress bindings; radiome owns macOS system media controls.
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
        #[cfg(target_os = "macos")]
        command.arg("--input-media-keys=no");
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
    fn tick_inner(&mut self) -> Result<()> {
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
        if let Some(stream) = self.streams.due(Instant::now()) {
            self.snapshot.stream = Some(stream.url.clone());
            self.snapshot.stream_round = self.streams.round() + 1;
            self.load(&stream.url);
        }
        if self.metered.elapsed() >= TICK {
            self.request(json!(["get_property", "af-metadata/radiome_meter"]), 10);
            self.metered = Instant::now();
            if self.meter_received.elapsed() > Duration::from_millis(250) {
                for level in &mut self.snapshot.levels {
                    *level *= 0.8;
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
                    crate::player::Failure::Load(stream) => {
                        self.snapshot.stream = Some(stream.url.clone());
                        self.snapshot.stream_round = self.streams.round() + 1;
                        self.load(&stream.url);
                        self.snapshot.state = PlayerState::Buffering;
                    }
                    crate::player::Failure::Wait => self.snapshot.state = PlayerState::Buffering,
                    crate::player::Failure::GiveUp(message) => {
                        self.snapshot.state = PlayerState::Error;
                        self.snapshot.error = Some(message);
                    }
                }
            }
            Some("end-file") if value["reason"] == "eof" => self.snapshot.state = PlayerState::Idle,
            _ => {}
        }
        if value["request_id"] == 10 && value["error"] == "success" {
            let data = &value["data"];
            let overall = rms(data, "Overall");
            let left = rms(data, "1").or(overall);
            let right = rms(data, "2");
            self.snapshot.levels = self.meter.update(left, right);
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

fn positive(value: Option<i64>) -> Option<u64> {
    value.filter(|value| *value > 0).map(|value| value as u64)
}

/// One RMS reading in dB from an `af-metadata` reply, e.g. channel `"1"` or
/// `"Overall"`, mirroring what the lavfi filter reports.
fn rms(metadata: &Value, channel: &str) -> Option<f64> {
    let key = format!("lavfi.astats.{channel}.RMS_level");
    metadata[key.as_str()]
        .as_str()
        .and_then(|value| value.parse::<f64>().ok())
}

impl Drop for MpvEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::StreamKind;
    use crate::player::host;

    // message() only touches engine fields, so a throwaway child process stands in
    // for mpv; it is killed by the Drop guard like the real one.
    fn engine_without_mpv() -> MpvEngine {
        MpvEngine {
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

    fn stream(url: &str) -> Stream {
        Stream {
            kind: StreamKind::Main,
            url: url.to_owned(),
        }
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
        engine.streams = Streams::new(vec![
            stream(&first),
            Stream {
                kind: StreamKind::Hls,
                url: second.clone(),
            },
        ]);
        engine.streams.next();
        engine.snapshot.stream = Some(first);
        engine.snapshot.stream_round = 1;
        engine.message(
            &json!({"event": "end-file", "reason": "error", "file_error": "loading failed"}),
        );
        assert_eq!(engine.snapshot.stream.as_deref(), Some(second.as_str()));
        assert_eq!(engine.snapshot.stream_round, 1);
        assert_eq!(engine.snapshot.state, PlayerState::Buffering);
        assert_eq!(host("https://a.example.com:443/x"), "a.example.com");
    }

    #[test]
    #[ignore = "requires mpv and local Unix sockets; uses silent audio output"]
    fn mpv_falls_back_measures_real_audio_and_accepts_volume_and_pause() {
        let mut engine = MpvEngine::build(65, true).unwrap();
        // The first stream cannot be opened, so the engine must fall back to the second.
        engine.streams = Streams::new(vec![
            stream("/nonexistent/radiome-stream.aacp"),
            stream("av://lavfi:sine=frequency=440:sample_rate=44100"),
        ]);
        let url = engine.streams.next().unwrap().url;
        engine.load(&url);
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            engine.tick_inner().unwrap();
            if engine.snapshot.state == PlayerState::Playing && engine.snapshot.levels[0] > 0.1 {
                break;
            }
            std::thread::sleep(TICK);
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
            engine.tick_inner().unwrap();
            std::thread::sleep(TICK);
        }
        assert_eq!(engine.snapshot.state, PlayerState::Paused);
        assert_eq!(engine.snapshot.levels, [0.0; 2]);
        engine.command(json!(["keypress", "PLAYPAUSE"]));
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && engine.snapshot.state != PlayerState::Playing {
            engine.tick_inner().unwrap();
            std::thread::sleep(TICK);
        }
        assert_eq!(engine.snapshot.state, PlayerState::Playing);
        engine.command(json!(["keypress", "PREV"]));
        engine.command(json!(["keypress", "NEXT"]));
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && engine.media.len() < 2 {
            engine.tick_inner().unwrap();
            std::thread::sleep(TICK);
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
