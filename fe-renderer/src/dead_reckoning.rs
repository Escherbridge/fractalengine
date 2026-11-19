use bevy::math::Vec3;
use bevy::prelude::{Commands, Component, Entity, Quat, Query, Res, Time, Transform};

/// Minimum allowed smoothing window in seconds (roughly one 60fps frame).
const MIN_SMOOTH_SECS: f32 = 0.016;
/// Maximum allowed smoothing window in seconds to avoid unbounded lag.
const MAX_SMOOTH_SECS: f32 = 5.0;
/// Translation delta above which updates snap immediately instead of sliding.
const TELEPORT_SNAP_DISTANCE: f32 = 500.0;

#[derive(Component)]
pub struct DeadReckoningPredictor {
    pub velocity: Vec3,
    pub last_network_pos: Vec3,
    pub last_network_time: f64,
    pub threshold: f32,
}

impl Default for DeadReckoningPredictor {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            last_network_pos: Vec3::ZERO,
            last_network_time: 0.0,
            threshold: 0.1,
        }
    }
}

pub fn dead_reckoning_system(
    mut query: Query<(&mut Transform, &mut DeadReckoningPredictor)>,
    time: Res<Time>,
) {
    for (mut transform, predictor) in query.iter_mut() {
        let dt = time.elapsed_secs_f64() - predictor.last_network_time;
        let predicted = predictor.last_network_pos + predictor.velocity * dt as f32;
        transform.translation = predicted;
        // CROSS-CRATE: NetworkCommand::EntityUpdate — deferred to Sprint 5B when threshold crossed
    }
}

/// Target transform component for frame-rate-independent 60fps smooth interpolation
/// between committed scene updates.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct SmoothTransformTarget {
    /// Starting translation when this transition began.
    pub start_translation: Vec3,
    /// Starting rotation when this transition began.
    pub start_rotation: Quat,
    /// Starting scale when this transition began.
    pub start_scale: Vec3,
    /// Committed target translation.
    pub target_translation: Vec3,
    /// Committed target rotation.
    pub target_rotation: Quat,
    /// Committed target scale.
    pub target_scale: Vec3,
    /// Optional instantaneous velocity hint (m/s) from telemetry metadata.
    pub velocity: Vec3,
    /// Interpolation duration in seconds.
    pub duration_secs: f32,
    /// Elapsed seconds within this transition.
    pub elapsed_secs: f32,
}

impl SmoothTransformTarget {
    /// Builds a smooth transition between `current` and `target` when `smoothing_ms`
    /// is positive (or a non-zero velocity hint defaults the window to `200 ms`).
    ///
    /// Returns `None` when no smoothing is requested or the delta exceeds
    /// [`TELEPORT_SNAP_DISTANCE`].
    pub fn from_transition(
        current: &Transform,
        target: &Transform,
        velocity: Vec3,
        smoothing_ms: Option<f32>,
    ) -> Option<Self> {
        let raw_secs = match smoothing_ms {
            Some(ms) if ms.is_finite() && ms > 0.0 => ms / 1000.0,
            None if velocity.is_finite() && velocity.length_squared() > 1e-6 => 0.2,
            _ => return None,
        };
        if !current.translation.is_finite()
            || !target.translation.is_finite()
            || current.translation.distance(target.translation) > TELEPORT_SNAP_DISTANCE
        {
            return None;
        }
        Some(Self {
            start_translation: current.translation,
            start_rotation: current.rotation,
            start_scale: current.scale,
            target_translation: target.translation,
            target_rotation: target.rotation,
            target_scale: target.scale,
            velocity: if velocity.is_finite() {
                velocity
            } else {
                Vec3::ZERO
            },
            duration_secs: raw_secs.clamp(MIN_SMOOTH_SECS, MAX_SMOOTH_SECS),
            elapsed_secs: 0.0,
        })
    }

    /// Advances the transition by `dt_secs`, returning the interpolated transform and
    /// `true` if the transition has completed and settled on its exact target.
    pub fn advance(&mut self, dt_secs: f32) -> (Transform, bool) {
        if dt_secs.is_finite() && dt_secs > 0.0 {
            self.elapsed_secs += dt_secs;
        }
        let t = (self.elapsed_secs / self.duration_secs).clamp(0.0, 1.0);
        let transform = Transform {
            translation: self.start_translation.lerp(self.target_translation, t),
            rotation: self.start_rotation.slerp(self.target_rotation, t),
            scale: self.start_scale.lerp(self.target_scale, t),
        };
        let done = t >= 1.0;
        (transform, done)
    }
}

/// Interpolates entity `Transform`s toward their [`SmoothTransformTarget`] every frame
/// and removes the target component once settled.
pub fn smooth_transform_system(
    mut commands: Commands,
    time: Res<Time>,
    mut query: Query<(Entity, &mut Transform, &mut SmoothTransformTarget)>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut target) in query.iter_mut() {
        let (next, done) = target.advance(dt);
        *transform = next;
        if done {
            commands.entity(entity).remove::<SmoothTransformTarget>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_target_interpolates_and_settles_at_exact_target() {
        let current = Transform::from_xyz(0.0, 0.0, 0.0);
        let goal = Transform {
            translation: Vec3::new(10.0, 0.0, 0.0),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            scale: Vec3::splat(2.0),
        };
        let mut smooth = SmoothTransformTarget::from_transition(
            &current,
            &goal,
            Vec3::new(10.0, 0.0, 0.0),
            Some(200.0),
        )
        .expect("smooth transition created");

        let (mid, done_mid) = smooth.advance(0.1);
        assert!(!done_mid);
        assert!((mid.translation.x - 5.0).abs() < 1e-4);
        assert!((mid.scale.x - 1.5).abs() < 1e-4);

        let (final_tx, done_end) = smooth.advance(0.15);
        assert!(done_end);
        assert!(final_tx.translation.distance(goal.translation) < 1e-4);
        assert!(final_tx.scale.distance(goal.scale) < 1e-4);
    }

    #[test]
    fn zero_smoothing_or_large_teleport_returns_none() {
        let a = Transform::from_xyz(0.0, 0.0, 0.0);
        let b = Transform::from_xyz(1.0, 0.0, 0.0);
        assert!(SmoothTransformTarget::from_transition(&a, &b, Vec3::ZERO, Some(0.0)).is_none());
        assert!(SmoothTransformTarget::from_transition(&a, &b, Vec3::ZERO, None).is_none());

        let far = Transform::from_xyz(900.0, 0.0, 0.0);
        assert!(SmoothTransformTarget::from_transition(&a, &far, Vec3::ZERO, Some(200.0)).is_none());
    }
}
