//! Inspector Position/Rotation/Scale "Apply" action handling. See
//! `fe-ui/src/AGENTS.md` §inspector-transform.

use bevy::prelude::*;

use crate::node_manager::NodeManager;
use crate::plugin::InspectorFormState;

/// Parsed transform values ready to write onto the selected entity.
pub(crate) struct ParsedTransform {
    pub position: Vec3,
    /// Euler angles in radians (XYZ order), matching `Transform::rotation`'s
    /// `to_euler(EulerRot::XYZ)` round-trip used by `inspector_sync`.
    pub rotation_euler: Vec3,
    pub scale: Vec3,
}

/// Pure: parse the inspector's Position/Rotation/Scale text buffers.
/// Rotation buffers are in degrees (as displayed); returns `None` for any
/// axis that fails to parse, leaving the caller to fall back rather than
/// silently zero it out.
pub(crate) fn parse_inspector_transform(inspector: &InspectorFormState) -> Option<ParsedTransform> {
    fn parse_axes(bufs: &[String; 3]) -> Option<Vec3> {
        let x = bufs[0].trim().parse::<f32>().ok()?;
        let y = bufs[1].trim().parse::<f32>().ok()?;
        let z = bufs[2].trim().parse::<f32>().ok()?;
        Some(Vec3::new(x, y, z))
    }

    let position = parse_axes(&inspector.pos)?;
    let rot_deg = parse_axes(&inspector.rot)?;
    let scale = parse_axes(&inspector.scale)?;

    Some(ParsedTransform {
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
/// DB + P2P in the same frame's `UiSet::Selection` pass.
pub(crate) fn apply(
    inspector: &InspectorFormState,
    node_mgr: &mut NodeManager,
    transform_query: &mut Query<&mut Transform>,
) {
    let Some(sel) = node_mgr.selected.as_mut() else { return };
    let Some(parsed) = parse_inspector_transform(inspector) else {
        bevy::log::warn!("ApplyNodeTransform: could not parse one or more transform fields");
        return;
    };
    let Ok(mut transform) = transform_query.get_mut(sel.entity) else { return };

    transform.translation = parsed.position;
    transform.rotation = Quat::from_euler(
        EulerRot::XYZ,
        parsed.rotation_euler.x,
        parsed.rotation_euler.y,
        parsed.rotation_euler.z,
    );
    transform.scale = parsed.scale;

    sel.drag_committed = true;
}

#[cfg(test)]
mod tests {
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
        assert!(parse_inspector_transform(&inspector).is_none());
    }

    #[test]
    fn parse_inspector_transform_trims_whitespace() {
        let mut inspector = InspectorFormState::default();
        inspector.pos = [" 1.0 ".into(), "0".into(), "0".into()];
        let parsed = parse_inspector_transform(&inspector).expect("should parse");
        assert_eq!(parsed.position.x, 1.0);
    }
}
