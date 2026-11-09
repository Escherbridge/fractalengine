//! `portal.open` binding offer (bindings.md §6, D-X5): a bound, admitted bundle is offered with one Open action.
//! A replicated binding row never mounts anything; only the user's Open does. See `fe-ui/src/AGENTS.md` §portal.

use crate::actions::UiAction;

/// How the winning `portal.open` binding is served on this peer (mirror of `BindingResolution`'s states).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfferResolution {
    /// An admitted web bundle serves the point here.
    Resolved,
    /// The built-in (node `webpage_url` portal) serves it.
    Builtin,
    /// Placement names another peer.
    Elsewhere,
    /// Bound, but not runnable here; `reason` is the host's short phrase.
    Unresolved { reason: String },
}

/// One effective `portal.open` point at the active petal, as the binding host reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalBindingPoint {
    /// Scope string the winning row sits at.
    pub scope: String,
    pub package_id: Option<String>,
    pub resolution: OfferResolution,
}

/// What the inspector shows for a bound bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalOffer {
    /// Admitted and ready: one Open action for the repository node.
    Open { package_id: String, scope: String, node_id: String },
    /// Bound but not runnable here: shown as state only, no Open.
    Unavailable { package_id: String, reason: String },
}

impl PortalOffer {
    /// The action behind the Open button; the only path that mounts a bound bundle.
    pub fn open_action(&self) -> Option<UiAction> {
        match self {
            Self::Open { node_id, .. } => Some(UiAction::OpenBundle { node_id: node_id.clone() }),
            Self::Unavailable { .. } => None,
        }
    }

    /// Button or state text.
    pub fn label(&self) -> String {
        match self {
            Self::Open { package_id, .. } => format!("Open {package_id}"),
            Self::Unavailable { package_id, reason } => format!("{package_id} unavailable: {reason}"),
        }
    }
}

/// Pure: the offer for a petal's effective `portal.open` point. `node_for` maps a package id to the
/// repository node holding its admitted bundle; no node means no Open. Built-in and elsewhere offer nothing.
pub fn portal_offer(
    point: Option<&PortalBindingPoint>,
    node_for: impl Fn(&str) -> Option<String>,
) -> Option<PortalOffer> {
    let point = point?;
    let package_id = point.package_id.clone()?;
    match &point.resolution {
        OfferResolution::Resolved => match node_for(&package_id) {
            Some(node_id) => Some(PortalOffer::Open { package_id, scope: point.scope.clone(), node_id }),
            None => Some(PortalOffer::Unavailable { package_id, reason: "bundle not on this peer".into() }),
        },
        OfferResolution::Unresolved { reason } => {
            Some(PortalOffer::Unavailable { package_id, reason: reason.clone() })
        }
        OfferResolution::Builtin | OfferResolution::Elsewhere => None,
    }
}

/// Inspector section: the offer's Open button or its state line; returns the clicked action.
pub fn show_offer(ui: &mut bevy_egui::egui::Ui, offer: &PortalOffer) -> Option<UiAction> {
    match offer.open_action() {
        Some(action) => ui.button(offer.label()).clicked().then_some(action),
        None => {
            ui.weak(offer.label());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(resolution: OfferResolution) -> PortalBindingPoint {
        PortalBindingPoint { scope: "VERSE#v-FRACTAL#f-PETAL#p".into(), package_id: Some("org.example.map".into()), resolution }
    }

    #[test]
    fn offer_states_map_to_open_or_nothing() {
        let node = |id: &str| (id == "org.example.map").then(|| "node-1".to_string());
        let open = portal_offer(Some(&point(OfferResolution::Resolved)), node).unwrap();
        assert!(matches!(open.open_action(), Some(UiAction::OpenBundle { node_id }) if node_id == "node-1"));
        let blocked = portal_offer(Some(&point(OfferResolution::Unresolved { reason: "not admitted".into() })), node).unwrap();
        assert!(blocked.open_action().is_none());
        assert!(portal_offer(Some(&point(OfferResolution::Builtin)), node).is_none());
        assert!(portal_offer(Some(&point(OfferResolution::Elsewhere)), node).is_none());
        assert!(portal_offer(Some(&point(OfferResolution::Resolved)), |_| None).unwrap().open_action().is_none());
    }
}
