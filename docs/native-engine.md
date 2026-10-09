# The native audio engine

Implementation notes for native AAC-LC playback, HTTP transport, and the
optional mpv fallback.

## Goal

One binary, zero installable system packages: `make run` and the radio plays.
Cargo dependencies compiled into the binary are fine; the only accepted
runtime library is `libasound.so.2` on Linux (present on virtually every
desktop distribution; no static ALSA link exists upstream, alsa-sys issue
#10).

## What the investigation found (empirically verified)

1. **Radio Record serves no MP3 at all.** All 117 stations, all bitrates, are
   AAC in ADTS: `stream_320` is AAC-LC ~96 kbps (the field name is historic),
   `stream_128` is HE-AAC (SBR), `stream_64` is HE-AACv2. HLS variants:
   112 kbps AAC-LC, 64/32 kbps HE-AAC.
2. **HLS segments are not MPEG-TS** — they are plain ADTS files with an ID3v2
   header. No TS demuxer needed; the same Symphonia ADTS reader covers direct
   streams and HLS segments.
3. **The best-quality stream (HLS 112k AAC-LC) decodes with the simple LC
   decoder** — the native engine covers both real degradation paths
   (`320` direct and HLS) without touching HE-AAC.
4. **Host topology**: the three direct streams share one host, HLS lives on
   another. The fallback chain degenerates to `320 → HLS` in practice, so
   skipping HE-AAC streams costs almost nothing in resilience.
5. **HE-AAC in pure Rust is not production-ready** (symphonia: not
   implemented; alternatives are weeks old). The engine therefore *refuses*
   HE-only stream lists up front rather than playing half-band audio —
   "must refuse, not half-play" is a red line.
6. mpv applies volume in the audio output, *after* its filter chain, so the
   astats meter measures pre-gain signal. The native engine reproduces this:
   RMS is computed on samples consumed by the output callback, before gain.

## Architecture

```
worker (supervisor, 50 ms)  ← mpsc ReaderEvent — reader thread (HTTP + decode)
        ↑ atomics                                  ↓ interleaved f32
        |                                       rtrb ring (288,000 samples)
audio callback (cpal) ← pops samples, applies volume, counts underruns
```

- **No locks anywhere.** Outside the audio path: mpsc only. Inside it:
  a wait-free SPSC ring (rtrb) and atomics. Data flows one way:
  callback → atomics → supervisor → mpsc → main thread (unchanged).
- **The real-time callback** never blocks, allocates, locks,
  logs, or panics. It pops from the ring, multiplies by an atomic volume,
  clamps, and writes to the device buffer. It also meters the consumed signal,
  publishes current fill, and buffers 0.3 s at startup or after starvation.
- **Generation token**: direct reads have a 2 s idle timeout and connections
  have a 5 s timeout; HLS requests have a 10 s global timeout. The reader
  checks generation and pause while waiting for ring space. Old sinks stop
  immediately on replacement; readers retire after their current operation.
- **Native pause**: the next callback silences output and clears queued samples
  and interpolation state. An epoch also catches pause/resume between callbacks.
  The reader drains paused audio; HLS resume rejoins recent published segments.
  mpv retains its own pause/cache behavior.
- **Bounded audio**: the ring has 288,000 samples (3 s at 48 kHz stereo).
  The reader holds one decoded HLS segment and feeds it fully with backpressure.
  Playlist bodies are limited to 256 KiB and segment bodies to 4 MiB. Events
  carry stream metadata; per-callback telemetry uses atomics. Each fallback
  creates a fresh ring and sink so samples and rates cannot leak across URLs.

## Components

| Concern | Choice | Why |
| --- | --- | --- |
| HTTP (API, HLS playlists) | `ureq` 3 (rustls + ring + gzip + webpki-roots, all default) | no system TLS/CA store, synchronous |
| HTTP (live ADTS body) | `ureq` 2 (`timeout_read`) | ureq 3 has no per-read idle timeout; an endless stream needs one to detect a stalled server |
| AAC-LC decode + ADTS | `symphonia` 0.6 (`aac` feature, MPL-2.0) | status "Great", lineage from ffmpeg's AAC author |
| Audio output | `cpal` 0.18 | CoreAudio (system framework, zero install) on macOS; ALSA on Linux; handles device hot-plug, suspend, and default-device switches itself in 0.18 |
| Ring | `rtrb` 0.4 | documented wait-free SPSC, no allocation after construction |
| HLS client | hand-written `engine/hls.rs` | unencrypted ADTS playlists; `url` resolves relative references; metadata is ignored |

## Engine selection

`RADIOME_ENGINE=auto|native|mpv` (default `auto`): selection runs for each
station. Native is preferred for an LC-playable stream with an output device;
otherwise mpv is opened. In auto mode, native stream exhaustion or device
failure drops native and tries mpv once with the full stream list, current
volume, and pause. Missing mpv preserves the native error. Explicit native
or mpv overrides disable automatic engine switching. Native filters the
known HE stream kinds and picks only the LC variant from an HLS master.

## Verification

- Unit tests run against recorded fixtures of the real streams
  (`tests/fixtures/`), including an HE-AAC sample demonstrating the unsupported low-rate core and
  real HLS playlists/segments. Output tests use the production callback and
  verify complete segment transfer, pause silence, buffer reset, starvation,
  and engine switching. A throttled reader paces direct fixtures like network.
- `#[ignore]` live tests (network + device): direct stream, HLS stream, and
  a short burst on the real output device — the native equivalent of
  `make test-audio`, runnable without mpv. The HLS test observes 18 s after
  startup to cover multiple publications without underruns.
- Format limitation: native selection trusts Record's `stream_320` as LC
  and HLS `CODECS` declarations. Symphonia can decode only the low-rate core
  of implicit HE-AAC; the fixture test demonstrates this, not codec rejection.
  If upstream relabels HE-AAC as LC, use `RADIOME_ENGINE=mpv`.

## Roadmap beyond this implementation

1. **HLS `X-DATERANGE` metadata** (X-ARTIST/X-TITLE) as a faster track source
   than the 15 s API poll — additive, after the engine soaks.
2. **Linux system media controls**: add MPRIS integration. macOS already
   uses an application-owned MPRemoteCommandCenter session for both engines,
   with Cocoa events pumped on the main thread alongside the terminal loop.
   Terminal F7/F8/F9 remain available on both platforms.
3. **libfdk-aac** (vendored C via `symphonia-adapter-fdk-aac`) behind a
   feature flag to cover the HE-AAC tail (dead 96k mount + dead HLS host
   simultaneously) and eventually drop mpv entirely. Gated on the radiome
   license being compatible with FDK's Software IP Royalty-Free License.
4. ~~Resampling only if a device cannot be opened at the stream's rate~~ —
   shipped 2026-10-09: linear interpolation in the output callback (upsampling
   only; slower devices are refused). Stereo output devices that
   expose only 48 kHz can play 44.1 kHz streams. A `rubato` upgrade can
   later replace the interpolator if its imaging is ever measured to matter.

## Sources

- Stream formats measured with ffprobe on live samples, 2026-10-08.
- Symphonia codec status: https://github.com/pdeljanov/Symphonia
- cpal 0.18 CoreAudio host (hot-plug, default-device monitor):
  https://github.com/RustAudio/cpal
- mpv volume vs filters: mpv `DOCS/man/options.rst` (`--volume`),
  `audio/out/ao.c` (`ao_set_gain`)
- ureq timeouts: https://docs.rs/ureq (v3 `Timeouts`, v2 `timeout_read`)
- rtrb: https://docs.rs/rtrb
- alsa-sys static link status: https://github.com/diwic/alsa-sys/issues/10
