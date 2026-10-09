//! Audio engines. The worker thread in `player.rs` drives one `EngineImpl`
//! at a time; `open` picks which one, honouring `RADIOME_ENGINE`.

pub(crate) mod hls;
pub(crate) mod mpv;
pub(crate) mod native;

use crate::api::{Stream, StreamKind};
use crate::error::Result;
use crate::player::{PlayerState, Snapshot};

/// Everything the worker needs from an audio engine. Implementations must be
/// driven only from the worker thread and must publish state through
/// `Snapshot` once per `tick`.
pub(crate) trait EngineImpl {
    /// Begin loading `streams` (already in fallback priority order); a
    /// previous load is replaced. Engines filter out kinds they cannot play.
    fn play(&mut self, streams: Vec<Stream>);
    /// Toggle pause.
    fn pause(&mut self);
    fn set_volume(&mut self, volume: u8);
    /// Advance the engine: drain events, run per-tick work, refresh the
    /// snapshot. Returning `Err` is fatal for this engine instance; the
    /// worker reports it and drops the engine.
    fn tick(&mut self) -> Result<()>;
    fn snapshot(&self) -> Snapshot;
    /// Drain pending media-key commands (prev/next), if any.
    fn take_media(&mut self) -> Vec<isize>;
}

/// Which engine to open. `Auto` prefers the native engine when it can play
/// anything from the list and a default output device exists, falling back
/// to mpv (if installed) otherwise.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EngineKind {
    Auto,
    Native,
    Mpv,
}

pub(crate) fn open(volume: u8, streams: &[Stream]) -> Result<Box<dyn EngineImpl>> {
    let requested = std::env::var("RADIOME_ENGINE").unwrap_or_default();
    let kind = match requested.trim().to_ascii_lowercase().as_str() {
        "mpv" => EngineKind::Mpv,
        "native" => EngineKind::Native,
        _ => EngineKind::Auto,
    };
    let native_first = prefer_native(kind, streams, native::device_available);
    if native_first {
        let active: Box<dyn EngineImpl> =
            Box::new(native::NativeEngine::new(volume, native::cpal_sink));
        return if kind == EngineKind::Auto {
            Ok(Box::new(FallbackEngine::new(active, volume, open_mpv)))
        } else {
            Ok(active)
        };
    }
    open_mpv(volume)
}

fn prefer_native(
    kind: EngineKind,
    streams: &[Stream],
    device_available: impl FnOnce() -> bool,
) -> bool {
    match kind {
        EngineKind::Native => true,
        EngineKind::Mpv => false,
        EngineKind::Auto => {
            streams.iter().any(|stream| native_plays(stream.kind)) && device_available()
        }
    }
}

fn open_mpv(volume: u8) -> Result<Box<dyn EngineImpl>> {
    Ok(Box::new(mpv::MpvEngine::new(volume)?))
}

type EngineFactory = Box<dyn FnMut(u8) -> Result<Box<dyn EngineImpl>>>;

/// Auto mode keeps the full stream list for a single mpv attempt after native
/// exhausts its URLs or fails to open/use the audio device. Explicit engine
/// overrides bypass this wrapper.
struct FallbackEngine {
    active: Option<Box<dyn EngineImpl>>,
    fallback: Option<EngineFactory>,
    streams: Vec<Stream>,
    volume: u8,
    paused: bool,
    snapshot: Snapshot,
}

impl FallbackEngine {
    fn new(
        active: Box<dyn EngineImpl>,
        volume: u8,
        fallback: impl FnMut(u8) -> Result<Box<dyn EngineImpl>> + 'static,
    ) -> Self {
        Self {
            active: Some(active),
            fallback: Some(Box::new(fallback)),
            streams: Vec::new(),
            volume,
            paused: false,
            snapshot: Snapshot::default(),
        }
    }
}

impl EngineImpl for FallbackEngine {
    fn play(&mut self, streams: Vec<Stream>) {
        self.streams = streams.clone();
        self.paused = false;
        if let Some(active) = &mut self.active {
            active.play(streams);
        }
    }

    fn pause(&mut self) {
        self.paused = !self.paused;
        if let Some(active) = &mut self.active {
            active.pause();
        }
    }

    fn set_volume(&mut self, volume: u8) {
        self.volume = volume;
        if let Some(active) = &mut self.active {
            active.set_volume(volume);
        }
    }

    fn tick(&mut self) -> Result<()> {
        let Some(active) = &mut self.active else {
            return Ok(());
        };
        self.snapshot = match active.tick() {
            Ok(()) => active.snapshot(),
            Err(err) => Snapshot {
                state: PlayerState::Error,
                error: Some(err.to_string()),
                ..Snapshot::default()
            },
        };
        if self.snapshot.state == PlayerState::Error {
            // Drop native before opening mpv so it cannot keep producing sound
            // or competing for the device while fallback starts.
            self.active = None;
            if let Some(mut open) = self.fallback.take() {
                match open(self.volume) {
                    Ok(mut next) => {
                        next.play(self.streams.clone());
                        if self.paused {
                            next.pause();
                        }
                        self.snapshot = next.snapshot();
                        self.active = Some(next);
                    }
                    Err(err) => {
                        let reason = self
                            .snapshot
                            .error
                            .take()
                            .unwrap_or_else(|| "native engine failed".into());
                        self.snapshot.error =
                            Some(format!("{reason} · mpv fallback unavailable: {err}"));
                    }
                }
            }
        }
        Ok(())
    }

    fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }

    fn take_media(&mut self) -> Vec<isize> {
        self.active
            .as_mut()
            .map_or_else(Vec::new, |active| active.take_media())
    }
}

/// Whether the native engine can decode this stream kind: the direct
/// AAC-LC stream and the AAC-LC variant of the HLS playlist.
pub(crate) fn native_plays(kind: StreamKind) -> bool {
    matches!(kind, StreamKind::Main | StreamKind::Hls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use std::sync::mpsc::{self, Sender};

    fn streams(kind: StreamKind) -> Vec<Stream> {
        vec![Stream {
            kind,
            url: "https://example.com/audio".into(),
        }]
    }

    struct FakeEngine {
        events: Sender<String>,
        snapshot: Snapshot,
        fatal: bool,
    }

    impl EngineImpl for FakeEngine {
        fn play(&mut self, streams: Vec<Stream>) {
            self.events.send(format!("play:{}", streams.len())).unwrap();
        }
        fn pause(&mut self) {
            self.events.send("pause".into()).unwrap();
        }
        fn set_volume(&mut self, volume: u8) {
            self.events.send(format!("volume:{volume}")).unwrap();
        }
        fn tick(&mut self) -> Result<()> {
            if self.fatal {
                Err(Error::new("native device failed"))
            } else {
                Ok(())
            }
        }
        fn snapshot(&self) -> Snapshot {
            self.snapshot.clone()
        }
        fn take_media(&mut self) -> Vec<isize> {
            vec![1]
        }
    }

    impl Drop for FakeEngine {
        fn drop(&mut self) {
            let _ = self.events.send("drop".into());
        }
    }

    #[test]
    fn engine_selection_depends_on_current_station_and_explicit_override() {
        let lc = streams(StreamKind::Main);
        let he = streams(StreamKind::High);
        assert!(prefer_native(EngineKind::Auto, &lc, || true));
        assert!(!prefer_native(EngineKind::Auto, &he, || true));
        assert!(!prefer_native(EngineKind::Auto, &lc, || false));
        assert!(prefer_native(EngineKind::Native, &he, || false));
        assert!(!prefer_native(EngineKind::Mpv, &lc, || true));
    }

    #[test]
    fn automatic_fallback_handles_snapshot_and_fatal_errors_with_current_controls() {
        for fatal in [false, true] {
            let (tx, rx) = mpsc::channel();
            let native = FakeEngine {
                events: tx.clone(),
                snapshot: Snapshot {
                    state: PlayerState::Error,
                    error: Some("streams exhausted".into()),
                    ..Snapshot::default()
                },
                fatal,
            };
            let mut engine = FallbackEngine::new(Box::new(native), 80, move |volume| {
                tx.send(format!("open:{volume}")).unwrap();
                Ok(Box::new(FakeEngine {
                    events: tx.clone(),
                    snapshot: Snapshot {
                        state: PlayerState::Playing,
                        ..Snapshot::default()
                    },
                    fatal: false,
                }))
            });
            let mut list = streams(StreamKind::Main);
            list.extend(streams(StreamKind::High));
            engine.play(list);
            engine.set_volume(35);
            engine.pause();
            engine.tick().unwrap();
            assert_eq!(engine.snapshot().state, PlayerState::Playing);
            assert_eq!(
                rx.try_iter().collect::<Vec<_>>(),
                [
                    "play:2",
                    "volume:35",
                    "pause",
                    "drop",
                    "open:35",
                    "play:2",
                    "pause"
                ]
            );
            assert_eq!(engine.take_media(), [1]);
            engine.tick().unwrap();
            assert!(rx.try_recv().is_err(), "fallback must happen only once");
        }
    }

    #[test]
    fn missing_mpv_preserves_native_error_and_does_not_retry_each_tick() {
        let (tx, rx) = mpsc::channel();
        let active = FakeEngine {
            events: tx.clone(),
            snapshot: Snapshot {
                state: PlayerState::Error,
                error: Some("LC streams unreachable".into()),
                ..Snapshot::default()
            },
            fatal: false,
        };
        let mut engine = FallbackEngine::new(Box::new(active), 80, move |_| {
            tx.send("open".into()).unwrap();
            Err(Error::new("mpv not installed"))
        });
        engine.play(streams(StreamKind::Main));
        for _ in 0..3 {
            engine.tick().unwrap();
        }
        let snapshot = engine.snapshot();
        assert_eq!(snapshot.state, PlayerState::Error);
        let error = snapshot.error.unwrap();
        assert!(error.contains("LC streams unreachable"));
        assert!(error.contains("mpv not installed"));
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            ["play:1", "drop", "open"]
        );
    }
}
