//! Rigid-body state and integration for the chassis.
//!
//! The chassis is a single rigid body with a diagonal inertia tensor expressed
//! in the chassis frame. Ground truth for the integration order is the
//! offroad4x4 prototype (`Car::apply_world_torque`): a world torque is rotated
//! into the chassis frame, divided component-wise by the principal moments,
//! and rotated back. Dividing a world torque directly by body-axis inertias
//! mixes the axes on a tilted truck (the bug recorded in `docs/PHYSICS.md`).

use macroquad::prelude::{vec3, Quat, Vec3};

/// Position, orientation and the two velocities of one rigid body.
#[derive(Clone, Copy, Debug)]
pub struct RigidBody {
    /// Centre of mass in world space (metres).
    pub pos: Vec3,
    /// Chassis-to-world rotation.
    pub rot: Quat,
    /// Linear velocity of the centre of mass (m/s).
    pub vel: Vec3,
    /// Angular velocity in world space (rad/s).
    pub angular: Vec3,
}

impl Default for RigidBody {
    fn default() -> Self {
        Self {
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            vel: Vec3::ZERO,
            angular: Vec3::ZERO,
        }
    }
}

impl RigidBody {
    pub fn new(pos: Vec3, rot: Quat) -> Self {
        Self {
            pos,
            rot: rot.normalize(),
            ..Default::default()
        }
    }

    /// `I⁻¹·v` for a world-space vector, with the inertia tensor diagonal in
    /// the chassis frame.
    pub fn inv_inertia_world(&self, v: Vec3, inertia: Vec3) -> Vec3 {
        let local = self.rot.inverse() * v;
        self.rot
            * vec3(
                local.x / inertia.x.max(1e-6),
                local.y / inertia.y.max(1e-6),
                local.z / inertia.z.max(1e-6),
            )
    }

    /// Velocity of a point at offset `r` from the centre of mass.
    pub fn point_velocity(&self, r: Vec3) -> Vec3 {
        self.vel + self.angular.cross(r)
    }

    /// Effective inverse mass along `n` at offset `r`:
    /// `1/m + n·((I⁻¹(r×n))×r)`. Used by the contact solver so an off-centre
    /// corner strike also turns the chassis.
    pub fn inverse_effective_mass(&self, r: Vec3, n: Vec3, mass: f32, inertia: Vec3) -> f32 {
        1.0 / mass.max(1e-6) + n.dot(self.inv_inertia_world(r.cross(n), inertia).cross(r))
    }

    /// Apply impulse `j` (N·s) at offset `r` from the centre of mass.
    pub fn apply_impulse(&mut self, j: Vec3, r: Vec3, mass: f32, inertia: Vec3) {
        self.vel += j / mass.max(1e-6);
        self.angular += self.inv_inertia_world(r.cross(j), inertia);
    }

    /// Symplectic Euler on the linear velocity: `v += F/m·dt`.
    pub fn integrate_force(&mut self, force: Vec3, mass: f32, dt: f32) {
        self.vel += force / mass.max(1e-6) * dt;
    }

    /// Integrate angular velocity against the chassis-frame inertia and apply
    /// the per-step angular damping.
    pub fn integrate_torque(&mut self, torque: Vec3, inertia: Vec3, damping: f32, dt: f32) {
        self.angular += self.inv_inertia_world(torque, inertia) * dt;
        self.angular *= (1.0 - damping * dt).clamp(0.0, 1.0);
    }

    /// Advance position and orientation from the current velocities.
    pub fn integrate_pose(&mut self, dt: f32) {
        self.pos += self.vel * dt;
        let w = self.angular;
        if w.length_squared() > 1e-8 {
            let dq = Quat::from_axis_angle(w.normalize(), w.length() * dt);
            self.rot = (dq * self.rot).normalize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_fall_matches_kinematics() {
        let mut body = RigidBody::new(Vec3::new(0.0, 10.0, 0.0), Quat::IDENTITY);
        let mass = 1800.0;
        let gravity = vec3(0.0, -mass * 9.81, 0.0);
        let dt = 1.0 / 240.0;
        for _ in 0..240 {
            body.integrate_force(gravity, mass, dt);
            body.integrate_pose(dt);
        }
        // Semi-implicit Euler lags the analytic ½gt² by ½g·dt·t.
        let t = 1.0;
        let expected = 10.0 - 0.5 * 9.81 * t * t - 0.5 * 9.81 * dt * t;
        assert!(
            (body.pos.y - expected).abs() < 1e-3,
            "y={} expected≈{expected}",
            body.pos.y
        );
        assert!((body.vel.y + 9.81).abs() < 1e-3);
    }

    #[test]
    fn off_centre_impulse_converts_linear_to_angular_momentum() {
        let mut body = RigidBody::default();
        let inertia = vec3(400.0, 700.0, 400.0);
        let r = vec3(0.0, 0.0, 1.5);
        let j = vec3(100.0, 0.0, 0.0);
        body.apply_impulse(j, r, 1000.0, inertia);
        assert!((body.vel.x - 0.1).abs() < 1e-6);
        // r × j = (0, 1.5·100, 0) → yaw about +Y.
        assert!((body.angular.y - 150.0 / 700.0).abs() < 1e-5);
        assert!(body.angular.x.abs() < 1e-6 && body.angular.z.abs() < 1e-6);
    }

    #[test]
    fn torque_rotates_into_the_chassis_frame() {
        let body = RigidBody::new(
            Vec3::ZERO,
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        );
        let inertia = vec3(100.0, 400.0, 200.0);
        // A world +X torque on a body yawed +90° is chassis-local +Z.
        let local = body.rot.inverse() * Vec3::X;
        assert!((local.z - 1.0).abs() < 1e-5, "local={local:?}");
        let inv = body.inv_inertia_world(Vec3::X, inertia);
        assert!((inv.length() - 1.0 / 200.0).abs() < 1e-5, "inv={inv:?}");
    }

    #[test]
    fn pose_integration_keeps_quaternion_normalised() {
        let mut body = RigidBody::default();
        body.angular = vec3(0.7, 1.3, -0.4);
        for _ in 0..10_000 {
            body.integrate_pose(1.0 / 240.0);
        }
        assert!((body.rot.length() - 1.0).abs() < 1e-4);
    }
}
