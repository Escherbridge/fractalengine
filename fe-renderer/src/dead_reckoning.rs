use bevy::math::Vec3;
use bevy::prelude::{Component, Query, Res, Time, Transform};

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
