use super::*;

fn fixture() -> ak::TreeUpdate {
    let mut root = ak::Node::new(ak::Role::Window);
    root.set_children(vec![ak::NodeId(1), ak::NodeId(2), ak::NodeId(3)]);
    let mut target = ak::Node::new(ak::Role::Button);
    target.add_action(ak::Action::Click);
    target.set_label("Open petal fixture");
    target.set_bounds(ak::Rect::new(1.0, 2.0, 40.0, 24.0));
    let mut related = ak::Node::new(ak::Role::TextInput);
    related.set_label("Fixed relationship fixture");
    related.set_description("Preserved description");
    related.set_value("Preserved fixture value");
    related.set_controls(vec![ak::NodeId(1), ak::NodeId(3)]);
    related.set_details(vec![ak::NodeId(1)]);
    related.set_described_by(vec![ak::NodeId(1)]);
    related.set_flow_to(vec![ak::NodeId(1)]);
    related.set_labelled_by(vec![ak::NodeId(1)]);
    related.set_owns(vec![ak::NodeId(1)]);
    related.set_radio_group(vec![ak::NodeId(1)]);
    related.set_active_descendant(ak::NodeId(1));
    related.set_error_message(ak::NodeId(1));
    related.set_in_page_link_target(ak::NodeId(1));
    related.set_member_of(ak::NodeId(1));
    related.set_next_on_line(ak::NodeId(1));
    related.set_previous_on_line(ak::NodeId(1));
    related.set_popup_for(ak::NodeId(1));
    related.set_text_selection(ak::TextSelection {
        anchor: ak::TextPosition { node: ak::NodeId(1), character_index: 5 },
        focus: ak::TextPosition { node: ak::NodeId(3), character_index: 9 },
    });
    ak::TreeUpdate {
        nodes: vec![
            (ak::NodeId(100), root),
            (ak::NodeId(1), target),
            (ak::NodeId(2), related),
            (ak::NodeId(3), ak::Node::new(ak::Role::TextRun)),
        ],
        tree: Some(ak::Tree::new(ak::NodeId(100))),
        focus: ak::NodeId(1),
    }
}

pub(super) fn run() {
    let original = fixture();
    let targets = [TargetIdentity { physical: ak::NodeId(1), epoch: 17 }];
    let mut ledger = Ledger::new();
    assert!(full_tree_is_safe(&original));
    let (mapped, aliases) = remap_with_ledger(&original, 19, &targets, &mut ledger).unwrap();
    let external = aliases[0].provider;
    assert_ne!(external, targets[0].physical);
    assert_eq!(aliases.len(), 1);
    assert_eq!(mapped.nodes.len(), original.nodes.len());
    assert_eq!(mapped.tree.as_ref().unwrap().root, ak::NodeId(100));
    assert_eq!(mapped.focus, external);
    assert_eq!(mapped.nodes[0].1.children(), &[external, ak::NodeId(2), ak::NodeId(3)]);
    assert_eq!(mapped.nodes[1].0, external);
    assert_eq!(mapped.nodes[1].1, original.nodes[1].1);
    assert_eq!(original.nodes[1].0, ak::NodeId(1));
    assert_eq!(original.focus, ak::NodeId(1));
    assert_eq!(original.nodes[0].1.children()[0], ak::NodeId(1));
    let related = &mapped.nodes[2].1;
    assert_eq!(related.controls(), &[external, ak::NodeId(3)]);
    assert_eq!(related.details(), &[external]);
    assert_eq!(related.described_by(), &[external]);
    assert_eq!(related.flow_to(), &[external]);
    assert_eq!(related.labelled_by(), &[external]);
    assert_eq!(related.owns(), &[external]);
    assert_eq!(related.radio_group(), &[external]);
    assert_eq!(related.active_descendant(), Some(external));
    assert_eq!(related.error_message(), Some(external));
    assert_eq!(related.in_page_link_target(), Some(external));
    assert_eq!(related.member_of(), Some(external));
    assert_eq!(related.next_on_line(), Some(external));
    assert_eq!(related.previous_on_line(), Some(external));
    assert_eq!(related.popup_for(), Some(external));
    let selection = related.text_selection().unwrap();
    assert_eq!(selection.anchor.node, external);
    assert_eq!(selection.anchor.character_index, 5);
    assert_eq!(selection.focus.node, ak::NodeId(3));
    assert_eq!(selection.focus.character_index, 9);
    assert_eq!(related.label(), original.nodes[2].1.label());
    assert_eq!(related.description(), original.nodes[2].1.description());
    assert_eq!(related.value(), original.nodes[2].1.value());
    assert!(full_tree_is_safe(&mapped));
    let (_, same) = remap_with_ledger(&original, 19, &targets, &mut ledger).unwrap();
    assert_eq!(same, aliases);
    assert_eq!(ledger.issued.len(), 1);

    let renewed = [TargetIdentity { physical: ak::NodeId(1), epoch: 18 }];
    let (_, renewed_aliases) = remap_with_ledger(&original, 19, &renewed, &mut ledger).unwrap();
    assert_ne!(renewed_aliases[0].provider, external);
    let (_, replaced_set) = remap_with_ledger(&original, 20, &renewed, &mut ledger).unwrap();
    assert_ne!(replaced_set[0].provider, renewed_aliases[0].provider);
    assert_eq!(ledger.issued.len(), 3);
    assert!(ledger.issued.iter().any(|issued| issued.alias.provider == external));

    let mut unsafe_update = original.clone();
    unsafe_update.tree = None;
    assert!(!full_tree_is_safe(&unsafe_update));
    assert!(remap_with_ledger(&unsafe_update, 21, &targets, &mut ledger).is_none());
    unsafe_update = original.clone();
    unsafe_update.nodes.remove(0);
    assert!(!full_tree_is_safe(&unsafe_update));
    unsafe_update = original.clone();
    unsafe_update.nodes[2].1.set_popup_for(ak::NodeId(999));
    assert!(!full_tree_is_safe(&unsafe_update));
    unsafe_update = original.clone();
    unsafe_update.focus = ak::NodeId(999);
    assert!(!full_tree_is_safe(&unsafe_update));
    unsafe_update = original.clone();
    unsafe_update.nodes[0].1.push_child(ak::NodeId(1));
    assert!(!full_tree_is_safe(&unsafe_update));
    unsafe_update = original.clone();
    unsafe_update.nodes[2].1.push_child(ak::NodeId(1));
    assert!(!full_tree_is_safe(&unsafe_update));
    unsafe_update = original.clone();
    unsafe_update.nodes.push((ak::NodeId(1), ak::Node::new(ak::Role::Button)));
    assert!(!full_tree_is_safe(&unsafe_update));
    unsafe_update = original.clone();
    unsafe_update.nodes[0].1.set_children(vec![ak::NodeId(1)]);
    unsafe_update.nodes[2].1.push_child(ak::NodeId(3));
    unsafe_update.nodes[3].1.push_child(ak::NodeId(2));
    assert!(!full_tree_is_safe(&unsafe_update));
    assert!(full_tree_is_safe(&original));
    assert_eq!(ledger.issued.len(), 3);

    let mut ineligible = original.clone();
    ineligible.nodes[0].1.set_hidden();
    assert!(full_tree_is_safe(&ineligible));
    assert!(remap_with_ledger(&ineligible, 21, &targets, &mut ledger).is_none());
    ineligible = original.clone();
    ineligible.nodes[1].1.set_disabled();
    assert!(remap_with_ledger(&ineligible, 21, &targets, &mut ledger).is_none());
    let bad_target = [TargetIdentity { physical: ak::NodeId(2), epoch: 23 }];
    assert!(remap_with_ledger(&original, 21, &bad_target, &mut ledger).is_none());
    let root_target = [TargetIdentity { physical: ak::NodeId(100), epoch: 23 }];
    assert!(remap_with_ledger(&original, 21, &root_target, &mut ledger).is_none());
    assert!(remap_with_ledger(&original, 21, &[targets[0], targets[0]], &mut ledger).is_none());
    assert!(remap_with_ledger(&original, 0, &targets, &mut ledger).is_none());
    assert!(remap_with_ledger(&original, 21, &[], &mut ledger).is_none());
    assert_eq!(ledger.issued.len(), 3);

    let mut collision = Ledger::new();
    collision.next = 100;
    assert!(remap_with_ledger(&original, 21, &targets, &mut collision).is_none());
    assert!(collision.issued.is_empty());
    let mut physical_collision = original.clone();
    physical_collision.nodes[0].1.push_child(external);
    physical_collision.nodes.push((external, ak::Node::new(ak::Role::Button)));
    assert!(full_tree_is_safe(&physical_collision));
    assert!(remap_with_ledger(&physical_collision, 21, &targets, &mut ledger).is_none());
    assert_eq!(ledger.issued.len(), 3);

    let mut capped = Ledger::new();
    for epoch in 1..=MAX_ISSUED as u64 {
        let target = TargetIdentity { physical: ak::NodeId(1), epoch };
        let nodes = validate(&original).unwrap();
        assert!(capped.aliases(25, &[target], &nodes.nodes).is_some());
    }
    assert_eq!(capped.issued.len(), MAX_ISSUED);
    assert!(remap_with_ledger(&original, 26, &targets, &mut capped).is_none());
    assert!(capped.exhausted);
    assert_eq!(capped.issued.len(), MAX_ISSUED);
    assert!(capped.issued.iter().any(|issued| issued.alias.provider == ak::NodeId(u64::MAX)));
    let mut exhausted = Ledger::new();
    exhausted.next = 1;
    assert!(remap_with_ledger(&original, 25, &targets, &mut exhausted).is_none());
    assert!(exhausted.exhausted);
    assert!(exhausted.issued.is_empty());
}
