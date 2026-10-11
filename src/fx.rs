//! Motion for the HUD, as pure functions of time and seeds so every frame is
//! reproducible in tests. Two kinds of motion exist and are kept apart:
//! audio-reactive motion (bass glow, peak caps, the buffering fill) follows
//! real engine data and comes to rest with it; decorative motion (the title
//! decode on a track change, the ON AIR pulse) runs only briefly after an
//! event or as a slow 1 Hz blink. `RADIO_RECORD_FX` selects how much runs.

use ratatui::text::Span;
use std::time::Duration;

/// How long a new track's title takes to decode.
pub const REVEAL: Duration = Duration::from_millis(450);
/// Half-period of the ON AIR pulse.
pub const PULSE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Fx {
    /// Audio-reactive and decorative motion.
    Full,
    /// Audio-reactive motion only; nothing moves without audio.
    Calm,
    /// A still interface: no glow, no reveal, no pulse.
    #[default]
    Off,
}

impl Fx {
    /// Map a `RADIO_RECORD_FX` value to a mode. Empty and unknown values mean
    /// `full`; `NO_COLOR` caps it at `calm`, since glows are color effects.
    pub fn resolve(no_color: bool, name: Option<&str>) -> Self {
        let mode = match name.map(str::trim) {
            Some("calm") => Self::Calm,
            Some("off") => Self::Off,
            _ => Self::Full,
        };
        if no_color && mode == Self::Full {
            Self::Calm
        } else {
            mode
        }
    }

    /// Short effects after events, and the ON AIR pulse.
    pub fn decorative(self) -> bool {
        self == Self::Full
    }

    /// Effects that follow the audio and buffering.
    pub fn reactive(self) -> bool {
        self != Self::Off
    }
}

/// Resolve the motion mode from `RADIO_RECORD_FX` and `NO_COLOR`, noting an
/// unknown value on stderr. Called once, before the terminal takes over.
pub fn from_env() -> Fx {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    let name = std::env::var("RADIO_RECORD_FX").unwrap_or_default();
    let name = name.trim();
    if !name.is_empty() && !["full", "calm", "off"].contains(&name) {
        eprintln!("radio-record: unknown RADIO_RECORD_FX '{name}', using full");
    }
    Fx::resolve(no_color, Some(name))
}

/// Progress of an effect that started at `since`, or `None` once it is over
/// (or if the clock is behind it).
pub fn progress(now: Duration, since: Duration, length: Duration) -> Option<f32> {
    let elapsed = now.checked_sub(since)?;
    (elapsed < length).then(|| elapsed.as_secs_f32() / length.as_secs_f32())
}

/// A square wave: true for the first half of every `2 × half` period.
pub fn blink(now: Duration, half: Duration) -> bool {
    (now.as_millis() / half.as_millis().max(1)).is_multiple_of(2)
}

const GLYPHS: [&str; 12] = ["░", "▒", "▓", "#", "@", "%", "&", "0", "1", "/", "<", ">"];

/// Decode `text` left to right: characters before `progress` are final, the
/// rest cycle through noise glyphs chosen by `seed` and the frame. Spaces
/// and wide characters stay put, so the line never changes width.
pub fn scramble(text: &str, progress: f32, seed: u64) -> String {
    let progress = if progress.is_finite() {
        progress.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let count = text.chars().count();
    let resolved = (progress * count as f32).ceil() as usize;
    let step = (progress * 20.0) as u64;
    text.chars()
        .enumerate()
        .map(|(index, character)| {
            let narrow = Span::raw(character.to_string()).width() == 1;
            if index < resolved || character.is_whitespace() || !narrow {
                character.to_string()
            } else {
                let pick = mix(seed, index as u64, step) % GLYPHS.len() as u64;
                GLYPHS[pick as usize].to_owned()
            }
        })
        .collect()
}

/// A stable 64-bit hash for effect seeds (splitmix64 over three words).
pub fn mix(a: u64, b: u64, c: u64) -> u64 {
    let mut z = a
        .wrapping_add(b.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(c.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_resolve_with_no_color_capping_motion() {
        assert_eq!(Fx::resolve(false, None), Fx::Full);
        assert_eq!(Fx::resolve(false, Some(" calm ")), Fx::Calm);
        assert_eq!(Fx::resolve(false, Some("off")), Fx::Off);
        assert_eq!(Fx::resolve(false, Some("sparkles")), Fx::Full);
        assert_eq!(Fx::resolve(true, None), Fx::Calm);
        assert_eq!(Fx::resolve(true, Some("off")), Fx::Off);
        assert!(Fx::Full.decorative() && Fx::Full.reactive());
        assert!(!Fx::Calm.decorative() && Fx::Calm.reactive());
        assert!(!Fx::Off.decorative() && !Fx::Off.reactive());
    }

    #[test]
    fn scramble_resolves_to_the_text_and_keeps_its_shape() {
        let text = "Sound Of Goodbye";
        assert_eq!(scramble(text, 1.0, 7), text);
        assert_eq!(scramble(text, f32::NAN, 7), text);
        let noise = scramble(text, 0.0, 7);
        assert_ne!(noise, text);
        assert_eq!(noise.chars().count(), text.chars().count());
        for (a, b) in noise.chars().zip(text.chars()) {
            assert_eq!(a == ' ', b == ' ', "spaces stay in place");
        }
        assert_eq!(scramble(text, 0.3, 7), scramble(text, 0.3, 7), "seeded");
        let half = scramble(text, 0.5, 7);
        assert!(
            half.starts_with("Sound Of"),
            "the left half is final: {half}"
        );
        let cyrillic = scramble("Синтвейв", 0.0, 3);
        assert_eq!(cyrillic.chars().count(), 8);
    }

    #[test]
    fn effects_run_for_their_length_only() {
        let start = Duration::from_secs(10);
        assert_eq!(progress(start, start, REVEAL), Some(0.0));
        let half = progress(start + REVEAL / 2, start, REVEAL).unwrap();
        assert!((half - 0.5).abs() < 1e-3);
        assert_eq!(progress(start + REVEAL, start, REVEAL), None);
        assert_eq!(progress(Duration::ZERO, start, REVEAL), None);
        assert!(blink(Duration::from_millis(100), PULSE));
        assert!(!blink(Duration::from_millis(600), PULSE));
        assert!(blink(Duration::from_millis(1100), PULSE));
    }
}
