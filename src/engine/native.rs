//! The native audio engine: pure-Rust decode (Symphonia AAC-LC over ADTS)
//! feeding an SPSC ring that a cpal output callback drains. No external
//! processes or system packages; on Linux the only runtime dependency is
//! libasound (loaded by cpal).
//!
//! Thread model (all shared state is either an mpsc channel, an SPSC ring,
//! or an atomic — never an application lock):
//!
//! ```text
//! worker (supervisor, 50 ms)  ← mpsc ReaderEvent — reader thread (HTTP+decode)
//!        ↑ atomics                                  ↓ interleaved f32
//!        |                                       rtrb ring (fixed capacity)
//! audio callback (cpal) ← pops samples, applies volume, counts underruns
//! ```
//!
//! During pause the reader drains the stream and the callback clears queued
//! samples. Resume buffers fresh audio; HLS rejoins recent published segments.

use crate::api::{Stream, StreamKind};
use crate::engine::{EngineImpl, native_plays};
use crate::error::{Error, Result};
use crate::player::{AudioMeter, Failure, PlayerState, Snapshot, Streams};
use rtrb::{Consumer, Producer, RingBuffer};
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::{
    AudioCodecParameters, AudioDecoderOptions, CODEC_ID_NULL_AUDIO, well_known::CODEC_ID_AAC,
};
use symphonia::core::errors::Error as SymphError;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSourceStream, ReadOnlySource};

// Fixed capacity: 3 seconds of 48 kHz stereo, 1.5 seconds at 96 kHz.
const RING_CAPACITY: usize = 48_000 * 2 * 3;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
// Idle read timeout for the endless ADTS body; a server that goes quiet for
// this long fails the URL and the fallback machine moves on.
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const HLS_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PLAYLIST_BYTES: u64 = 256 * 1024;
const MAX_SEGMENT_BYTES: u64 = 4 * 1024 * 1024;

/// A running output: dropping it stops consuming the ring.
pub(crate) trait SinkHandle: Send {
    /// Whether the output device reported a fatal error.
    fn failed(&self) -> bool;
}

/// Builds the consuming side: the real one drives a cpal output stream,
/// tests use a null sink that drains at real-time pace.
pub(crate) type SinkFactory =
    fn(Consumer<f32>, Arc<OutputState>, u32) -> Result<Box<dyn SinkHandle>>;

/// Controls and telemetry shared with the output callback. The callback owns
/// ring consumption, buffering, and the pre-gain meter; the reader owns writes.
pub(crate) struct OutputState {
    volume: AtomicU32,
    paused: AtomicBool,
    pause_epoch: AtomicU64,
    fill: AtomicU32,
    buffering: AtomicBool,
    underruns: AtomicU32,
    left_db: AtomicU32,
    right_db: AtomicU32,
    meter_sequence: AtomicU64,
}

impl OutputState {
    fn new(volume: u8) -> Self {
        Self {
            volume: AtomicU32::new(u32::from(volume.min(100))),
            paused: AtomicBool::new(false),
            pause_epoch: AtomicU64::new(0),
            fill: AtomicU32::new(0),
            buffering: AtomicBool::new(true),
            underruns: AtomicU32::new(0),
            left_db: AtomicU32::new(f32::NEG_INFINITY.to_bits()),
            right_db: AtomicU32::new(f32::NEG_INFINITY.to_bits()),
            meter_sequence: AtomicU64::new(0),
        }
    }
}

enum ReaderEvent {
    Telemetry {
        codec: Option<String>,
        samplerate: Option<u64>,
        channels: Option<u64>,
    },
    Bitrate(u64),
    StreamChanged {
        url: String,
        round: u32,
        consumer: Consumer<f32>,
    },
    GaveUp(String),
}

struct ReaderShared {
    generation: Arc<AtomicU64>,
    my_generation: u64,
    output: Arc<OutputState>,
}

impl ReaderShared {
    fn alive(&self) -> bool {
        self.generation.load(Ordering::Relaxed) == self.my_generation
    }
    fn paused(&self) -> bool {
        self.output.paused.load(Ordering::Acquire)
    }
}

enum StopReason {
    /// The engine moved on (station switch or stop); exit immediately.
    Generation,
    Error(String),
}

pub(crate) struct NativeEngine {
    generation: Arc<AtomicU64>,
    output: Arc<OutputState>,
    meter_seen: u64,
    events: Option<Receiver<ReaderEvent>>,
    sink_factory: SinkFactory,
    consumer: Option<Consumer<f32>>,
    sink: Option<Box<dyn SinkHandle>>,
    meter: AudioMeter,
    snapshot: Snapshot,
}

impl EngineImpl for NativeEngine {
    fn play(&mut self, streams: Vec<Stream>) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.sink = None;
        self.consumer = None;
        let volume = self.output.volume.load(Ordering::Relaxed) as u8;
        self.output = Arc::new(OutputState::new(volume));
        self.meter_seen = 0;
        self.meter = AudioMeter::default();
        self.snapshot = Snapshot::default();
        self.events = None;
        let playable: Vec<Stream> = streams
            .into_iter()
            .filter(|stream| native_plays(stream.kind))
            .collect();
        if playable.is_empty() {
            self.snapshot.state = PlayerState::Error;
            self.snapshot.error =
                Some("HE-AAC stream needs mpv · install mpv or Enter to retry".into());
            return;
        }
        self.snapshot.state = PlayerState::Buffering;
        let (event_tx, event_rx) = mpsc::channel();
        self.events = Some(event_rx);
        let shared = ReaderShared {
            generation: self.generation.clone(),
            my_generation: self.generation.load(Ordering::Relaxed),
            output: self.output.clone(),
        };
        thread::spawn(move || reader(shared, Streams::new(playable), event_tx));
    }

    fn pause(&mut self) {
        if self.snapshot.state == PlayerState::Error {
            return;
        }
        let paused = !self.output.paused.load(Ordering::Acquire);
        // Both transitions invalidate queued audio, even if pause and resume
        // arrive between callbacks. The reader also abandons an in-flight block.
        self.output.pause_epoch.fetch_add(1, Ordering::AcqRel);
        self.output.paused.store(paused, Ordering::Release);
        self.output.buffering.store(true, Ordering::Release);
        self.snapshot.state = if paused {
            PlayerState::Paused
        } else {
            PlayerState::Buffering
        };
    }

    fn set_volume(&mut self, volume: u8) {
        self.output
            .volume
            .store(u32::from(volume.min(100)), Ordering::Relaxed);
    }

    fn tick(&mut self) -> Result<()> {
        if let Some(events) = self.events.take() {
            for event in events.try_iter() {
                self.handle_event(event);
            }
            self.events = Some(events);
        }
        if self.sink.as_ref().is_some_and(|sink| sink.failed()) {
            self.fail("audio device failed · Enter to retry".into());
        }
        let fill = self.output.fill.load(Ordering::Relaxed) as usize;
        if self.snapshot.state != PlayerState::Error {
            self.snapshot.state = if self.output.paused.load(Ordering::Acquire) {
                PlayerState::Paused
            } else if self.sink.is_some() && !self.output.buffering.load(Ordering::Acquire) {
                PlayerState::Playing
            } else {
                PlayerState::Buffering
            };
        }
        self.snapshot.underruns = self.output.underruns.load(Ordering::Relaxed);
        let rate = self.snapshot.samplerate.unwrap_or(44_100) as usize;
        let start_fill = start_fill(rate);
        self.snapshot.buffering = (self.snapshot.state == PlayerState::Buffering)
            .then_some((fill * 100 / start_fill).min(100) as u8);
        self.snapshot.cache_seconds = matches!(
            self.snapshot.state,
            PlayerState::Playing | PlayerState::Buffering
        )
        .then_some(fill as f64 / (rate * 2) as f64);
        if self.snapshot.state == PlayerState::Playing {
            let sequence = self.output.meter_sequence.load(Ordering::Acquire);
            if sequence != self.meter_seen {
                self.meter_seen = sequence;
                self.snapshot.levels = self.meter.update(
                    Some(f64::from(f32::from_bits(
                        self.output.left_db.load(Ordering::Relaxed),
                    ))),
                    Some(f64::from(f32::from_bits(
                        self.output.right_db.load(Ordering::Relaxed),
                    ))),
                );
            }
        } else {
            self.snapshot.levels = [0.0; 2];
            self.meter = AudioMeter::default();
        }
        Ok(())
    }

    fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }
    fn take_media(&mut self) -> Vec<isize> {
        Vec::new()
    }
}

impl NativeEngine {
    pub(crate) fn new(volume: u8, sink_factory: SinkFactory) -> Self {
        Self {
            generation: Arc::new(AtomicU64::new(0)),
            output: Arc::new(OutputState::new(volume)),
            meter_seen: 0,
            events: None,
            sink_factory,
            consumer: None,
            sink: None,
            meter: AudioMeter::default(),
            snapshot: Snapshot::default(),
        }
    }

    fn fail(&mut self, message: String) {
        self.sink = None;
        self.consumer = None;
        self.snapshot.state = PlayerState::Error;
        self.snapshot.error = Some(message);
        self.output.fill.store(0, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    fn handle_event(&mut self, event: ReaderEvent) {
        if self.snapshot.state == PlayerState::Error {
            return;
        }
        match event {
            ReaderEvent::Telemetry {
                codec,
                samplerate,
                channels,
            } => {
                self.snapshot.codec = codec;
                self.snapshot.samplerate = samplerate;
                self.snapshot.channels = channels;
                if self.sink.is_none()
                    && let Some(rate) = samplerate
                {
                    self.open_sink(rate as u32);
                }
            }
            ReaderEvent::Bitrate(bitrate) => self.snapshot.bitrate = Some(bitrate),
            ReaderEvent::StreamChanged {
                url,
                round,
                consumer,
            } => {
                // Every fallback gets a fresh ring and output stream: no old
                // samples, resampler state, or sample rate survives a URL change.
                self.sink = None;
                self.output.underruns.store(0, Ordering::Relaxed);
                self.consumer = Some(consumer);
                self.meter = AudioMeter::default();
                self.meter_seen = self.output.meter_sequence.load(Ordering::Acquire);
                self.output.fill.store(0, Ordering::Relaxed);
                self.output.buffering.store(true, Ordering::Release);
                self.snapshot = Snapshot {
                    state: PlayerState::Buffering,
                    stream: Some(url),
                    stream_round: round,
                    ..Snapshot::default()
                };
            }
            ReaderEvent::GaveUp(message) => self.fail(message),
        }
    }

    fn open_sink(&mut self, rate: u32) {
        let Some(consumer) = self.consumer.take() else {
            return;
        };
        match (self.sink_factory)(consumer, self.output.clone(), rate) {
            Ok(handle) => self.sink = Some(handle),
            Err(err) => self.fail(format!("audio device error: {err} · Enter to retry")),
        }
    }
}

impl Drop for NativeEngine {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

fn start_fill(rate: usize) -> usize {
    (rate * 3 / 10).max(1) * 2
}

/// All mutable audio state belongs to the callback. This implementation is
/// also used by the null sink so offline tests exercise real output behavior.
struct OutputCallback {
    consumer: Consumer<f32>,
    output: Arc<OutputState>,
    resample: Resample,
    epoch: u64,
    start_fill: usize,
    playing: bool,
    meter_window: usize,
    meter_count: usize,
    squares: [f64; 2],
}

impl OutputCallback {
    fn new(consumer: Consumer<f32>, output: Arc<OutputState>, rate: u32, device_rate: u32) -> Self {
        Self {
            consumer,
            epoch: output.pause_epoch.load(Ordering::Acquire),
            output,
            resample: Resample::new(rate, device_rate),
            start_fill: start_fill(rate as usize),
            playing: false,
            meter_window: (device_rate as usize / 20).max(1),
            meter_count: 0,
            squares: [0.0; 2],
        }
    }

    fn fill<T: cpal::Sample + cpal::FromSample<f32>>(&mut self, data: &mut [T]) {
        let epoch = self.output.pause_epoch.load(Ordering::Acquire);
        let paused = self.output.paused.load(Ordering::Acquire);
        if epoch != self.epoch || paused {
            // Bound the drain to the samples already queued. The producer may
            // be running concurrently, so never loop until it stops writing.
            let queued = self.consumer.slots();
            for _ in 0..queued {
                let _ = self.consumer.pop();
            }
            self.resample = Resample::new(self.resample.in_rate, self.resample.out_rate);
            self.epoch = epoch;
            self.playing = false;
            self.meter_count = 0;
            self.squares = [0.0; 2];
        }
        if !paused && !self.playing && self.consumer.slots() >= self.start_fill {
            self.playing = true;
        }
        let gain = self.output.volume.load(Ordering::Relaxed) as f32 / 100.0;
        let mut starved = false;
        for frame in data.chunks_mut(2) {
            let samples = if !paused && self.playing {
                match self.resample.frame(&mut self.consumer) {
                    Some(samples) => {
                        self.measure(samples);
                        samples
                    }
                    None => {
                        self.playing = false;
                        self.meter_count = 0;
                        self.squares = [0.0; 2];
                        starved = true;
                        [0.0; 2]
                    }
                }
            } else {
                [0.0; 2]
            };
            frame[0] = T::from_sample((samples[0] * gain).clamp(-1.0, 1.0));
            if let Some(right) = frame.get_mut(1) {
                *right = T::from_sample((samples[1] * gain).clamp(-1.0, 1.0));
            }
        }
        if starved {
            self.output.underruns.fetch_add(1, Ordering::Relaxed);
        }
        self.output
            .fill
            .store(self.consumer.slots() as u32, Ordering::Relaxed);
        self.output
            .buffering
            .store(!self.playing, Ordering::Release);
    }

    fn measure(&mut self, samples: [f32; 2]) {
        for (square, sample) in self.squares.iter_mut().zip(samples) {
            *square += f64::from(sample).powi(2);
        }
        self.meter_count += 1;
        if self.meter_count == self.meter_window {
            let db = self
                .squares
                .map(|sum| (10.0 * (sum / self.meter_count as f64).log10()) as f32);
            self.output
                .left_db
                .store(db[0].to_bits(), Ordering::Relaxed);
            self.output
                .right_db
                .store(db[1].to_bits(), Ordering::Relaxed);
            self.output.meter_sequence.fetch_add(1, Ordering::Release);
            self.meter_count = 0;
            self.squares = [0.0; 2];
        }
    }
}

/// Whether a default output device exists, so engine selection can fall
/// back to mpv before a station is even tried.
pub(crate) fn device_available() -> bool {
    use cpal::traits::HostTrait;
    cpal::default_host().default_output_device().is_some()
}

/// Frame-rate conversion between the stream and the output device using
/// linear interpolation, so Bluetooth headsets that only offer 48 kHz can
/// still play 44.1 kHz radio. All state lives inside the output callback:
/// no locks, no allocation. Upsampling only — a device slower than the
/// stream is refused instead of aliased.
struct Resample {
    in_rate: u32,
    out_rate: u32,
    /// Input frames consumed per output frame: 1.0 is a pass-through.
    step: f32,
    frac: f32,
    a: [f32; 2],
    b: [f32; 2],
    primed: bool,
}

impl Resample {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        Self {
            in_rate,
            out_rate,
            step: in_rate as f32 / out_rate as f32,
            frac: 0.0,
            a: [0.0; 2],
            b: [0.0; 2],
            primed: false,
        }
    }

    /// One output frame, popping input frames from the ring as needed.
    /// `None` means the ring ran dry; state is kept so playback resumes
    /// cleanly when data returns.
    fn frame(&mut self, consumer: &mut Consumer<f32>) -> Option<[f32; 2]> {
        if self.in_rate == self.out_rate {
            return pop_frame(consumer);
        }
        if !self.primed {
            if consumer.slots() < 4 {
                return None;
            }
            self.a = pop_frame(consumer)?;
            self.b = pop_frame(consumer)?;
            self.primed = true;
        }
        while self.frac >= 1.0 {
            let next = pop_frame(consumer)?;
            self.a = self.b;
            self.b = next;
            self.frac -= 1.0;
        }
        let t = self.frac;
        let frame = [
            self.a[0] + (self.b[0] - self.a[0]) * t,
            self.a[1] + (self.b[1] - self.a[1]) * t,
        ];
        self.frac += self.step;
        Some(frame)
    }
}

fn pop_frame(consumer: &mut Consumer<f32>) -> Option<[f32; 2]> {
    if consumer.slots() < 2 {
        return None;
    }
    let left = consumer.pop().ok()?;
    let right = consumer.pop().unwrap_or(left);
    Some([left, right])
}

/// The device rate to open for a stream of `rate` Hz: the stream rate when
/// the device supports it, else the closest supported rate that is not below
/// it. Prefer the current device rate when it supports upsampling or pass-through.
/// `None` means every config is slower than the stream.
fn device_rate(min: u32, max: u32, rate: u32, current: Option<u32>) -> Option<u32> {
    // Keeping the device clock avoids a CoreAudio rate-change handshake, which
    // can time out on headphones even when the advertised range includes rate.
    if let Some(current) = current
        && current >= rate
        && min <= current
        && current <= max
    {
        return Some(current);
    }
    if min <= rate && rate <= max {
        Some(rate)
    } else if rate < min {
        Some(min)
    } else {
        None
    }
}

/// CPAL also reports nonfatal notifications through its error callback.
/// Keep this path allocation-free: overload notifications can run on the
/// real-time audio thread. A fatal failure stays latched until the sink drops.
fn handle_output_error(kind: cpal::ErrorKind, failed: &AtomicBool) {
    if !matches!(
        kind,
        cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied
    ) {
        failed.store(true, Ordering::Relaxed);
    }
}

/// The production sink: a cpal output stream whose callback drains the ring.
pub(crate) fn cpal_sink(
    consumer: Consumer<f32>,
    output: Arc<OutputState>,
    rate: u32,
) -> Result<Box<dyn SinkHandle>> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SizedSample};

    fn build_stream<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        failed: std::sync::Arc<AtomicBool>,
        callback: OutputCallback,
    ) -> Result<cpal::Stream>
    where
        T: SizedSample + FromSample<f32>,
    {
        let error_callback = {
            let failed = failed.clone();
            move |error: cpal::Error| {
                handle_output_error(error.kind(), &failed);
            }
        };
        let mut callback = callback;
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _| callback.fill(data),
                error_callback,
                None,
            )
            .map_err(|err| Error::new(format!("failed to open audio stream: {err}")))
    }

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| Error::new("no audio output device"))?;
    let configs = device
        .supported_output_configs()
        .map_err(|err| Error::new(format!("audio device query failed: {err}")))?;
    let current_rate = device
        .default_output_config()
        .ok()
        .map(|config| config.sample_rate());
    // Preserve the device clock whenever possible; otherwise choose the closest
    // supported rate not below the stream. Faster output is resampled up.
    let supported = configs
        .into_iter()
        .filter(|config| {
            config.channels() == 2
                && matches!(
                    config.sample_format(),
                    cpal::SampleFormat::F32 | cpal::SampleFormat::I16
                )
        })
        .filter_map(|config| {
            device_rate(
                config.min_sample_rate(),
                config.max_sample_rate(),
                rate,
                current_rate,
            )
            .map(|open_rate| (config, open_rate))
        })
        .min_by_key(|&(_, open_rate)| (Some(open_rate) != current_rate, open_rate))
        .ok_or_else(|| {
            Error::new(format!(
                "the audio device does not support {rate} Hz stereo"
            ))
        })?;
    let (supported, open_rate) = (supported.0, supported.1);
    let config = cpal::StreamConfig {
        channels: 2,
        sample_rate: open_rate,
        buffer_size: cpal::BufferSize::Default,
    };
    let callback = OutputCallback::new(consumer, output, rate, open_rate);
    let failed = std::sync::Arc::new(AtomicBool::new(false));
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(&device, config, failed.clone(), callback)?,
        cpal::SampleFormat::I16 => build_stream::<i16>(&device, config, failed.clone(), callback)?,
        other => {
            return Err(Error::new(format!(
                "unsupported audio sample format: {other:?}"
            )));
        }
    };
    stream
        .play()
        .map_err(|err| Error::new(format!("failed to start audio stream: {err}")))?;
    Ok(Box::new(CpalHandle { stream, failed }))
}

struct CpalHandle {
    // Held to keep the output stream alive: dropping a cpal Stream stops it.
    #[allow(dead_code)]
    stream: cpal::Stream,
    failed: std::sync::Arc<AtomicBool>,
}

impl SinkHandle for CpalHandle {
    fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
fn fixture_path(name: &str) -> String {
    format!(
        "{}/tests/fixtures/{}",
        env!("CARGO_MANIFEST_DIR"),
        name.strip_prefix("fixtures/")
            .unwrap_or(name)
            .split('?')
            .next()
            .unwrap_or(name)
    )
}

/// Reads a fixture file at streaming pace so tests exercise the same
/// pacing as a network body.
#[cfg(test)]
struct ThrottledReader {
    inner: std::io::Cursor<Vec<u8>>,
    pos: usize,
    started: Instant,
    rate: f64,
}

#[cfg(test)]
impl Read for ThrottledReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.pos >= self.inner.get_ref().len() {
            return Ok(0);
        }
        let available = loop {
            let allowed = (self.started.elapsed().as_secs_f64() * self.rate) as usize;
            let available = allowed.saturating_sub(self.pos).min(buf.len());
            if available > 0 {
                break available;
            }
            thread::sleep(Duration::from_millis(20));
        };
        let read = self.inner.read(&mut buf[..available])?;
        self.pos += read;
        Ok(read)
    }
}

#[cfg(test)]
fn throttled_fixture(name: &str) -> std::io::Result<ThrottledReader> {
    Ok(ThrottledReader {
        inner: std::io::Cursor::new(std::fs::read(fixture_path(name))?),
        pos: 0,
        started: Instant::now(),
        rate: 12_000.0,
    })
}

fn reader(shared: ReaderShared, mut streams: Streams, events: Sender<ReaderEvent>) {
    let mut current = streams.next();
    while let Some(stream) = current {
        if !shared.alive() {
            return;
        }
        let (mut producer, consumer) = RingBuffer::new(RING_CAPACITY);
        let _ = events.send(ReaderEvent::StreamChanged {
            url: stream.url.clone(),
            round: streams.round() + 1,
            consumer,
        });
        let result = match stream.kind {
            StreamKind::Hls => run_hls(&stream, &shared, &mut producer, &events),
            _ => run_adts(&stream, &shared, &mut producer, &events),
        };
        match result {
            StopReason::Generation => return,
            StopReason::Error(reason) => match streams.fail(&reason, Instant::now()) {
                Failure::Load(next) => current = Some(next),
                Failure::Wait => loop {
                    if !shared.alive() {
                        return;
                    }
                    if let Some(next) = streams.due(Instant::now()) {
                        current = Some(next);
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                },
                Failure::GiveUp(message) => {
                    let _ = events.send(ReaderEvent::GaveUp(message));
                    return;
                }
            },
        }
    }
    let _ = events.send(ReaderEvent::GaveUp("no playable stream".into()));
}

/// A direct Icecast ADTS stream, decoded frame by frame as the body arrives.
fn run_adts(
    stream: &Stream,
    shared: &ReaderShared,
    producer: &mut Producer<f32>,
    events: &Sender<ReaderEvent>,
) -> StopReason {
    let body = match open_body(stream) {
        Ok(body) => body,
        Err(err) => return StopReason::Error(format!("loading failed: {err}")),
    };
    let mss = MediaSourceStream::new(Box::new(ReadOnlySource::new(body)), Default::default());
    let mut format =
        match symphonia::default::formats::AdtsReader::try_new(mss, FormatOptions::default()) {
            Ok(format) => format,
            Err(err) => return StopReason::Error(format!("unsupported stream format: {err}")),
        };
    let Some(track) = format.default_track(TrackType::Audio) else {
        return StopReason::Error("no audio track in stream".into());
    };
    let Some(params) = track
        .codec_params
        .as_ref()
        .and_then(CodecParameters::audio)
        .cloned()
    else {
        return StopReason::Error("no audio track in stream".into());
    };
    let info = match track_info(&params) {
        Ok(info) => info,
        Err(reason) => return StopReason::Error(reason),
    };
    send_telemetry(events, &info);
    let track_id = track.id;
    let mut decoder = match symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
    {
        Ok(decoder) => decoder,
        Err(err) => return StopReason::Error(format!("unsupported codec: {err}")),
    };
    let mut pipe = Pipe::new(producer);
    let mut interleaved: Vec<f32> = Vec::new();
    let mut stereo: Vec<f32> = Vec::new();
    let mut bytes = 0u64;
    let mut frames = 0u64;
    loop {
        if !shared.alive() {
            return StopReason::Generation;
        }
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            // The server closed the stream, the read timed out, or the source
            // ended: fall through to the next URL instead of freezing.
            Ok(None) | Err(SymphError::IoError(_)) => {
                return StopReason::Error("stream ended".into());
            }
            Err(SymphError::DecodeError(_)) => continue,
            Err(SymphError::ResetRequired) => return StopReason::Error("stream reset".into()),
            Err(err) => return StopReason::Error(format!("stream error: {err}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        bytes += packet.data.len() as u64;
        frames += 1;
        if frames.is_multiple_of(20) {
            // One AAC frame is 1024 samples: frame size × rate / 1024 ≈ bitrate.
            let bitrate = bytes * 8 * u64::from(info.rate) / (frames * 1024);
            let _ = events.send(ReaderEvent::Bitrate(bitrate));
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphError::DecodeError(_)) | Err(SymphError::IoError(_)) => continue,
            Err(err) => return StopReason::Error(format!("decoder error: {err}")),
        };
        decoded.copy_to_vec_interleaved::<f32>(&mut interleaved);
        if info.channels == 1 {
            stereo.clear();
            for sample in &interleaved {
                stereo.push(*sample);
                stereo.push(*sample);
            }
            pipe.push(&stereo, shared);
        } else {
            pipe.push(&interleaved, shared);
        }
    }
}

struct TrackInfo {
    rate: u32,
    channels: u64,
    codec: Option<&'static str>,
}

fn track_info(params: &AudioCodecParameters) -> std::result::Result<TrackInfo, String> {
    if params.codec == CODEC_ID_NULL_AUDIO {
        return Err("no audio track in stream".to_owned());
    }
    let rate = params
        .sample_rate
        .filter(|rate| (8_000..=96_000).contains(rate))
        .ok_or_else(|| "stream sample rate is not supported".to_owned())?;
    let channels = params
        .channels
        .as_ref()
        .map(|channels| channels.count())
        .filter(|count| matches!(count, 1 | 2))
        .ok_or_else(|| "stream must be mono or stereo".to_owned())? as u64;
    let codec = (params.codec == CODEC_ID_AAC).then_some("aac");
    Ok(TrackInfo {
        rate,
        channels,
        codec,
    })
}

fn send_telemetry(events: &Sender<ReaderEvent>, info: &TrackInfo) {
    let _ = events.send(ReaderEvent::Telemetry {
        codec: info.codec.map(str::to_owned),
        samplerate: Some(u64::from(info.rate)),
        channels: Some(info.channels),
    });
}

fn open_body(stream: &Stream) -> Result<Box<dyn Read + Send + Sync>> {
    #[cfg(test)]
    if let Some(path) = stream.url.strip_prefix("fixture://") {
        let reader = throttled_fixture(path)
            .map_err(|err| Error::new(format!("fixture read failed: {err}")))?;
        return Ok(Box::new(reader));
    }
    let agent = stream_http::AgentBuilder::new()
        .user_agent(crate::api::USER_AGENT)
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build();
    let response = agent
        .get(&stream.url)
        .call()
        .map_err(|err| Error::new(format!("{err}")))?;
    Ok(Box::new(response.into_reader()))
}

fn hls_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(crate::api::USER_AGENT)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(HLS_TIMEOUT))
        .build()
        .into()
}

fn get_text(http: &ureq::Agent, url: &str) -> Result<String> {
    #[cfg(test)]
    if let Some(path) = url.strip_prefix("fixture://") {
        return std::fs::read_to_string(fixture_path(path))
            .map_err(|err| Error::new(format!("fixture read failed: {err}")));
    }
    let mut response = http
        .get(url)
        .call()
        .map_err(|err| Error::new(format!("{err}")))?;
    let mut text = String::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_PLAYLIST_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_PLAYLIST_BYTES {
        return Err(Error::new("HLS playlist too large"));
    }
    Ok(text)
}

fn get_bytes(http: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    #[cfg(test)]
    if let Some(path) = url.strip_prefix("fixture://") {
        return std::fs::read(fixture_path(path))
            .map_err(|err| Error::new(format!("fixture read failed: {err}")));
    }
    let mut response = http
        .get(url)
        .call()
        .map_err(|err| Error::new(format!("{err}")))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_SEGMENT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SEGMENT_BYTES {
        return Err(Error::new("HLS segment too large"));
    }
    Ok(bytes)
}

/// The HLS variant: fetch the master playlist, pick the AAC-LC variant, then
/// join the live edge and play segments as they are published.
fn run_hls(
    stream: &Stream,
    shared: &ReaderShared,
    producer: &mut Producer<f32>,
    events: &Sender<ReaderEvent>,
) -> StopReason {
    let http = hls_agent();
    let master = match get_text(&http, &stream.url) {
        Ok(text) => text,
        Err(err) => return StopReason::Error(format!("loading failed: {err}")),
    };
    let Some(media_url) = crate::engine::hls::pick_lc_variant(&master, &stream.url) else {
        return StopReason::Error("no AAC-LC HLS variant".into());
    };
    let mut next_seq: Option<u64> = None;
    let mut pipe: Option<Pipe> = None;
    let mut stream_rate = None;
    let mut poll_at = Instant::now();
    let mut last_progress = Instant::now();
    let mut pause_epoch = shared.output.pause_epoch.load(Ordering::Acquire);
    let mut joining = true;
    let mut bad_segments = 0;
    loop {
        if !shared.alive() {
            return StopReason::Generation;
        }
        if Instant::now() < poll_at {
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        let text = match get_text(&http, &media_url) {
            Ok(text) => text,
            Err(err) => return StopReason::Error(format!("loading failed: {err}")),
        };
        let playlist = match crate::engine::hls::parse_media(&text) {
            Ok(playlist) => playlist,
            Err(err) => return StopReason::Error(format!("bad HLS playlist: {err}")),
        };
        if last_progress.elapsed()
            > Duration::from_secs_f64(
                (playlist.target_duration * 3.0).max(HLS_TIMEOUT.as_secs_f64()),
            )
        {
            return StopReason::Error("HLS playlist stopped producing decodable audio".into());
        }
        let epoch = shared.output.pause_epoch.load(Ordering::Acquire);
        if epoch != pause_epoch && !shared.paused() {
            next_seq = None;
            joining = true;
        }
        pause_epoch = epoch;
        poll_at = Instant::now()
            + Duration::from_secs_f64((playlist.target_duration / 2.0).clamp(1.0, 5.0));
        if next_seq.is_none() {
            // Two published segments cover the next publication interval.
            // Starting with only the last 1.5 s would starve before it arrives.
            let index = playlist.segments.len().saturating_sub(2);
            next_seq = Some(playlist.media_sequence + index as u64);
        }
        let wanted = next_seq.unwrap();
        for segment in &playlist.segments {
            if segment.seq < wanted {
                continue;
            }
            if !shared.alive() {
                return StopReason::Generation;
            }
            let url = crate::engine::hls::resolve(&segment.uri, &media_url);
            let bytes = match get_bytes(&http, &url) {
                Ok(bytes) => bytes,
                Err(err) => return StopReason::Error(format!("loading failed: {err}")),
            };
            let duration = segment.duration.max(0.1);
            let _ = events.send(ReaderEvent::Bitrate(
                (bytes.len() as f64 * 8.0 / duration) as u64,
            ));
            next_seq = Some(segment.seq + 1);
            let (samples, info) = match decode_block(&bytes) {
                Ok(decoded) => decoded,
                Err(err) => {
                    bad_segments += 1;
                    if bad_segments >= 3 {
                        return StopReason::Error(format!("HLS decode failed: {err}"));
                    }
                    continue; // Skip an isolated corrupt segment, including its sequence.
                }
            };
            bad_segments = 0;
            last_progress = Instant::now();
            if stream_rate.is_some_and(|rate| rate != info.rate) {
                return StopReason::Error("HLS sample rate changed".into());
            }
            if pipe.is_none() {
                stream_rate = Some(info.rate);
                send_telemetry(events, &info);
                pipe = Some(Pipe::new(producer));
            }
            let start = if joining {
                samples.len().saturating_sub(info.rate as usize * 3 * 2)
            } else {
                0
            };
            pipe.as_mut().unwrap().push(&samples[start..], shared);
            joining = false;
        }
    }
}

/// Decode a complete in-memory ADTS block (an HLS segment) to interleaved f32.
fn decode_block(bytes: &[u8]) -> Result<(Vec<f32>, TrackInfo)> {
    let bytes = crate::engine::hls::strip_id3(bytes);
    let mss = MediaSourceStream::new(
        Box::new(std::io::Cursor::new(bytes.to_vec())),
        Default::default(),
    );
    let mut format =
        symphonia::default::formats::AdtsReader::try_new(mss, FormatOptions::default())
            .map_err(|err| Error::new(format!("bad ADTS segment: {err}")))?;
    let Some(track) = format.default_track(TrackType::Audio) else {
        return Err(Error::new("no audio track in HLS segment"));
    };
    let Some(params) = track
        .codec_params
        .as_ref()
        .and_then(CodecParameters::audio)
        .cloned()
    else {
        return Err(Error::new("no audio track in HLS segment"));
    };
    let info = track_info(&params).map_err(Error::new)?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|err| Error::new(format!("unsupported codec: {err}")))?;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut out: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) | Err(SymphError::IoError(_)) => break, // End of the segment.
            Err(SymphError::DecodeError(_)) => continue,
            Err(_) => break,
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphError::DecodeError(_)) | Err(SymphError::IoError(_)) => continue,
            Err(err) => return Err(Error::new(format!("decoder error: {err}"))),
        };
        decoded.copy_to_vec_interleaved::<f32>(&mut interleaved);
        if info.channels == 1 {
            for sample in &interleaved {
                out.push(*sample);
                out.push(*sample);
            }
        } else {
            out.extend_from_slice(&interleaved);
        }
    }
    if out.is_empty() {
        return Err(Error::new("no decodable audio in HLS segment"));
    }
    Ok((out, info))
}

/// Backpressure belongs on the reader thread. A decoded block stays here
/// until all its samples reach the ring; pause or a generation change cancels it.
struct Pipe<'a> {
    producer: &'a mut Producer<f32>,
}

impl<'a> Pipe<'a> {
    fn new(producer: &'a mut Producer<f32>) -> Self {
        Self { producer }
    }

    fn push(&mut self, mut samples: &[f32], shared: &ReaderShared) {
        let epoch = shared.output.pause_epoch.load(Ordering::Acquire);
        while !samples.is_empty() {
            if !shared.alive()
                || shared.paused()
                || shared.output.pause_epoch.load(Ordering::Acquire) != epoch
            {
                return;
            }
            // Publish whole stereo frames so a callback can never consume a
            // left sample without its right partner.
            let available = self.producer.slots() / 2 * 2;
            let count = available.min(samples.len() / 2 * 2);
            if count == 0 {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            let _ = self.producer.push_entire_slice(&samples[..count]);
            samples = &samples[count..];
        }
    }
}

#[cfg(test)]
pub(crate) mod null_sink {
    use super::*;

    /// Drains the ring at real-time pace without any audio hardware.
    pub(crate) fn open(
        consumer: Consumer<f32>,
        output: Arc<OutputState>,
        rate: u32,
    ) -> Result<Box<dyn SinkHandle>> {
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        thread::spawn(move || {
            let mut callback = OutputCallback::new(consumer, output, rate, rate);
            let mut buffer = vec![0.0f32; rate as usize / 100 * 2];
            while !signal.load(Ordering::Relaxed) {
                callback.fill(&mut buffer);
                thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(Box::new(NullHandle { stop }))
    }

    struct NullHandle {
        stop: std::sync::Arc<AtomicBool>,
    }

    impl SinkHandle for NullHandle {
        fn failed(&self) -> bool {
            false
        }
    }

    impl Drop for NullHandle {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::api::StreamKind;

    fn stream(kind: StreamKind, url: &str) -> Stream {
        Stream {
            kind,
            url: url.to_owned(),
        }
    }

    fn engine() -> NativeEngine {
        NativeEngine::new(80, null_sink::open)
    }

    fn pump(engine: &mut NativeEngine, seconds: f64) {
        let deadline = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < deadline {
            engine.tick().unwrap();
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for(
        engine: &mut NativeEngine,
        seconds: f64,
        check: impl Fn(&NativeEngine) -> bool,
    ) -> bool {
        let deadline = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < deadline {
            engine.tick().unwrap();
            if check(engine) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn decodes_real_record_lc_fixture() {
        let bytes = std::fs::read(fixture_path("record96-lc.aacp")).unwrap();
        let (samples, info) = decode_block(&bytes).unwrap();
        assert_eq!(info.rate, 44_100);
        assert_eq!(info.channels, 2);
        assert_eq!(info.codec, Some("aac"));
        assert!(!samples.is_empty());
        assert_eq!(samples.len() % 2, 0, "interleaved stereo");
        let peak = samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!(
            peak > 0.05,
            "the fixture must contain real audio, peak {peak}"
        );
        // AAC decoders may overshoot full scale between samples; the output
        // sink clamps, so anything sane is fine here.
        assert!(peak < 2.0, "decoded peak is implausible: {peak}");
    }

    #[test]
    fn decodes_real_hls_segment_with_id3_header() {
        let bytes = std::fs::read(fixture_path("112/l0_6ac775da60ecdd612e0db32e.aac")).unwrap();
        assert!(
            bytes.starts_with(b"ID3"),
            "fixture must keep its ID3 header"
        );
        let (samples, info) = decode_block(&bytes).unwrap();
        assert_eq!(info.rate, 44_100);
        assert_eq!(info.channels, 2);
        assert!(samples.len() > 44_100, "a 6 s segment must decode fully");
    }

    #[test]
    fn he_aac_fixture_does_not_decode_as_full_rate_stereo() {
        // ADTS can carry an implicit SBR extension. Symphonia may decode its
        // low-rate core, so native selection must filter the HE stream kinds
        // and choose only the explicitly LC variant from HLS masters.
        let bytes = std::fs::read(fixture_path("record64-he.aacp")).unwrap();
        match decode_block(&bytes) {
            Err(_) => {} // The decoder may also reject the unsupported profile.
            Ok((samples, info)) => {
                // If it decodes, the result is a half-band core: it must not
                // look like a proper 44.1 kHz stereo stream.
                let looks_normal =
                    info.rate == 44_100 && info.channels == 2 && samples.len() > 44_100;
                assert!(
                    !looks_normal,
                    "an HE-AAC stream decoded as normal audio; the guard is missing"
                );
            }
        }
    }

    #[test]
    fn output_rate_preserves_device_clock_when_resampling_is_supported() {
        assert_eq!(
            device_rate(44_100, 96_000, 44_100, Some(48_000)),
            Some(48_000)
        );
        assert_eq!(
            device_rate(44_100, 96_000, 48_000, Some(48_000)),
            Some(48_000)
        );
        // A slower or out-of-range clock cannot be used by our upsampler.
        assert_eq!(
            device_rate(44_100, 96_000, 48_000, Some(44_100)),
            Some(48_000)
        );
        assert_eq!(
            device_rate(44_100, 48_000, 44_100, Some(96_000)),
            Some(44_100)
        );
        assert_eq!(device_rate(44_100, 48_000, 96_000, Some(48_000)), None);
    }

    #[test]
    fn resample_passes_frames_through_at_matching_rates() {
        let (mut producer, mut consumer) = RingBuffer::new(64);
        // Matching rates need no look-ahead: every input frame must emerge.
        for frame in [[0.1, 0.2], [0.3, 0.4], [0.5, 0.6], [0.7, 0.8]] {
            let _ = producer.push_entire_slice(&frame);
        }
        let mut resample = Resample::new(44_100, 44_100);
        for expected in [[0.1, 0.2], [0.3, 0.4], [0.5, 0.6], [0.7, 0.8]] {
            let frame = resample.frame(&mut consumer).expect("frames must flow");
            assert!(
                (frame[0] - expected[0]).abs() < 1e-6 && (frame[1] - expected[1]).abs() < 1e-6,
                "got {frame:?}, want {expected:?}"
            );
        }
        assert!(resample.frame(&mut consumer).is_none(), "ring is drained");
    }

    #[test]
    fn resample_interpolates_between_input_frames() {
        let (mut producer, mut consumer) = RingBuffer::new(64);
        for frame in [[0.0, 0.0], [2.0, 2.0]] {
            let _ = producer.push_entire_slice(&frame);
        }
        // 24 kHz in, 48 kHz out: two output frames per input frame.
        let mut resample = Resample::new(24_000, 48_000);
        let first = resample.frame(&mut consumer).unwrap();
        let second = resample.frame(&mut consumer).unwrap();
        assert_eq!(first, [0.0, 0.0], "t = 0 reproduces the older frame");
        assert_eq!(second, [1.0, 1.0], "t = 0.5 is the midpoint");
        // The next step needs a third input frame that is not there yet.
        assert!(
            resample.frame(&mut consumer).is_none(),
            "underrun is honest"
        );
        // Data arrives; interpolation resumes from where it stopped.
        let _ = producer.push_entire_slice(&[4.0, 4.0]);
        let third = resample.frame(&mut consumer).expect("must resume");
        assert_eq!(third, [2.0, 2.0]);
    }

    #[test]
    fn device_rate_prefers_the_stream_and_refuses_slower_devices() {
        assert_eq!(device_rate(44_100, 48_000, 44_100, None), Some(44_100));
        assert_eq!(device_rate(48_000, 48_000, 44_100, None), Some(48_000));
        assert_eq!(device_rate(48_000, 96_000, 44_100, None), Some(48_000));
        assert_eq!(device_rate(24_000, 24_000, 44_100, None), None);
    }

    fn reader_shared(output: Arc<OutputState>) -> ReaderShared {
        ReaderShared {
            generation: Arc::new(AtomicU64::new(1)),
            my_generation: 1,
            output,
        }
    }

    #[test]
    fn segment_larger_than_ring_reaches_output_in_order_without_loss() {
        let output = Arc::new(OutputState::new(100));
        let shared = reader_shared(output.clone());
        let (mut producer, consumer) = RingBuffer::new(RING_CAPACITY);
        // Initial tail followed by a full HLS segment: larger than both the
        // ring and the old pending limit. Every stereo frame has a unique value.
        let samples: Vec<f32> = (0..44_100 * 9)
            .flat_map(|index| [0.1 + index as f32 / 1_000_000.0; 2])
            .collect();
        let expected = samples.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let writer = thread::spawn(move || {
            let mut pipe = Pipe::new(&mut producer);
            pipe.push(&samples[..44_100 * 3 * 2], &shared);
            pipe.push(&samples[44_100 * 3 * 2..], &shared);
            done_tx.send(()).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while consumer.slots() < start_fill(44_100) {
            assert!(Instant::now() < deadline, "reader did not fill the ring");
            thread::yield_now();
        }
        assert!(
            done_rx.try_recv().is_err(),
            "writer must wait for the consumer"
        );
        let mut callback = OutputCallback::new(consumer, output, 44_100, 44_100);
        let mut received = Vec::new();
        let mut writer_done = false;
        let mut buffer = [0.0f32; 882];
        while received.len() < expected.len() {
            assert!(
                Instant::now() < deadline,
                "samples lost: {} of {}",
                received.len(),
                expected.len()
            );
            writer_done |= done_rx.try_recv().is_ok();
            if callback.consumer.slots() >= buffer.len() || writer_done {
                callback.fill(&mut buffer);
                received.extend(buffer.iter().copied().filter(|sample| *sample != 0.0));
            }
            thread::yield_now();
        }
        writer.join().unwrap();
        assert_eq!(received, expected);
    }

    #[test]
    fn pause_mutes_output_discards_backlog_and_clears_resampler() {
        for (rate, device_rate) in [(100, 100), (100, 200)] {
            let mut engine = engine();
            engine.snapshot.state = PlayerState::Playing;
            let (mut producer, consumer) = RingBuffer::new(256);
            producer.push_entire_slice(&[0.5; 128]).unwrap();
            let mut callback =
                OutputCallback::new(consumer, engine.output.clone(), rate, device_rate);
            let mut buffer = [0.0f32; 10];
            callback.fill(&mut buffer);
            assert!(buffer.iter().all(|sample| *sample > 0.0));
            engine.pause();
            callback.fill(&mut buffer);
            assert_eq!(buffer, [0.0; 10]);
            assert_eq!(callback.consumer.slots(), 0);
            assert_eq!(engine.output.underruns.load(Ordering::Relaxed), 0);
            engine.pause();
            callback.fill(&mut buffer);
            assert_eq!(buffer, [0.0; 10], "resume must not replay queued audio");
            producer.push_entire_slice(&[0.25; 128]).unwrap();
            callback.fill(&mut buffer);
            assert_eq!(buffer, [0.2; 10], "only new audio with 80% gain may emerge");
        }
    }

    #[test]
    fn pause_and_resume_between_callbacks_still_discard_old_audio() {
        let mut engine = engine();
        engine.snapshot.state = PlayerState::Playing;
        let (mut producer, consumer) = RingBuffer::new(128);
        producer.push_entire_slice(&[0.5; 100]).unwrap();
        let mut callback = OutputCallback::new(consumer, engine.output.clone(), 100, 100);
        let mut buffer = [0.0f32; 10];
        callback.fill(&mut buffer);
        engine.pause();
        engine.pause();
        callback.fill(&mut buffer);
        assert_eq!(buffer, [0.0; 10]);
        assert_eq!(callback.consumer.slots(), 0);
    }

    #[test]
    fn empty_output_updates_cache_state_and_meter_without_reader_events() {
        let mut engine = engine();
        engine.snapshot.samplerate = Some(100);
        let (mut producer, consumer) = RingBuffer::new(256);
        producer.push_entire_slice(&[0.5; 100]).unwrap();
        engine.sink = Some(Box::new(TestSink));
        let mut callback = OutputCallback::new(consumer, engine.output.clone(), 100, 100);
        let mut buffer = [0.0f32; 20];
        callback.fill(&mut buffer);
        engine.tick().unwrap();
        assert_eq!(engine.snapshot.state, PlayerState::Playing);
        assert_eq!(engine.snapshot.cache_seconds, Some(0.4));
        assert!(engine.snapshot.levels[0] > 0.0);
        for _ in 0..5 {
            callback.fill(&mut buffer);
        }
        engine.tick().unwrap();
        assert_eq!(engine.snapshot.state, PlayerState::Buffering);
        assert_eq!(engine.snapshot.cache_seconds, Some(0.0));
        assert_eq!(engine.snapshot.buffering, Some(0));
        assert_eq!(engine.snapshot.levels, [0.0; 2]);
        assert_eq!(engine.snapshot.underruns, 1);
    }

    #[test]
    fn output_meter_measures_pre_gain_audio_and_prebuffering_is_silent() {
        let output = Arc::new(OutputState::new(0));
        let (mut producer, consumer) = RingBuffer::new(256);
        let mut callback = OutputCallback::new(consumer, output.clone(), 100, 100);
        let mut buffer = [1.0f32; 10];
        producer.push_entire_slice(&[0.5; 20]).unwrap();
        callback.fill(&mut buffer);
        assert_eq!(buffer, [0.0; 10]);
        assert_eq!(
            callback.consumer.slots(),
            20,
            "prebuffering must not consume audio"
        );
        assert_eq!(output.meter_sequence.load(Ordering::Acquire), 0);
        producer.push_entire_slice(&[0.5; 80]).unwrap();
        callback.fill(&mut buffer);
        assert_eq!(buffer, [0.0; 10], "volume zero must mute output");
        let db = f32::from_bits(output.left_db.load(Ordering::Relaxed));
        assert!(
            (db + 6.0206).abs() < 0.001,
            "meter must still measure the signal"
        );
        output.volume.store(50, Ordering::Relaxed);
        callback.fill(&mut buffer);
        assert_eq!(buffer, [0.25; 10]);
        assert_eq!(f32::from_bits(output.left_db.load(Ordering::Relaxed)), db);
    }

    struct TestSink;
    impl SinkHandle for TestSink {
        fn failed(&self) -> bool {
            false
        }
    }

    struct ErrorSink(Arc<AtomicBool>);
    impl SinkHandle for ErrorSink {
        fn failed(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn output_notifications_keep_native_playback_alive() {
        let mut engine = engine();
        let failed = Arc::new(AtomicBool::new(false));
        engine.sink = Some(Box::new(ErrorSink(failed.clone())));
        engine.output.buffering.store(false, Ordering::Release);
        let generation = engine.generation.load(Ordering::Relaxed);
        for kind in [
            cpal::ErrorKind::Xrun,
            cpal::ErrorKind::DeviceChanged,
            cpal::ErrorKind::RealtimeDenied,
            cpal::ErrorKind::Xrun,
        ] {
            handle_output_error(kind, &failed);
            engine.tick().unwrap();
            assert_eq!(engine.snapshot.state, PlayerState::Playing);
            assert!(engine.snapshot.error.is_none());
            assert!(engine.sink.is_some());
            assert_eq!(engine.generation.load(Ordering::Relaxed), generation);
        }
    }

    #[test]
    fn fatal_output_errors_remain_latched_and_stop_native_playback() {
        for kind in [
            cpal::ErrorKind::DeviceNotAvailable,
            cpal::ErrorKind::StreamInvalidated,
            cpal::ErrorKind::BackendError,
            cpal::ErrorKind::Other,
        ] {
            let mut engine = engine();
            let failed = Arc::new(AtomicBool::new(false));
            engine.sink = Some(Box::new(ErrorSink(failed.clone())));
            let generation = engine.generation.load(Ordering::Relaxed);
            handle_output_error(kind, &failed);
            // A later recoverable notification must not hide a fatal error.
            handle_output_error(cpal::ErrorKind::Xrun, &failed);
            engine.tick().unwrap();
            assert_eq!(engine.snapshot.state, PlayerState::Error);
            assert_eq!(
                engine.snapshot.error.as_deref(),
                Some("audio device failed · Enter to retry")
            );
            assert!(engine.sink.is_none());
            assert_eq!(engine.generation.load(Ordering::Relaxed), generation + 1);
        }
    }

    #[test]
    fn blocked_reader_exits_on_pause_or_station_switch() {
        for pause in [true, false] {
            let output = Arc::new(OutputState::new(100));
            let shared = reader_shared(output.clone());
            let generation = shared.generation.clone();
            let (mut producer, consumer) = RingBuffer::new(16);
            let (tx, rx) = mpsc::channel();
            let writer = thread::spawn(move || {
                Pipe::new(&mut producer).push(&[0.5; 1024], &shared);
                tx.send(()).unwrap();
            });
            let deadline = Instant::now() + Duration::from_secs(1);
            while consumer.slots() < 16 {
                assert!(Instant::now() < deadline);
                thread::yield_now();
            }
            if pause {
                output.paused.store(true, Ordering::Release);
            } else {
                generation.fetch_add(1, Ordering::Relaxed);
            }
            rx.recv_timeout(Duration::from_secs(1))
                .expect("blocked reader must retire");
            writer.join().unwrap();
        }
    }

    #[test]
    fn native_engine_plays_a_direct_stream_end_to_end() {
        let mut engine = engine();
        engine.play(vec![stream(StreamKind::Main, "fixture://record96-lc.aacp")]);
        assert!(wait_for(&mut engine, 4.0, |engine| {
            engine.snapshot.state == PlayerState::Playing
                && engine.snapshot.levels[0] > 0.0
                && engine.snapshot.bitrate.is_some()
        }));
        let snapshot = engine.snapshot();
        assert_eq!(snapshot.codec.as_deref(), Some("aac"));
        assert_eq!(snapshot.samplerate, Some(44_100));
        assert_eq!(snapshot.channels, Some(2));
        assert!(snapshot.bitrate.is_some(), "bitrate must be measured");
        assert_eq!(snapshot.stream_round, 1);
        assert!(snapshot.levels[0] > 0.0, "the meter must follow real audio");
        assert!(snapshot.levels[1] > 0.0);
        assert!(snapshot.cache_seconds.is_some());
        pump(&mut engine, 0.2);
        // Pause is radio: the state flips and the meter settles.
        engine.pause();
        pump(&mut engine, 0.3);
        assert_eq!(engine.snapshot.state, PlayerState::Paused);
        assert_eq!(engine.snapshot.levels, [0.0, 0.0]);
        engine.pause();
        assert!(
            wait_for(&mut engine, 4.0, |engine| {
                engine.snapshot.state == PlayerState::Playing
            }),
            "resume must return to the live edge"
        );
    }

    #[test]
    fn native_engine_falls_back_to_hls_and_reports_the_round() {
        // The direct stream is a nonexistent fixture: the reader must fall
        // through to the HLS playlist, which plays.
        let mut engine = engine();
        engine.play(vec![
            stream(StreamKind::Main, "fixture://missing.aacp"),
            stream(StreamKind::Hls, "fixture://fixtures/hls-master.m3u8"),
        ]);
        assert!(
            wait_for(&mut engine, 10.0, |engine| {
                engine.snapshot.state == PlayerState::Playing
            }),
            "must fall through to HLS and play; state: {:?}, error: {:?}, stream: {:?}",
            engine.snapshot.state,
            engine.snapshot.error,
            engine.snapshot.stream
        );
        let snapshot = engine.snapshot();
        // The snapshot reports the station's HLS URL (the master playlist),
        // which ui::stream_label matches against Station::stream_hls.
        assert!(
            snapshot
                .stream
                .as_deref()
                .unwrap()
                .contains("hls-master.m3u8")
        );
        assert_eq!(snapshot.stream_round, 1, "still the first fallback round");
        assert_eq!(snapshot.codec.as_deref(), Some("aac"));
    }

    #[test]
    fn he_only_lists_are_refused_up_front() {
        let mut engine = engine();
        engine.play(vec![
            stream(StreamKind::High, "https://example.com/64.aacp"),
            stream(StreamKind::Low, "https://example.com/32.aacp"),
        ]);
        engine.tick().unwrap();
        let snapshot = engine.snapshot();
        assert_eq!(snapshot.state, PlayerState::Error);
        assert!(snapshot.error.unwrap().contains("needs mpv"));
    }

    #[test]
    fn generation_switch_retires_the_previous_reader() {
        let mut engine = engine();
        engine.play(vec![stream(StreamKind::Main, "fixture://record96-lc.aacp")]);
        pump(&mut engine, 0.3);
        let first = engine.generation.load(Ordering::Relaxed);
        engine.play(vec![stream(StreamKind::Main, "fixture://record96-lc.aacp")]);
        assert!(first < engine.generation.load(Ordering::Relaxed));
        assert!(wait_for(&mut engine, 4.0, |engine| {
            engine.snapshot.state == PlayerState::Playing
        }));
    }

    #[test]
    #[ignore = "requires network access to Radio Record streams"]
    fn live_direct_stream_plays_end_to_end() {
        let mut engine = engine();
        engine.play(vec![
            stream(
                StreamKind::Main,
                "https://radiorecord.hostingradio.ru/rr_main96.aacp",
            ),
            stream(
                StreamKind::Hls,
                "https://hls-01-radiorecord.hostingradio.ru/record/playlist.m3u8",
            ),
        ]);
        assert!(
            wait_for(&mut engine, 20.0, |engine| {
                engine.snapshot.state == PlayerState::Playing
                    && engine.snapshot.levels[0] > 0.0
                    && engine.snapshot.bitrate.is_some()
            }),
            "the live stream must reach playing; state: {:?}, error: {:?}",
            engine.snapshot.state,
            engine.snapshot.error
        );
        let snapshot = engine.snapshot();
        assert_eq!(snapshot.codec.as_deref(), Some("aac"));
        assert_eq!(snapshot.samplerate, Some(44_100));
        assert_eq!(snapshot.channels, Some(2));
        assert!(snapshot.bitrate.is_some(), "bitrate must be measured");
        assert!(snapshot.levels[0] > 0.0, "the meter must follow real audio");
        assert!(snapshot.cache_seconds.is_some());
        // Pause is radio: the meter settles and resume returns to the edge.
        engine.pause();
        pump(&mut engine, 0.4);
        assert_eq!(engine.snapshot.state, PlayerState::Paused);
        assert_eq!(engine.snapshot.levels, [0.0, 0.0]);
        engine.pause();
        assert!(
            wait_for(&mut engine, 10.0, |engine| {
                engine.snapshot.state == PlayerState::Playing
            }),
            "resume must return to the live edge"
        );
    }

    #[test]
    #[ignore = "requires network access to Radio Record streams"]
    fn live_hls_stream_plays_end_to_end() {
        let mut engine = engine();
        engine.play(vec![stream(
            StreamKind::Hls,
            "https://hls-01-radiorecord.hostingradio.ru/record/playlist.m3u8",
        )]);
        assert!(
            wait_for(&mut engine, 30.0, |engine| {
                engine.snapshot.state == PlayerState::Playing && engine.snapshot.levels[0] > 0.0
            }),
            "the live HLS stream must reach playing; state: {:?}, error: {:?}",
            engine.snapshot.state,
            engine.snapshot.error
        );
        let snapshot = engine.snapshot();
        assert_eq!(snapshot.codec.as_deref(), Some("aac"));
        assert_eq!(snapshot.samplerate, Some(44_100));
        assert!(snapshot.levels[0] > 0.0);
        let underruns = snapshot.underruns;
        // Observe multiple six-second publications, not just initial startup.
        pump(&mut engine, 18.0);
        assert_eq!(
            engine.snapshot.state,
            PlayerState::Playing,
            "{:?}",
            engine.snapshot.error
        );
        assert_eq!(
            engine.snapshot.underruns, underruns,
            "HLS must remain continuous"
        );
    }

    #[test]
    #[ignore = "plays a short burst on the real audio device"]
    fn live_device_plays_the_stream() {
        if !device_available() {
            eprintln!("no audio device; skipping");
            return;
        }
        let mut engine = NativeEngine::new(70, cpal_sink);
        engine.play(vec![stream(
            StreamKind::Main,
            "https://radiorecord.hostingradio.ru/rr_main96.aacp",
        )]);
        assert!(wait_for(&mut engine, 20.0, |engine| {
            engine.snapshot.state == PlayerState::Playing
        }));
        pump(&mut engine, 3.0);
        assert_eq!(engine.snapshot.state, PlayerState::Playing);
    }
}
