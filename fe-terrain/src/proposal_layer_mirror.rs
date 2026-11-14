//! Mirrors the ProposalOverlay layer's visibility into a plain-data fe-renderer resource for fe-ui.

use crate::layers::{LayerStack, LayerType};
use bevy::ecs::change_detection::DetectChangesMut;
use bevy::prelude::*;
use fe_renderer::terrain_overlay::ProposalLayerVisible;

/// Visible unless a ProposalOverlay layer exists and every such layer is hidden.
fn proposal_layer_visible(layer_stack: &LayerStack) -> bool {
    let mut any_layer = false;
    for layer in layer_stack.iter() {
        if matches!(layer.layer_type, LayerType::ProposalOverlay) {
            any_layer = true;
            if layer.visible {
                return true;
            }
        }
    }
    !any_layer
}

/// Copy the layer's visibility into [`ProposalLayerVisible`], writing only on change.
pub fn mirror_proposal_layer_visibility(
    layer_stack: Res<LayerStack>,
    mut mirror: ResMut<ProposalLayerVisible>,
) {
    if !layer_stack.is_changed() {
        return;
    }
    let visible = proposal_layer_visible(&layer_stack);
    if mirror.bypass_change_detection().visible != visible {
        mirror.visible = visible;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::MapLayer;

    #[test]
    fn mirror_follows_proposal_layer_visibility() {
        let mut app = App::new();
        app.insert_resource(LayerStack::new())
            .init_resource::<ProposalLayerVisible>()
            .add_systems(Update, mirror_proposal_layer_visibility);

        app.update();
        assert!(app.world().resource::<ProposalLayerVisible>().visible, "no layer => shown");

        let id = app
            .world_mut()
            .resource_mut::<LayerStack>()
            .add_layer(MapLayer::new(LayerType::ProposalOverlay, 0));
        let rows = [(true, true), (false, false), (true, true)];
        for (layer_visible, expected) in rows {
            app.world_mut()
                .resource_mut::<LayerStack>()
                .set_visibility(id, layer_visible);
            app.update();
            assert_eq!(
                app.world().resource::<ProposalLayerVisible>().visible,
                expected,
                "layer visible={layer_visible}"
            );
        }
    }
}
