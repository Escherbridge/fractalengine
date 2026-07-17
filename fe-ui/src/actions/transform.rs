//! Inspector Position/Rotation/Scale "Apply" action handling. See
//! `fe-ui/src/AGENTS.md` §inspector-transform.

use bevy::prelude::*;

use crate::node_manager::NodeManager;
use crate::plugin::InspectorFormState;

/// Parsed transform values ready to write onto the selected entity.
#[derive(Debug)]
pub(crate) struct ParsedTransform {
    pub position: Vec3,
    /// Euler angles in radians (XYZ order), matching `Transform::rotation`'s
    /// `to_euler(EulerRot::XYZ)` round-trip used by `inspector_sync`.
    pub rotation_euler: Vec3,
    pub scale: Vec3,
}

/// Pure: parse the inspector's Position/Rotation/Scale text buffers.
/// Rotation buffers are in degrees (as displayed); returns `Err` naming the
/// first axis that fails to parse, leaving the caller to fall back (and tell
/// the user) rather than silently zero it out.
pub(crate) fn parse_inspector_transform(
    inspector: &InspectorFormState,
) -> Result<ParsedTransform, String> {
    fn parse_axes(label: &str, bufs: &[String; 3]) -> Result<Vec3, String> {
        const AXES: [&str; 3] = ["X", "Y", "Z"];
        let mut vals = [0.0_f32; 3];
        for (i, buf) in bufs.iter().enumerate() {
            vals[i] = buf
                .trim()
                .parse::<f32>()
                .map_err(|_| format!("{} {label} isn't a number", AXES[i]))?;
        }
        Ok(Vec3::from_array(vals))
    }

    let position = parse_axes("position", &inspector.pos)?;
    let rot_deg = parse_axes("rotation", &inspector.rot)?;
    let scale = parse_axes("scale", &inspector.scale)?;

    Ok(ParsedTransform {
        position,
        rotation_euler: Vec3::new(
            rot_deg.x.to_radians(),
            rot_deg.y.to_radians(),
            rot_deg.z.to_radians(),
        ),
        scale,
    })
}

/// `UiAction::ApplyNodeTransform` handler: parses the inspector buffers and
/// writes them onto the selected entity's `Transform`, then marks the drag
/// as committed so `transform_broadcast::broadcast_transform` persists it to
/// DB + P2P in the same frame's `UiSet::Selection` pass. `Err` carries a
/// user-facing parse-failure reason for the dispatcher's toast.
pub(crate) fn apply(
    inspector: &InspectorFormState,
    node_mgr: &mut NodeManager,
    transform_query: &mut Query<&mut Transform>,
) -> Result<(), String> {
    let Some(sel) = node_mgr.selected.as_mut() else {
        return Ok(());
    };
    let parsed = match parse_inspector_transform(inspector) {
        Ok(parsed) => parsed,
        Err(reason) => {
            bevy::log::warn!("ApplyNodeTransform: {reason}");
            return Err(reason);
        }
    };
    let Ok(mut transform) = transform_query.get_mut(sel.entity) else {
        return Ok(());
    };

    transform.translation = parsed.position;
    transform.rotation = Quat::from_euler(
        EulerRot::XYZ,
        parsed.rotation_euler.x,
        parsed.rotation_euler.y,
        parsed.rotation_euler.z,
    );
    transform.scale = parsed.scale;

    sel.drag_committed = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)] // default-then-set is clearer in test fixtures
    use super::*;

    #[test]
    fn parse_inspector_transform_valid_buffers() {
        let mut inspector = InspectorFormState::default();
        inspector.pos = ["1.5".into(), "-2".into(), "0".into()];
        inspector.rot = ["90".into(), "0".into(), "0".into()];
        inspector.scale = ["2".into(), "2".into(), "2".into()];

        let parsed = parse_inspector_transform(&inspector).expect("should parse");
        assert_eq!(parsed.position, Vec3::new(1.5, -2.0, 0.0));
        assert!((parsed.rotation_euler.x - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert_eq!(parsed.scale, Vec3::new(2.0, 2.0, 2.0));
    }

    #[test]
    fn parse_inspector_transform_rejects_invalid_number() {
        let mut inspector = InspectorFormState::default();
        inspector.pos = ["not-a-number".into(), "0".into(), "0".into()];
        let err = parse_inspector_transform(&inspector).expect_err("should fail");
        assert_eq!(err, "X position isn't a number");
    }

    #[test]
    fn parse_inspector_transform_trims_whitespace() {
        let mut inspector = InspectorFormState::default();
        inspector.pos = [" 1.0 ".into(), "0".into(), "0".into()];
        let parsed = parse_inspector_transform(&inspector).expect("should parse");
        assert_eq!(parsed.position.x, 1.0);
    }
}
