use std::sync::Arc;

use super::{adoption::Preview, context};
use crate::{
    accessibility::ResponseExt,
    actions::{UiAction, UiManager},
    navigation_manager::NavigationManager,
    node_manager::NodeManager,
    plugin::LocalUserRole,
    verse_manager::VerseManager,
};
use bevy_egui::egui;

#[derive(Clone, Default)]
pub(crate) struct PanelView {
    pub(super) node_id: Option<String>,
    pub(super) preview: Option<Arc<Preview>>,
    pub(super) status: Option<String>,
    pub(super) busy: bool,
    pub(super) ready: bool,
}

impl PanelView {
    pub(super) fn matches(
        &self,
        node: Option<&str>,
        preview: Option<&Arc<Preview>>,
        status: Option<&str>,
        busy: bool,
        ready: bool,
    ) -> bool {
        self.node_id.as_deref() == node
            && self.status.as_deref() == status
            && self.busy == busy
            && self.ready == ready
            && match (&self.preview, preview) {
                (None, None) => true,
                (Some(before), Some(after)) => Arc::ptr_eq(before, after),
                _ => false,
            }
    }
}

pub(crate) fn render(
    ui: &mut egui::Ui,
    nodes: &NodeManager,
    manager: &VerseManager,
    nav: &NavigationManager,
    role: &LocalUserRole,
    state: &mut UiManager,
) {
    let Some(selected) = nodes.primary().filter(|_| nodes.selection_count() == 1) else {
        return;
    };
    let node = selected.node_id.as_str();
    let source = nav
        .active_petal_id
        .as_deref()
        .and_then(|petal| manager.scene_asset_source(node, petal));
    let tree_asset = nav
        .active_petal_id
        .as_deref()
        .and_then(|petal| manager.find_petal(petal))
        .is_some_and(|petal| {
            petal
                .nodes
                .iter()
                .any(|entry| entry.id == node && entry.has_asset)
        });
    if !tree_asset
        && source
            .as_ref()
            .is_none_or(|source| source.asset.asset_id().is_none())
    {
        return;
    }
    if manager.scene_access_blocked() || !role.role.is_some_and(|role| role.can_view()) {
        ui.label("Scene asset access is unavailable while current permissions are resolved");
        return;
    }
    let mut action = None;
    ui.group(|ui| {
        ui.strong("Scene asset sharing");
        if let Some(source) = &source {
            match source.asset {
                fe_runtime::scene_replication::ReplicaNodeAsset::LocalBlob { content_hash, .. } => { ui.label("Verified local bytes are available"); ui.monospace(format!("BLAKE3 {content_hash}")); }
                fe_runtime::scene_replication::ReplicaNodeAsset::Unavailable { reason, .. } => { ui.label(unavailable(reason)); }
                fe_runtime::scene_replication::ReplicaNodeAsset::NoAsset => { ui.label("This object has no scene asset"); }
            }
        } else { ui.label("Current asset details need a refresh; review requests verified details"); }
        let view = &state.scene_asset_view;
        let view_matches = view.node_id.as_deref() == Some(node);
        if let Some(status) = view.status.as_deref().filter(|_| view_matches) { ui.label(status); }
        let can_review = context::owner(role) && !manager.scene_access_blocked() && !(view_matches && view.busy);
        if ui.add_enabled(can_review, egui::Button::new("Review local copy for sharing")).named(ui, "Review local scene asset copy").clicked() {
            action = Some(UiAction::ReviewSceneAssetCopy { node_id: node.into() });
        }
        if !context::owner(role) { ui.label("An Owner can review and adopt a verified local copy."); }
        if let Some(preview) = view.preview.as_ref().filter(|_| view_matches) {
            ui.separator();
            ui.label("Record responsibility for this verified local copy and make it eligible for sharing.");
            ui.monospace(format!("Asset {}", preview.candidate.asset_id));
            ui.monospace(format!("BLAKE3 {}", fe_runtime::blob_store::hash_to_hex(&preview.candidate.content_hash)));
            ui.label(format!("{} / {} bytes", preview.candidate.content_type, preview.candidate.size_bytes));
            ui.label(format!("{} object reference(s) in this Petal", preview.candidate.reference_fingerprints.len()));
            egui::ScrollArea::vertical().id_salt("scene-asset-reviewed-references").max_height(120.0).show_rows(ui, ui.text_style_height(&egui::TextStyle::Monospace), preview.candidate.reference_fingerprints.len(), |ui, rows| {
                for row in rows { ui.monospace(&preview.candidate.reference_fingerprints[row].node_id); }
            });
            if preview.candidate.model_fingerprint.is_some() { ui.label("The reviewed reference set also includes local Portal metadata."); }
            let current = preview.matches_context(manager, nav, nodes, role);
            if ui.add_enabled(view.ready && !view.busy && current, egui::Button::new("Adopt this local copy")).named(ui, "Adopt verified local scene asset copy").clicked() {
                action = Some(UiAction::AdoptSceneAssetCopy { preview_id: preview.id.clone() });
            }
            if ui.button("Cancel local-copy review").named(ui, "Cancel local scene asset review").clicked() { action = Some(UiAction::CancelSceneAssetReview); }
        }
    });
    if let Some(action) = action {
        state.push_action(action);
    }
}

fn unavailable(reason: &fe_runtime::scene_replication::ReplicaAssetUnavailable) -> &'static str {
    use fe_runtime::scene_replication::ReplicaAssetUnavailable::*;
    match reason {
        MissingMetadata => "The object's asset details have not arrived yet",
        AmbiguousMetadata => "Conflicting asset identities prevent this model from loading",
        ForeignAnchor => "The asset is associated with a different scene",
        UnanchoredMetadata => {
            "A verified association between this asset and its scene is unavailable"
        }
        InvalidMetadata => "Asset details failed verification",
        UnsupportedSceneAsset => "This asset format cannot be loaded as a scene",
        MissingBlob => "The model's verified bytes are not available locally yet",
        TransferLimitExceeded => "The model exceeds the peer download size limit",
    }
}
