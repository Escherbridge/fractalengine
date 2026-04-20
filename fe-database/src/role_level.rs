use std::fmt;
use serde::{Deserialize, Serialize};

/// Hierarchical role levels ordered by privilege.
/// Owner > Manager > Editor > Viewer > None
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RoleLevel {
    Owner,
    Manager,
    Editor,
    Viewer,
    None,
}

impl fmt::Display for RoleLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            RoleLevel::Owner => "owner",
            RoleLevel::Manager => "manager",
            RoleLevel::Editor => "editor",
            RoleLevel::Viewer => "viewer",
            RoleLevel::None => "none",
        };
        write!(f, "{}", s)
    }
}

/// Numeric rank: higher = more privilege. Used for Ord impl.
impl RoleLevel {
    fn rank(self) -> u8 {
        match self {
            RoleLevel::Owner => 4,
            RoleLevel::Manager => 3,
            RoleLevel::Editor => 2,
            RoleLevel::Viewer => 1,
            RoleLevel::None => 0,
        }
    }

    /// Returns `true` for Owner and Manager.
    pub fn can_manage(&self) -> bool {
        matches!(self, RoleLevel::Owner | RoleLevel::Manager)
    }

    /// Returns `true` for Owner, Manager, and Editor.
    pub fn can_edit(&self) -> bool {
        matches!(self, RoleLevel::Owner | RoleLevel::Manager | RoleLevel::Editor)
    }

    /// Returns `true` for Owner, Manager, Editor, and Viewer.
    pub fn can_view(&self) -> bool {
        matches!(
            self,
            RoleLevel::Owner | RoleLevel::Manager | RoleLevel::Editor | RoleLevel::Viewer
        )
    }

    /// Returns `true` if `self` is at least as privileged as `minimum`.
    pub fn is_at_least(&self, minimum: RoleLevel) -> bool {
        self.rank() >= minimum.rank()
    }
}

impl PartialOrd for RoleLevel {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RoleLevel {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl From<&str> for RoleLevel {
    fn from(s: &str) -> Self {
        match s {
            "owner" => RoleLevel::Owner,
            "manager" => RoleLevel::Manager,
            "editor" => RoleLevel::Editor,
            "viewer" | "public" => RoleLevel::Viewer,
            _ => RoleLevel::None,
        }
    }
}

impl From<RoleLevel> for String {
    fn from(level: RoleLevel) -> Self {
        level.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_output() {
        assert_eq!(RoleLevel::Owner.to_string(), "owner");
        assert_eq!(RoleLevel::Manager.to_string(), "manager");
        assert_eq!(RoleLevel::Editor.to_string(), "editor");
        assert_eq!(RoleLevel::Viewer.to_string(), "viewer");
        assert_eq!(RoleLevel::None.to_string(), "none");
    }

    #[test]
    fn ordering() {
        assert!(RoleLevel::Owner > RoleLevel::Manager);
        assert!(RoleLevel::Manager > RoleLevel::Editor);
        assert!(RoleLevel::Editor > RoleLevel::Viewer);
        assert!(RoleLevel::Viewer > RoleLevel::None);
        assert!(RoleLevel::Owner > RoleLevel::None);
    }

    #[test]
    fn ordering_eq() {
        assert_eq!(RoleLevel::Owner, RoleLevel::Owner);
        assert!(RoleLevel::None < RoleLevel::Viewer);
    }

    #[test]
    fn from_str_known() {
        assert_eq!(RoleLevel::from("owner"), RoleLevel::Owner);
        assert_eq!(RoleLevel::from("manager"), RoleLevel::Manager);
        assert_eq!(RoleLevel::from("editor"), RoleLevel::Editor);
        assert_eq!(RoleLevel::from("viewer"), RoleLevel::Viewer);
        assert_eq!(RoleLevel::from("none"), RoleLevel::None);
    }

    #[test]
    fn from_str_public_maps_to_viewer() {
        assert_eq!(RoleLevel::from("public"), RoleLevel::Viewer);
    }

    #[test]
    fn from_str_unknown_maps_to_none() {
        assert_eq!(RoleLevel::from("admin"), RoleLevel::None);
        assert_eq!(RoleLevel::from(""), RoleLevel::None);
        assert_eq!(RoleLevel::from("OWNER"), RoleLevel::None);
        assert_eq!(RoleLevel::from("superuser"), RoleLevel::None);
    }

    #[test]
    fn can_manage() {
        assert!(RoleLevel::Owner.can_manage());
        assert!(RoleLevel::Manager.can_manage());
        assert!(!RoleLevel::Editor.can_manage());
        assert!(!RoleLevel::Viewer.can_manage());
        assert!(!RoleLevel::None.can_manage());
    }

    #[test]
    fn can_edit() {
        assert!(RoleLevel::Owner.can_edit());
        assert!(RoleLevel::Manager.can_edit());
        assert!(RoleLevel::Editor.can_edit());
        assert!(!RoleLevel::Viewer.can_edit());
        assert!(!RoleLevel::None.can_edit());
    }

    #[test]
    fn can_view() {
        assert!(RoleLevel::Owner.can_view());
        assert!(RoleLevel::Manager.can_view());
        assert!(RoleLevel::Editor.can_view());
        assert!(RoleLevel::Viewer.can_view());
        assert!(!RoleLevel::None.can_view());
    }

    #[test]
    fn is_at_least() {
        assert!(RoleLevel::Owner.is_at_least(RoleLevel::Owner));
        assert!(RoleLevel::Owner.is_at_least(RoleLevel::None));
        assert!(RoleLevel::Editor.is_at_least(RoleLevel::Viewer));
        assert!(RoleLevel::Editor.is_at_least(RoleLevel::Editor));
        assert!(!RoleLevel::Editor.is_at_least(RoleLevel::Manager));
        assert!(!RoleLevel::None.is_at_least(RoleLevel::Viewer));
        assert!(!RoleLevel::Viewer.is_at_least(RoleLevel::Editor));
    }

    #[test]
    fn serde_round_trip() {
        let levels = [
            RoleLevel::Owner,
            RoleLevel::Manager,
            RoleLevel::Editor,
            RoleLevel::Viewer,
            RoleLevel::None,
        ];
        for level in &levels {
            let json = serde_json::to_string(level).expect("serialize");
            let back: RoleLevel = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*level, back);
        }
    }

    #[test]
    fn into_string() {
        let s: String = RoleLevel::Owner.into();
        assert_eq!(s, "owner");
        let s: String = RoleLevel::Viewer.into();
        assert_eq!(s, "viewer");
    }
}
