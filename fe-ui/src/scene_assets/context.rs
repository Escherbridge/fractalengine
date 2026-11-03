use crate::{
    navigation_manager::NavigationManager,
    node_manager::NodeManager,
    plugin::LocalUserRole,
    verse_manager::{SceneAdmission, VerseManager},
};

#[derive(Clone, Debug)]
pub(super) struct Context {
    pub node_id: String,
    pub asset_id: String,
    pub petal_id: String,
    pub scope: String,
    pub verse_id: String,
    fractal_id: Option<String>,
    navigation: u64,
    selection: u64,
    admission: SceneAdmission,
    access_generation: Option<u128>,
}

pub(super) fn owner(role: &LocalUserRole) -> bool {
    role.role == Some(fe_database::RoleLevel::Owner)
}

impl Context {
    pub fn capture(
        node_id: &str,
        manager: &VerseManager,
        nav: &NavigationManager,
        nodes: &NodeManager,
        role: &LocalUserRole,
    ) -> Result<Self, String> {
        if !owner(role) || manager.scene_access_blocked() {
            return Err("Current Owner access is required to review this local copy".into());
        }
        if nodes.selection_count() != 1
            || nodes.primary().is_none_or(|node| node.node_id != node_id)
        {
            return Err("Select exactly the object whose local copy you want to review".into());
        }
        let petal_id = nav
            .active_petal_id
            .as_deref()
            .ok_or("Open the object's Petal")?;
        let source = manager
            .scene_asset_source(node_id, petal_id)
            .ok_or("Asset details are refreshing; review the local copy again after they load")?;
        if nav.active_scope().as_deref() != Some(source.scope) {
            return Err(
                "The selected object's stored scope differs from navigation; reopen its Petal"
                    .into(),
            );
        }
        let asset_id = source
            .asset
            .asset_id()
            .ok_or("This object has no scene asset")?;
        Ok(Self {
            node_id: node_id.into(),
            asset_id: asset_id.into(),
            petal_id: petal_id.into(),
            scope: source.scope.into(),
            verse_id: nav
                .active_verse_id
                .clone()
                .ok_or("Open the object's Verse")?,
            navigation: nav.intent_revision,
            fractal_id: nav.active_fractal_id.clone(),
            selection: nodes.generation(),
            admission: source.admission,
            access_generation: manager.scene_access_generation(),
        })
    }

    pub fn matches(
        &self,
        manager: &VerseManager,
        nav: &NavigationManager,
        nodes: &NodeManager,
        role: &LocalUserRole,
    ) -> bool {
        owner(role)
            && !manager.scene_access_blocked()
            && nav.intent_revision == self.navigation
            && nav.active_verse_id.as_deref() == Some(self.verse_id.as_str())
            && nav.active_fractal_id == self.fractal_id
            && nav.active_petal_id.as_deref() == Some(self.petal_id.as_str())
            && nodes.generation() == self.selection
            && nodes.selection_count() == 1
            && nodes
                .primary()
                .is_some_and(|node| node.node_id == self.node_id)
            && manager.scene_access_generation() == self.access_generation
            && manager.is_current_scene_admission(&self.node_id, &self.petal_id, self.admission)
            && manager.scene_asset_id(&self.node_id, &self.petal_id) == Some(self.asset_id.as_str())
    }

    pub fn can_refresh(&self, manager: &VerseManager, nav: &NavigationManager) -> bool {
        !manager.scene_access_blocked()
            && nav.active_verse_id.as_deref() == Some(self.verse_id.as_str())
            && nav.active_fractal_id == self.fractal_id
            && nav.active_petal_id.as_deref() == Some(self.petal_id.as_str())
            && manager.petal_id_of(&self.node_id) == Some(self.petal_id.as_str())
            && manager.scene_asset_id(&self.node_id, &self.petal_id) == Some(self.asset_id.as_str())
    }
}
