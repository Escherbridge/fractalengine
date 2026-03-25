use fe_database::atlas::SpaceOverview; // CROSS-CRATE: verify this type exists after merge

/// Bevy resource for the space overview dashboard state.
#[derive(Debug, Clone, Default, bevy::prelude::Resource)]
pub struct DashboardState {
    pub petal_count: u64,
    pub room_count: u64,
    pub model_count: u64,
    pub peer_count: u64,
    pub is_online: bool,
}

impl DashboardState {
    /// Apply a successful space overview to this state.
    pub fn apply_overview(&mut self, overview: SpaceOverview) {
        self.petal_count = overview.petal_count;
        self.room_count = overview.room_count;
        self.model_count = overview.model_count;
        self.peer_count = overview.peer_count;
        self.is_online = true;
    }

    /// Mark the dashboard as offline (e.g., on DB error).
    pub fn apply_error(&mut self) {
        self.is_online = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_state_default_shows_all_zeroes() {
        let state = DashboardState::default();
        assert_eq!(state.petal_count, 0);
        assert!(!state.is_online);
    }

    #[test]
    fn dashboard_state_marks_offline_on_error() {
        let mut state = DashboardState::default();
        state.apply_error();
        assert!(!state.is_online);
    }

    #[test]
    fn dashboard_state_updates_counts_on_success() {
        let mut state = DashboardState::default();
        let overview = SpaceOverview {
            petal_count: 3,
            room_count: 7,
            model_count: 12,
            peer_count: 2,
            estimated_storage_bytes: 0,
        };
        state.apply_overview(overview);
        assert!(state.is_online);
        assert_eq!(state.petal_count, 3);
    }
}
