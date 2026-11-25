//! Auto quality, one rule for every device: start at [`START`] (or under the account's ceiling), then move as
//! Android's player (ExoPlayer's `AdaptiveTrackSelection`) and hls.js judge it. The bandwidth is two averages of
//! what was measured, a quick one and a slow one, and the lower is believed; a level is wanted when it fits in
//! [`FRACTION`] of that; going up waits for [`UP_AFTER`] seconds buffered, going down only happens under
//! [`DOWN_UNDER`].

use crate::hls::Rung;

/// The height Auto starts at on every device.
pub const START: i64 = 1080;
/// The share of the measured bandwidth a level may use (ExoPlayer's `DEFAULT_BANDWIDTH_FRACTION`).
pub const FRACTION: f64 = 0.7;
/// Seconds buffered ahead before Auto goes up a level (ExoPlayer's `minDurationForQualityIncreaseMs`).
pub const UP_AFTER: f64 = 10.;
/// Auto goes down only with less than this buffered ahead (ExoPlayer's `maxDurationForQualityDecreaseMs`).
pub const DOWN_UNDER: f64 = 25.;
/// Half-lives of the quick and slow averages, in seconds of measuring (hls.js's VoD EWMA).
const FAST: f64 = 3.;
const SLOW: f64 = 9.;
/// Seconds of measuring before the estimate is trusted.
const TRUSTED_AFTER: f64 = 2.;

/// The bandwidth as measured, in bits a second: hls.js's pair of exponentially weighted averages.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Estimate {
    fast: f64,
    slow: f64,
    /// Seconds measured so far: corrects the averages' pull towards zero while they're young.
    weight: f64,
}

impl Estimate {
    /// `bits` a second measured over the last `seconds`; nothing is learned while nothing was downloading.
    pub fn sample(&mut self, bits: f64, seconds: f64) {
        if bits <= 0. || seconds <= 0. {
            return;
        }
        let blend = |avg: f64, half: f64| {
            let keep = 0.5_f64.powf(seconds / half);
            keep * avg + (1. - keep) * bits
        };
        self.fast = blend(self.fast, FAST);
        self.slow = blend(self.slow, SLOW);
        self.weight += seconds;
    }

    /// The lower of the two averages, once there's enough of it; None before.
    pub fn bits(&self) -> Option<f64> {
        if self.weight < TRUSTED_AFTER {
            return None;
        }
        let corrected = |avg: f64, half: f64| avg / (1. - 0.5_f64.powf(self.weight / half));
        Some(corrected(self.fast, FAST).min(corrected(self.slow, SLOW)))
    }
}

/// The tallest level that fits in [`FRACTION`] of `bits`, at most `top` high; the lowest when none fits.
pub fn ideal(levels: &[Rung], bits: f64, top: i64) -> Option<Rung> {
    let allowed = levels.iter().filter(|r| r.height <= top);
    allowed.clone().filter(|r| r.bandwidth as f64 <= bits * FRACTION).max_by_key(|r| r.height)
        .or_else(|| allowed.min_by_key(|r| r.height)).copied()
}

/// Where Auto goes from `now` given the `ideal` level and the seconds buffered `ahead`; None to stay.
pub fn switch(now: Rung, ideal: Rung, ahead: f64) -> Option<Rung> {
    let up = ideal.height > now.height && ahead >= UP_AFTER;
    let down = ideal.height < now.height && ahead < DOWN_UNDER;
    (up || down).then_some(ideal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rung(height: i64, mbit: f64) -> Rung {
        Rung { height, bandwidth: (mbit * 1e6) as u64 }
    }

    #[test]
    fn the_estimate_believes_the_lower_average_once_trusted() {
        let mut e = Estimate::default();
        e.sample(20e6, 1.);
        assert_eq!(e.bits(), None);
        e.sample(20e6, 2.);
        assert!((e.bits().unwrap_or(0.) - 20e6).abs() < 1.);
        // (a sudden drop pulls the quick average down first, and that's the one believed)
        e.sample(2e6, 1.);
        let bits = e.bits().unwrap_or(0.);
        assert!(bits < 15e6 && bits > 2e6, "{bits}");
        // (no download: nothing learned)
        let before = e;
        e.sample(0., 5.);
        assert_eq!(e, before);
    }

    #[test]
    fn auto_wants_the_tallest_level_that_fits_under_the_ceiling() {
        let levels = [rung(480, 1.3), rung(720, 3.1), rung(1080, 6.1), rung(2160, 16.1)];
        assert_eq!(ideal(&levels, 30e6, 2160).map(|r| r.height), Some(2160));
        assert_eq!(ideal(&levels, 30e6, 1080).map(|r| r.height), Some(1080));
        assert_eq!(ideal(&levels, 9e6, 2160).map(|r| r.height), Some(1080));
        assert_eq!(ideal(&levels, 1e6, 2160).map(|r| r.height), Some(480));
    }

    #[test]
    fn up_waits_for_a_buffer_and_down_only_happens_short_of_one() {
        let (hd, uhd, sd) = (rung(1080, 6.1), rung(2160, 16.1), rung(720, 3.1));
        assert_eq!(switch(hd, uhd, 4.), None);
        assert_eq!(switch(hd, uhd, 12.), Some(uhd));
        assert_eq!(switch(hd, sd, 30.), None);
        assert_eq!(switch(hd, sd, 8.), Some(sd));
        assert_eq!(switch(hd, hd, 0.), None);
    }
}
