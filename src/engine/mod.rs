//! Built-in audio engine driven by the worker thread in `player.rs`.

#[cfg(target_os = "macos")]
mod coreaudio;
pub(crate) mod hls;
pub(crate) mod native;

use crate::api::{Stream, StreamKind};
use crate::error::Result;
use crate::player::Snapshot;

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
}

/// Open the built-in AAC-LC engine. Stream capability is checked by play().
pub(crate) fn open(volume: u8, _streams: &[Stream]) -> Result<Box<dyn EngineImpl>> {
    Ok(Box::new(native::NativeEngine::new(
        volume,
        native::cpal_sink,
    )))
}

/// Direct AAC-LC and the AAC-LC variant of an HLS playlist are supported.
pub(crate) fn native_plays(kind: StreamKind) -> bool {
    matches!(kind, StreamKind::Main | StreamKind::Hls)
}
