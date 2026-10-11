//! Frequency analysis for the visualizer. The native engine's worker feeds
//! one window of mono samples per tick (the newest audio the listener is
//! hearing); the analyzer returns `BANDS` log-spaced levels in 0..1 plus
//! falling peak caps. Each band is scaled against its own recent range,
//! blended with the station's overall loudness, so highs move as much as
//! bass. Pure computation: no I/O, no threads, and all buffers are
//! allocated once per sample rate.

use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};
use std::{collections::VecDeque, sync::Arc};

/// Levels produced per frame; the UI pools or interpolates them to columns.
pub(crate) const BANDS: usize = 96;
/// Window length: about 85 ms at 48 kHz, bins of 10.8–11.7 Hz, so every
/// bass band owns at least one bin.
pub(crate) const FFT_SIZE: usize = 4096;
const LOW_HZ: f32 = 50.0;
/// AAC radio streams carry little above this; higher bands would sit dead.
const HIGH_HZ: f32 = 14_000.0;
/// Treble tilt so hats register next to the kick.
const TILT_DB_PER_OCTAVE: f32 = 2.0;
/// Frames below this are treated as silence and never move the auto-gain.
const SILENCE_DB: f32 = -90.0;
/// Recent frame peaks the station ceiling follows: about 5 s at 20 frames a second.
const CEILING_FRAMES: usize = 100;
/// Station range under its ceiling; also the starting range of every band.
const RANGE_DB: f32 = 30.0;
/// How much each band's own range counts against the station's.
const BAND_WEIGHT: f32 = 0.7;
/// A band's range never narrows below this, so noise is not amplified.
const MIN_SPAN_DB: f32 = 12.0;
/// Bands this far under the station ceiling are silent, whatever their history.
const GATE_DB: f32 = 50.0;
const ATTACK: f32 = 0.8;
/// Per-frame acceleration of a falling column: a full drop takes about 0.3 s.
const GRAVITY: f32 = 0.05;
/// Peak caps rest on the highest recent level for this many frames, then fall slowly.
const PEAK_HOLD: u8 = 6;
const PEAK_GRAVITY: f32 = 0.01;

pub(crate) struct Analyzer {
    rate: u32,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    window_sum: f32,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    bands: Vec<Band>,
    ceilings: VecDeque<f32>,
    /// Per-band loud (≈95th percentile) and typical (median) dB, tracked in
    /// fixed steps: no sorting, no allocation.
    band_ceiling: [f32; BANDS],
    band_floor: [f32; BANDS],
    levels: [f32; BANDS],
    velocity: [f32; BANDS],
    peaks: [f32; BANDS],
    peak_velocity: [f32; BANDS],
    peak_hold: [u8; BANDS],
}

/// Which FFT bins one band covers; no two bands share a bin.
#[derive(Clone, Copy, Debug)]
struct Band {
    first: usize,
    last: usize,
    tilt_db: f32,
}

impl Analyzer {
    pub(crate) fn new(rate: u32) -> Self {
        let rate = rate.max(8_000);
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let phase = 2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32;
                0.5 - 0.5 * phase.cos()
            })
            .collect();
        Self {
            rate,
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            window_sum: window.iter().sum(),
            window,
            bands: bands(rate),
            fft,
            ceilings: VecDeque::with_capacity(CEILING_FRAMES + 1),
            band_ceiling: [f32::NEG_INFINITY; BANDS],
            band_floor: [f32::NEG_INFINITY; BANDS],
            levels: [0.0; BANDS],
            velocity: [0.0; BANDS],
            peaks: [0.0; BANDS],
            peak_velocity: [0.0; BANDS],
            peak_hold: [0; BANDS],
        }
    }

    pub(crate) fn rate(&self) -> u32 {
        self.rate
    }

    /// Forget all motion and loudness history, as after a pause or a new stream.
    pub(crate) fn reset(&mut self) {
        self.ceilings.clear();
        self.band_ceiling = [f32::NEG_INFINITY; BANDS];
        self.band_floor = [f32::NEG_INFINITY; BANDS];
        self.levels = [0.0; BANDS];
        self.velocity = [0.0; BANDS];
        self.peaks = [0.0; BANDS];
        self.peak_velocity = [0.0; BANDS];
        self.peak_hold = [0; BANDS];
    }

    /// Peak caps for the levels last returned by `analyze`.
    pub(crate) fn peaks(&self) -> &[f32; BANDS] {
        &self.peaks
    }

    /// Analyze the newest `FFT_SIZE` samples (oldest first). Shorter input is
    /// zero-padded at the front; non-finite samples count as silence.
    pub(crate) fn analyze(&mut self, samples: &[f32]) -> &[f32; BANDS] {
        let samples = &samples[samples.len().saturating_sub(FFT_SIZE)..];
        let pad = FFT_SIZE - samples.len();
        self.input[..pad].fill(0.0);
        for ((slot, sample), weight) in self.input[pad..]
            .iter_mut()
            .zip(samples)
            .zip(&self.window[pad..])
        {
            *slot = if sample.is_finite() {
                sample * weight
            } else {
                0.0
            };
        }
        let mut targets = [0.0f32; BANDS];
        if self
            .fft
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .is_ok()
        {
            let scale = 2.0 / self.window_sum;
            let mut db = [f32::NEG_INFINITY; BANDS];
            for (value, band) in db.iter_mut().zip(&self.bands) {
                let magnitude = self.output[band.first..band.last]
                    .iter()
                    .map(|bin| bin.norm())
                    .fold(0.0, f32::max);
                *value = 20.0 * (magnitude * scale).max(1e-9).log10() + band.tilt_db;
            }
            let peak = db.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            if peak.is_finite() && peak > SILENCE_DB {
                self.ceilings.push_back(peak);
                if self.ceilings.len() > CEILING_FRAMES {
                    self.ceilings.pop_front();
                }
                // Follow the station's loud moments, not its single loudest frame.
                let mut sorted: Vec<f32> = self.ceilings.iter().copied().collect();
                sorted.sort_by(f32::total_cmp);
                let global = sorted[(sorted.len() - 1) * 9 / 10];
                for (index, value) in db.into_iter().enumerate() {
                    targets[index] = self.normalize(index, value, global);
                }
            }
        }
        for (index, target) in targets.into_iter().enumerate() {
            let level = &mut self.levels[index];
            let velocity = &mut self.velocity[index];
            if target >= *level {
                *level += (target - *level) * ATTACK;
                *velocity = 0.0;
            } else {
                *velocity += GRAVITY;
                *level = (*level - *velocity).max(target);
            }
            let peak = &mut self.peaks[index];
            if *level >= *peak {
                *peak = *level;
                self.peak_hold[index] = PEAK_HOLD;
                self.peak_velocity[index] = 0.0;
            } else if self.peak_hold[index] > 0 {
                self.peak_hold[index] -= 1;
            } else {
                self.peak_velocity[index] += PEAK_GRAVITY;
                *peak = (*peak - self.peak_velocity[index]).max(*level);
            }
        }
        &self.levels
    }

    /// Place one band's dB inside its own recent range, blended with the
    /// station's range so a band that is always quiet still reads as quiet.
    fn normalize(&mut self, index: usize, value: f32, global: f32) -> f32 {
        let ceiling = &mut self.band_ceiling[index];
        let floor = &mut self.band_floor[index];
        if !ceiling.is_finite() {
            *ceiling = global;
            *floor = global - RANGE_DB;
        }
        // Stochastic quantile trackers in 1 dB steps: the ceiling settles
        // where 95% of frames fall below it, the floor at the median.
        *ceiling += if value > *ceiling { 0.95 } else { -0.05 };
        *floor += if value > *floor { 0.5 } else { -0.5 };
        if value < global - GATE_DB {
            return 0.0;
        }
        let top = BAND_WEIGHT * *ceiling + (1.0 - BAND_WEIGHT) * global;
        let bottom = (BAND_WEIGHT * *floor + (1.0 - BAND_WEIGHT) * (global - RANGE_DB))
            .min(top - MIN_SPAN_DB);
        ((value - bottom) / (top - bottom)).clamp(0.0, 1.0)
    }
}

/// Log-spaced bands from `LOW_HZ` to `HIGH_HZ` (capped below Nyquist). Where
/// log spacing is finer than the FFT, bands go linear, one bin each, so no
/// two bands read the same bin and neighbors never move in lockstep.
fn bands(rate: u32) -> Vec<Band> {
    let bin_hz = rate as f32 / FFT_SIZE as f32;
    let high = HIGH_HZ.min(rate as f32 * 0.45);
    let edge = |i: usize| LOW_HZ * (high / LOW_HZ).powf(i as f32 / BANDS as f32);
    let max_bin = FFT_SIZE / 2;
    let mut next = 0;
    (0..BANDS)
        .map(|i| {
            let first = ((edge(i) / bin_hz).ceil() as usize).max(next).min(max_bin);
            let last = ((edge(i + 1) / bin_hz).floor() as usize + 1)
                .max(first + 1)
                .min(max_bin + 1);
            next = last;
            let center_hz = (first + last - 1) as f32 / 2.0 * bin_hz;
            Band {
                first,
                last,
                tilt_db: TILT_DB_PER_OCTAVE * (center_hz.max(1.0) / 1000.0).log2(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f32, amplitude: f32, offset: usize) -> Vec<f32> {
        (offset..offset + FFT_SIZE)
            .map(|i| amplitude * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    /// The band whose bins contain `hz`.
    fn band_of(rate: u32, hz: f32) -> usize {
        let bin = (hz / (rate as f32 / FFT_SIZE as f32)).round() as usize;
        bands(rate)
            .iter()
            .position(|band| (band.first..band.last).contains(&bin))
            .unwrap()
    }

    fn loudest(levels: &[f32; BANDS]) -> usize {
        levels
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, _)| index)
            .unwrap()
    }

    #[test]
    fn a_tone_lights_the_band_that_contains_it() {
        for rate in [44_100, 48_000] {
            for hz in [100.0, 1_000.0, 8_000.0] {
                let mut analyzer = Analyzer::new(rate);
                let mut levels = [0.0; BANDS];
                for frame in 0..10 {
                    levels = *analyzer.analyze(&sine(rate, hz, 0.5, frame * 2400));
                }
                let peak = loudest(&levels);
                assert!(
                    peak.abs_diff(band_of(rate, hz)) <= 1,
                    "{hz} Hz at {rate}: peak in band {peak}, expected {}",
                    band_of(rate, hz)
                );
                assert!(levels[peak] > 0.5, "{hz} Hz is too quiet: {levels:?}");
            }
        }
    }

    #[test]
    fn two_tones_make_two_separate_peaks() {
        let rate = 48_000;
        let mut analyzer = Analyzer::new(rate);
        let mut levels = [0.0; BANDS];
        for frame in 0..10 {
            let low = sine(rate, 100.0, 0.4, frame * 2400);
            let high = sine(rate, 6_000.0, 0.4, frame * 2400);
            let mixed: Vec<f32> = low.iter().zip(&high).map(|(a, b)| a + b).collect();
            levels = *analyzer.analyze(&mixed);
        }
        let (low, high) = (band_of(rate, 100.0), band_of(rate, 6_000.0));
        let middle = (low + high) / 2;
        assert!(levels[low] > levels[middle] + 0.3, "{levels:?}");
        assert!(levels[high] > levels[middle] + 0.3, "{levels:?}");
    }

    #[test]
    fn quiet_highs_still_move_against_their_own_range() {
        // A loud kick and hats 30 dB quieter: with one global ceiling the
        // hats would never leave the floor.
        let rate = 44_100;
        let mut analyzer = Analyzer::new(rate);
        let hats = band_of(rate, 9_000.0);
        let mut seen = 0.0f32;
        for frame in 0..60 {
            let kick = sine(rate, 60.0, 0.6, frame * 2205);
            let level = if frame % 4 == 0 { 0.02 } else { 0.002 };
            let hat = sine(rate, 9_000.0, level, frame * 2205);
            let mixed: Vec<f32> = kick.iter().zip(&hat).map(|(a, b)| a + b).collect();
            let levels = analyzer.analyze(&mixed);
            if frame >= 40 {
                seen = seen.max(levels[hats]);
            }
        }
        assert!(seen > 0.5, "hat hits must climb: {seen}");
    }

    #[test]
    fn silence_and_garbage_rest_at_zero() {
        let mut analyzer = Analyzer::new(44_100);
        assert!(analyzer.analyze(&[0.0; FFT_SIZE]).iter().all(|l| *l == 0.0));
        let garbage = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0].repeat(FFT_SIZE / 4);
        assert!(analyzer.analyze(&garbage).iter().all(|l| *l == 0.0));
        assert!(analyzer.analyze(&[]).iter().all(|l| *l == 0.0));
        assert!(analyzer.peaks().iter().all(|l| *l == 0.0));
    }

    #[test]
    fn columns_fall_with_gravity_and_peaks_hold_then_follow() {
        let rate = 44_100;
        let mut analyzer = Analyzer::new(rate);
        for frame in 0..5 {
            analyzer.analyze(&sine(rate, 1_000.0, 0.5, frame * 2205));
        }
        let band = band_of(rate, 1_000.0);
        let top = analyzer.levels[band];
        let mut previous = top;
        let mut drops = Vec::new();
        for frame in 0..40 {
            let level = analyzer.analyze(&[0.0; FFT_SIZE])[band];
            assert!(level <= previous);
            drops.push(previous - level);
            previous = level;
            let peak = analyzer.peaks()[band];
            assert!(peak >= level, "a cap never sinks into its column");
            if frame < usize::from(PEAK_HOLD) {
                assert_eq!(peak, top, "the cap holds at the last high");
            }
        }
        assert_eq!(previous, 0.0, "a column must come to rest");
        assert!(drops[1] > drops[0], "the fall accelerates: {drops:?}");
        assert_eq!(analyzer.peaks()[band], 0.0, "caps come to rest too");
    }

    #[test]
    fn reset_forgets_motion() {
        let rate = 48_000;
        let mut analyzer = Analyzer::new(rate);
        analyzer.analyze(&sine(rate, 440.0, 0.5, 0));
        analyzer.reset();
        assert!(analyzer.levels.iter().all(|l| *l == 0.0));
        assert!(analyzer.peaks.iter().all(|l| *l == 0.0));
        assert!(analyzer.ceilings.is_empty());
        assert!(analyzer.band_ceiling.iter().all(|c| !c.is_finite()));
    }

    #[test]
    fn bands_own_distinct_bins_inside_the_spectrum() {
        for rate in [22_050, 44_100, 48_000, 96_000] {
            let bands = bands(rate);
            assert_eq!(bands.len(), BANDS);
            for pair in bands.windows(2) {
                assert!(pair[1].first >= pair[0].last, "{rate}: bins overlap");
            }
            for band in &bands {
                assert!(band.first < band.last && band.last <= FFT_SIZE / 2 + 1);
            }
        }
    }
}
