# radio-record

<p align="center">
  <strong>A keyboard-first internet radio player for macOS and Linux.</strong><br>
  Browse stations, control playback, and keep the music in focus.
</p>

<p align="center">
  <a href="https://github.com/isalikov/radio-record-cli/releases"><img src="https://img.shields.io/github/v/release/isalikov/radio-record-cli?style=flat-square&color=ff2bd6" alt="Latest release"></a>
  <a href="https://github.com/isalikov/radio-record-cli/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflow/status/isalikov/radio-record-cli/release.yml?style=flat-square&label=release" alt="Release build"></a>
</p>

<p align="center">
  <img src="assets/player-preview.png" alt="radio-record player preview">
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#keyboard">Keyboard</a> ·
  <a href="#support">Support</a>
</p>

Keyboard-only Rust radio player for [Radio Record](https://www.radiorecord.ru/).
Categories on the left, stations on the right, recent tracks below.
The accent color marks playback; the alert color marks the selected category
and favorites.

## Run

Building from source requires Rust/Cargo. On Linux, also install the ALSA
headers and pkg-config (`sudo apt install libasound2-dev pkg-config` on
Debian/Ubuntu). Release binaries need only the ALSA runtime library
(`libasound.so.2`); macOS uses the built-in CoreAudio framework. HTTP, TLS,
and AAC decoding are compiled into the binary.

Audio uses the built-in engine: direct AAC-LC streams and the AAC-LC variant
of HLS. HE-AAC-only stations are unsupported and report a clear error.

The Rust toolchain is pinned in `mise.toml`. With [mise](https://mise.jdx.dev) installed,
`mise install` in this directory fetches the right `cargo`, `rustfmt`, and `clippy`.
Any other Rust installation (rustup, distro package) works as well.

```sh
mise install
make run
```

Plain `make` prints help. Minimum terminal size: 44 × 12. History is hidden in
short windows to leave room for stations. Truecolor terminals show the full palette.

`RADIO_RECORD_THEME` selects a color theme: `default` (cyan and magenta on dark),
`amber` (monochrome amber CRT), `phosphor` (green terminal), or `paper`
(light, for white terminals). An unknown name falls back to `default` with a
note on stderr. `NO_COLOR` disables colors and takes precedence over the theme.

## Install

Once release archives are published, users can install with one command:

```sh
curl -fsSL https://raw.githubusercontent.com/isalikov/radio-record-cli/master/scripts/install.sh | sh
```

The script downloads the latest GitHub Release for the current macOS or Linux
architecture. If no release archive exists yet, it falls back to `cargo install`
from this repository, so the same command also works for local development
machines that already have Rust installed.

Installed binaries go to `~/.local/bin/radio-record` by default. If that directory is
not in `PATH`, add it once in your shell profile.

The binary uses CoreAudio on macOS and the ALSA runtime library on Linux.
HTTP, TLS, and AAC decoding are built in.

### Troubleshooting

**`audio device failed · Enter to retry`** — the native output stream reported
a fatal error. Press Enter to reopen it. Brief device underruns/overruns,
automatic audio-route changes, and denied real-time scheduling do not stop
playback; a brief audio glitch may still be audible.

**`audio device error: ... Sample rate update timed out`** — CoreAudio could
not finish opening the output while the device format was changing. Press
Enter to retry. radio-record opens the current hardware rate, including lower-rate
Bluetooth call formats such as 24 kHz, and resamples radio audio to it. Mono
output mixes both channels. On macOS the hardware clock is read separately
from the virtual stream format, so a call's lower rate is preserved. Bluetooth
headphones can sound less clear while their microphone is active.

**`HE-AAC streams are unsupported · select another station`** — the station
only offers streams the built-in AAC-LC decoder cannot play. Select a station
with a direct AAC-LC stream or an AAC-LC HLS variant.

**`<host> unreachable · Enter to retry`** — the player could not open any of
the station's streams. radio-record falls back from the direct streams to HLS and
then retries the whole list two more times, 2 seconds apart, before showing
this; the message names the hosts that failed. The HLS host is a rotating DNS
pool, so a retry can land on an address that works. A VPN or split-tunnel rule
that sends some of these addresses around the tunnel is a common cause: route
`*.hostingradio.ru` and `*.magonet.ru` through the VPN, or check with
`curl -sI https://radiorecord.hostingradio.ru/rr_main96.aacp`. Other errors
are shown as `Stream unavailable: <reason>`.

## Uninstall

```sh
curl -fsSL https://raw.githubusercontent.com/isalikov/radio-record-cli/master/scripts/uninstall.sh | sh
```

This removes `~/.local/bin/radio-record` (or `$RADIO_RECORD_PREFIX/bin/radio-record` if you installed
with a custom prefix) and keeps your favorites and volume. To delete the settings too,
pass `--purge` when running the script directly, or set `RADIO_RECORD_PURGE=1` when piping
it from curl:

```sh
curl -fsSL https://raw.githubusercontent.com/isalikov/radio-record-cli/master/scripts/uninstall.sh | RADIO_RECORD_PURGE=1 sh
```

Settings live in `~/.config/radio-record` by default; the script honours the same
`RADIO_RECORD_CONFIG_DIR` and `XDG_CONFIG_HOME` overrides as the app.

## Support

If radio-record is useful to you, you can support its development on Ko-fi:

<p align="center">
  <a href="https://ko-fi.com/isalikov"><img src="https://ko-fi.com/img/githubbutton_sm.svg" alt="Support radio-record on Ko-fi"></a>
</p>

Bug reports, station suggestions, and small improvements are welcome too.

## Keyboard

| Key | Action |
| --- | --- |
| Tab / Shift+Tab | Next / previous category, wrapping around |
| ↑ / ↓, j / k | Select a station without changing playback |
| Enter | Play selected station |
| F7 / F9 | Play previous / next station in the current category |
| F8 / Space | Play / pause |
| - / = | Player volume, 2% steps, 0–100% |
| f | Toggle favorite |
| i | Show / hide recent tracks |
| d | Toggle diagnostics overlay |
| a | Open account, sign in, sync favorites, or sign out |
| PgUp / PgDn, Home / End | Scroll stations |
| r | Refresh stations and history |
| s | Stop |
| ? / Esc | Open / close help |
| q / Ctrl+C | Quit |

Previous/next follows the playing station when it is in the current category;
otherwise it starts at the selected station. Unavailable streams are skipped.
Function transport keys also work while help is open.
Left/right arrows have no action. History is read-only. There is no search.

Favorites have a small magenta `+` outside the Favorites category. Inside Favorites,
the category itself identifies them, so the marker is hidden. Station and track
names retain their original spelling from the API; application labels are English.

Terminal F7/F8/F9 and forwarded media-key events are supported in every
engine. On macOS, radio-record registers system previous/play-pause/next controls
during playback. The hardware media keys work while radio-record
owns the active media session, including when its terminal is unfocused.
The session shows the station name and playback state and is cleared on
stop, playback failure, or exit. Fn+F7/F8/F9 sends ordinary function keys to
the terminal. No global keyboard interception is installed.

## Account and favorites sync

Press `a` to open your Radio Record account. Enter your email and password,
use Tab to move between fields, then Enter to sign in. Pasting is supported;
the password is masked and never saved. Esc closes the window and clears the
password. F7/F8/F9 and media keys still control playback while it is open.

The account window shows your name, email, Premium status, and sync status.
`s` (or Enter) syncs favorites now; `l` signs out after confirmation and lets
you switch accounts. Signing out keeps local favorites. Editing personal
information, registration, and password recovery remain on the Radio Record
website.

Favorites sync in the background on startup, 750 ms after local changes settle,
and every 30 seconds while signed in. Changes on only one side apply
automatically. On first sign-in when the lists differ, or when both sides have
changed since the last successful sync, a dialog offers three choices:

- **Merge both** preserves independent additions. With a previous sync baseline,
  it also honors deletions from either side; on first sign-in it keeps the union.
- **Use local on both** replaces server favorites with the local list.
- **Use server on both** replaces local favorites with the server list.

Use arrows or Tab to select, or 1/2/3, then Enter to apply. The dialog previews
station counts and additions/removals on each side. Esc defers the decision;
`a` reopens it. No replacement happens until you choose. A new server change
while reviewing causes the lists to be checked again before writing. Local
edits made during a request are kept for the next sync.

Only favorite **stations** are synced; favorite tracks and podcasts are untouched.
Network failures leave local favorites and the last successful baseline intact.
The sidebar shows `a <email>` while signed in and `a Account` otherwise.
A trailing `!` marks account issues; open the window to see the error and retry
with `s`. Offline playback and local favorites remain available.

The device session and per-account sync baseline are stored in `account.json`
next to `settings.json`, with owner-only permissions on Unix. Signing out removes
that file. Cookies and passwords are not stored. `RADIO_RECORD_CONFIG_DIR` and
`XDG_CONFIG_HOME` apply to both files.

## Audio and settings

Native audio uses HTTP (rustls), a pure-Rust AAC-LC decoder (Symphonia),
and a fixed-capacity ring feeding CoreAudio or ALSA. The engine preserves the
current output-device format, including sample rates below the radio stream
and mono headset outputs. On macOS it reads the hardware clock rather than
assuming the virtual stream format has the same rate. Matching rates pass
through directly; faster rates use linear interpolation; slower rates use a
low-pass filter before interpolation to suppress aliasing. Filter coefficients
are prepared before playback, and the callback allocates nothing. A mono
device receives the average of both channels. Unsupported device formats
are reported as audio-device errors.

Native pause silences output on the next callback and discards queued audio,
including the resampler's look-ahead and filter history. The reader keeps draining the radio;
resume buffers fresh audio. HLS rejoins recent published segments, so its
normal segment delay still applies.

HLS segments are fed completely as ring space becomes available; they are
not truncated to the ring's capacity. The output waits for 0.3 seconds of
audio at startup and after starvation. Its buffering percentage measures
progress toward that threshold. Cache fill, underruns, and audio levels come
from the output callback, so they follow consumption even while HTTP is idle.

Failed URLs fall through to the next supported stream, with three rounds
and a two-second delay between rounds. Each URL change resets the ring,
output sample rate, and telemetry. Stalled HLS playlists and repeated corrupt
segments trigger fallback instead of leaving playback silent indefinitely.
The `d` diagnostics overlay shows cache fill and underruns.

One smooth bar follows the audio before software gain, riding the louder channel. Its scale
adapts to the last four seconds of the station's loudness, leaving headroom
for beats. Attack and release are eased; there is no scrolling, peak marker,
or synthetic animation. Silence, pause and buffering settle the bar, and mono
sources mirror the left channel. Physical stroke thickness is determined by
the terminal font, not an exact pixel size.

Press `d` for a diagnostics overlay: player state and generation, codec, sample
rate, channels, bitrate, the active stream and its fallback round, cache fill
and underruns, worker health, history age, uptime, and a sparkline of recent
levels. The built-in engine measures telemetry in-process. While a stream buffers, the status symbol
shows the real cache percentage instead of a bare dot. If the player worker
stops reporting for two seconds while audio should be alive, the footer shows
`Player engine stalled · q quit`. The warning clears when the worker resumes
reporting; each new play request starts a fresh two-second deadline.

The UI and application logic use Rust, Ratatui and Crossterm. The built-in
engine decodes AAC in-process. macOS system media controls belong to radio-record
and remain available across station changes. Volume is software gain
and does not change system volume.

Favorites and volume are saved atomically in `~/.config/radio-record/settings.json`.
Directory precedence: `RADIO_RECORD_CONFIG_DIR`, `$XDG_CONFIG_HOME/radio-record`,
`~/.config/radio-record`. Corrupt settings are reported rather than overwritten.

History and current track metadata refresh every 15 seconds. The API may lag
behind audio, especially after pausing. `RADIO_RECORD_BASE_URL` overrides the API
base URL for development. Requests time out after 15 seconds.
Stream priority: `stream_320`, then the AAC-LC variant of `stream_hls`. If a
stream fails to load, the player tries the next distinct supported URL before
reporting an error. The HE-AAC `stream_128` and `stream_64` entries are skipped.

## Verify

```sh
make build
make test
make check
make test-audio
```

`make test` runs offline: stream fixtures recorded from the real streams are
decoded, and the engine pipeline is tested against a null output sink using
the same callback as the real device. Tests cover long segments, pause and
resume, stale buffers, station changes, stream fallback, and filtered downsampling. Live
checks that need the network run with
`cargo test live_ -- --ignored` (direct stream, HLS, and a short burst on the
real audio device). The HLS check observes multiple segment publications
for continuity. `make test-audio` opens the current real output format with
silent audio and verifies that its hardware rate and channels remain unchanged.
Account tests use a local HTTP server for login, merge decisions, write
verification, and request failures; they need no real account.
