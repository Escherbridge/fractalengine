/// State resource for the room metadata editor panel.
#[derive(Debug, Clone, Default)]
pub struct RoomEditorState {
    pub room_id: String,
    pub description: String,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub spawn_x: f32,
    pub spawn_y: f32,
    pub spawn_z: f32,
    pub yaw: f32,
    pub dirty: bool,
}

impl RoomEditorState {
    /// Returns `true` if bounds are geometrically valid (min < max on all axes).
    pub fn bounds_valid(&self) -> bool {
        self.min[0] < self.max[0] && self.min[1] < self.max[1] && self.min[2] < self.max[2]
    }

    /// Set yaw, clamping to [-180, 180].
    pub fn set_yaw(&mut self, degrees: f32) {
        // Normalise to (-360, 360) first, then shift into [-180, 180].
        let mut y = degrees % 360.0;
        if y > 180.0 {
            y -= 360.0;
        } else if y < -180.0 {
            y += 360.0;
        }
        self.yaw = y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_invalid_when_min_gte_max_on_any_axis() {
        let state = RoomEditorState {
            min: [0.0, 5.0, 0.0],
            max: [10.0, 3.0, 10.0], // y: min(5) > max(3)
            ..Default::default()
        };
        assert!(!state.bounds_valid());
    }

    #[test]
    fn bounds_valid_when_all_min_lt_max() {
        let state = RoomEditorState {
            min: [-1.0, 0.0, -1.0],
            max: [1.0, 2.0, 1.0],
            ..Default::default()
        };
        assert!(state.bounds_valid());
    }

    #[test]
    fn yaw_clamped_on_set() {
        let mut state = RoomEditorState::default();
        state.set_yaw(270.0);
        assert!(state.yaw >= -180.0 && state.yaw <= 180.0);
    }
}
