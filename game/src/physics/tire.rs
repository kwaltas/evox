//! Tyre slip and surface friction.
//!
//! One Pacejka curve is shared by the lateral and longitudinal axes, exactly
//! as the offroad4x4 prototype that finished every retail track. Forces act in
//! the terrain tangent plane, so grip scales with the surface normal and a
//! grade does not eat the drive force. See `docs/PHYSICS.md` for the retail
//! surface-grip table and the friction-ellipse clamp.

use crate::terrain::Surface;

/// Pacejka `B` (stiffness).
pub const TIRE_B: f32 = 8.6;
/// Pacejka `C` (shape).
pub const TIRE_C: f32 = 1.62;
/// Pacejka `E` (curvature).
pub const TIRE_E: f32 = 0.92;
/// Longitudinal share of the same curve (the prototype's `TYRE_LONG_SHARE`).
pub const TIRE_LONG_SHARE: f32 = 0.92;
/// Below this speed the slip angle is computed against it, so parking does not
/// spike the lateral force.
pub const TYRE_SLIP_FLOOR: f32 = 0.8;
/// Longitudinal demand is clamped to this many friction limits before the
/// curve, keeping the extrapolation finite.
pub const DEMAND_RANGE: f32 = 3.0;

/// Pacejka magic formula: `sin(atan(C·B·x − E·(B·x − atan(B·x))))`.
pub fn magic(x: f32) -> f32 {
    let bx = TIRE_B * x;
    (TIRE_C * (bx - TIRE_E * (bx - bx.atan())).atan()).sin()
}

/// Slip angle from the contact-patch velocity, with a low-speed floor so a
/// parked truck does not generate a huge lateral force.
pub fn slip_angle(v_long: f32, v_lat: f32) -> f32 {
    v_lat.atan2(v_long.abs().max(TYRE_SLIP_FLOOR))
}

/// Normalised longitudinal demand for a raw force request.
pub fn longitudinal_demand(force_n: f32, limit_n: f32) -> f32 {
    (force_n / limit_n.max(1.0)).clamp(-DEMAND_RANGE, DEMAND_RANGE)
}

/// Force from the shared Pacejka curve at a normalised demand.
pub fn longitudinal_force(demand: f32, limit_n: f32) -> f32 {
    magic(demand) * limit_n * TIRE_LONG_SHARE
}

/// Friction coefficient at a contact patch:
/// `base · (1 + tire_cf) · surface.grip · corner_cf`.
///
/// The `.CLR` material index is the terrain splat mask: asphalt 1.12, dirt
/// 0.92, gravel 0.84, grass 0.78, mud 0.66, sand 0.62, snow 0.55, water 0.48.
pub fn surface_mu(base_mu: f32, tire_cf: f32, surface: Surface, corner_cf: f32) -> f32 {
    base_mu * (1.0 + tire_cf) * surface.grip() * corner_cf.max(0.2)
}

/// Clamp a force pair to the friction circle `μ·Fz` while preserving its
/// direction. Returns the pair and whether it was clipped.
pub fn clamp_ellipse(f_long: f32, f_lat: f32, limit_n: f32) -> (f32, f32, bool) {
    let mag = (f_long * f_long + f_lat * f_lat).sqrt();
    if mag > limit_n && mag > 0.0 {
        (f_long * limit_n / mag, f_lat * limit_n / mag, true)
    } else {
        (f_long, f_lat, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_curve_is_odd_and_saturates() {
        assert!(magic(0.0).abs() < 1e-6);
        assert!((magic(0.3) + magic(-0.3)).abs() < 1e-6);
        assert!(magic(0.52) <= 1.0 && magic(0.52) > 0.9);
        assert!(magic(0.52) > magic(0.17));
    }

    #[test]
    fn slip_angle_is_stable_at_a_standstill() {
        assert!(slip_angle(0.0, 0.0).abs() < 1e-6);
        let a = slip_angle(0.0, 1.0);
        assert!((a - 1.0f32.atan2(TYRE_SLIP_FLOOR)).abs() < 1e-6);
        // Longitudinal speed beyond the floor drives the angle down.
        assert!(slip_angle(10.0, 1.0).abs() < a.abs());
    }

    #[test]
    fn asphalt_has_more_grip_than_dirt_and_mud() {
        let asphalt = surface_mu(1.0, 0.0, Surface::Asphalt, 1.0);
        let dirt = surface_mu(1.0, 0.0, Surface::Dirt, 1.0);
        let mud = surface_mu(1.0, 0.0, Surface::Mud, 1.0);
        assert!(asphalt > dirt && dirt > mud);
        assert!((asphalt - 1.12).abs() < 1e-6);
    }

    #[test]
    fn ellipse_preserves_direction_and_respects_the_limit() {
        let (fl, fa, clipped) = clamp_ellipse(6000.0, 3000.0, 5000.0);
        assert!(clipped);
        assert!((fl * fl + fa * fa).sqrt() - 5000.0 < 1e-3);
        assert!((fl / fa - 2.0).abs() < 1e-4);
        let (fl, fa, clipped) = clamp_ellipse(1000.0, 500.0, 5000.0);
        assert!(!clipped && fl == 1000.0 && fa == 500.0);
    }
}
