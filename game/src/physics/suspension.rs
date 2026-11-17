//! Raycast suspension spring/damper.
//!
//! Ground truth: the `.TRK` wheel-rate fields and the retail damper formula
//! `c = 3.5·√spring_rate` lb·s/ft (`decomp_4x42_full.c` 199 044-199 051 and
//! 200 586-200 592; the constant is `DAT_0065d482` / `DAT_0065d8a2` = 3.5f read
//! from `4x42.exe`). See `docs/PHYSICS.md`.
//!
//! The force is evaluated at the wheel's attachment point and applied along
//! the body up axis by the caller. It only ever pushes (a wheel cannot pull
//! the chassis down below full droop), so the result is clamped at zero.

/// `damping_cf = DAMPING_SQRT_FACTOR · √spring_rate` (retail `4x42.exe`).
pub const DAMPING_SQRT_FACTOR: f32 = 3.5;
/// lb·s/ft → N·s/m.
pub const LBF_S_PER_FT: f32 = 14.593_903;
/// Bump stops start at this share of the travel.
pub const BUMP_STOP_ZONE: f32 = 0.92;
/// Compression damping multiplier (plush bump absorption).
const BUMP_DAMPING: f32 = 0.90;
/// Rebound damping multiplier so landings settle without pogo-bouncing.
const REBOUND_DAMPING: f32 = 2.25;
/// Hydraulic cushion starts at this share of the travel.
const CUSHION_ZONE: f32 = 0.72;
/// Progressive bump-stop stiffness relative to the spring rate.
const BUMP_STOP_STIFFNESS: f32 = 8.0;
/// Rebound-side bump-stop stiffness relative to the spring rate.
const BUMP_STOP_REBOUND_STIFFNESS: f32 = 2.5;

/// One corner's spring/damper, in SI units.
#[derive(Clone, Copy, Debug)]
pub struct SpringDamper {
    /// Wheel rate (N/m).
    pub rate_n_m: f32,
    /// Damper rate at the wheel (N·s/m).
    pub damper_n_s_m: f32,
    /// Travel before the bump stop (m).
    pub travel_m: f32,
}

impl SpringDamper {
    pub fn new(rate_n_m: f32, damper_n_s_m: f32, travel_m: f32) -> Self {
        Self {
            rate_n_m: rate_n_m.max(0.0),
            damper_n_s_m: damper_n_s_m.max(0.0),
            travel_m: travel_m.max(0.02),
        }
    }

    /// Damper rate for a retail axle spring rate in lb/ft.
    pub fn retail_damper(spring_rate_lb_ft: f32) -> f32 {
        DAMPING_SQRT_FACTOR * spring_rate_lb_ft.max(1.0).sqrt() * LBF_S_PER_FT
    }

    /// `F = k·x + c·ẋ`, plus the hydraulic cushion and bump stops.
    ///
    /// `compression_m` is the distance the spring is compressed from full
    /// droop; `compression_vel_m_s` is positive while compressing. Rebound
    /// (negative velocity) uses the stiffer retail-biased curve so a landing
    /// does not pogo.
    pub fn force(&self, compression_m: f32, compression_vel_m_s: f32) -> f32 {
        let comp = compression_m.max(0.0);
        let k = self.rate_n_m;
        let damp = self.damper_n_s_m;
        let travel = self.travel_m;
        let eff_damp = if compression_vel_m_s < 0.0 {
            damp * REBOUND_DAMPING
        } else {
            damp * BUMP_DAMPING
        };
        let mut fs = k * comp + eff_damp * compression_vel_m_s;

        let cushion_zone = travel * CUSHION_ZONE;
        if comp > cushion_zone && compression_vel_m_s > 0.0 {
            let pen = (comp - cushion_zone) / (travel * (1.0 - CUSHION_ZONE)).max(0.02);
            fs += damp * 1.8 * compression_vel_m_s * pen.clamp(0.0, 2.0);
        }

        let stop_zone = travel * BUMP_STOP_ZONE;
        if comp > stop_zone {
            if compression_vel_m_s > 0.0 {
                fs += k * BUMP_STOP_STIFFNESS * (comp - stop_zone)
                    + damp * BUMP_STOP_REBOUND_STIFFNESS * compression_vel_m_s;
            } else {
                fs += k * BUMP_STOP_REBOUND_STIFFNESS * (comp - stop_zone);
            }
        }
        fs.max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hookes_law_at_rest() {
        let s = SpringDamper::new(30_000.0, 2_000.0, 0.3);
        assert!((s.force(0.10, 0.0) - 3_000.0).abs() < 1e-3);
        assert_eq!(s.force(0.0, 0.0), 0.0);
    }

    #[test]
    fn compression_damping_adds_force_and_rebound_removes_it() {
        let s = SpringDamper::new(30_000.0, 2_000.0, 0.3);
        let static_force = s.force(0.1, 0.0);
        assert!(s.force(0.1, 0.2) > static_force);
        assert!(s.force(0.1, -0.2) < static_force);
        // A wheel near full droop cannot pull the chassis down.
        assert_eq!(s.force(0.0, -1.0), 0.0);
    }

    #[test]
    fn bump_stop_is_progressive() {
        let s = SpringDamper::new(30_000.0, 2_000.0, 0.3);
        let before = s.force(0.27, 0.0);
        let after = s.force(0.295, 0.0);
        let spring_only_at_after = 30_000.0 * 0.295;
        assert!(before > 0.0);
        assert!(
            after > spring_only_at_after,
            "bump stop must add force: {after} vs {spring_only_at_after}"
        );
    }

    #[test]
    fn retail_damper_uses_the_3_5_sqrt_formula() {
        // 1178.4 lb/ft (selftest/retail front axle) → ~1864 N·s/m.
        let c = SpringDamper::retail_damper(1178.4);
        let expected = 3.5 * 1178.4f32.sqrt() * 14.593_903;
        assert!((c - expected).abs() < 1e-3);
    }
}
