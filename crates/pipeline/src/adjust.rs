use serde::{Deserialize, Serialize};

/// The edit a person makes to one photo. All values are relative to the
/// camera's own rendering, so `Adjustments::default()` means "as shot".
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjustments {
    /// Exposure change in stops (EV). +1 doubles the light.
    pub exposure: f32,
    /// White-balance temperature shift, -100 (cooler/bluer) to +100 (warmer/yellower).
    pub temperature: f32,
    /// White-balance tint shift, -100 (greener) to +100 (more magenta).
    pub tint: f32,
}

impl Default for Adjustments {
    fn default() -> Self {
        Self { exposure: 0.0, temperature: 0.0, tint: 0.0 }
    }
}

pub const EXPOSURE_RANGE: (f32, f32) = (-5.0, 5.0);
pub const TEMPERATURE_RANGE: (f32, f32) = (-100.0, 100.0);
pub const TINT_RANGE: (f32, f32) = (-100.0, 100.0);

impl Adjustments {
    /// Clamp every value into its slider range and replace NaN with the
    /// default, so a hand-edited or damaged edit file can't break rendering.
    pub fn sanitized(self) -> Self {
        fn fix(v: f32, (lo, hi): (f32, f32)) -> f32 {
            if v.is_finite() { v.clamp(lo, hi) } else { 0.0 }
        }
        Self {
            exposure: fix(self.exposure, EXPOSURE_RANGE),
            temperature: fix(self.temperature, TEMPERATURE_RANGE),
            tint: fix(self.tint, TINT_RANGE),
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_and_drops_nan() {
        let a = Adjustments { exposure: 99.0, temperature: f32::NAN, tint: -500.0 }.sanitized();
        assert_eq!(a, Adjustments { exposure: 5.0, temperature: 0.0, tint: -100.0 });
    }
}
