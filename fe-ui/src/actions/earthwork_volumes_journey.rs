use super::*;
use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseEntry};
use fe_renderer::terrain_overlay::{EarthworkVolumePair, EarthworkVolumeSource};
use serde_json::json;

// Existing terrain_proposal journey moved beside the background owner it exercises.
#[test]
fn brush_commit_bind_and_volume_gate_compose_without_repeat_writes() {
    let (tx, rx) = crossbeam::channel::unbounded();
    let sender = DbCommandSender(tx);
    let mut terrain = PetalMapState {
        petal_id: Some("p".into()),
        loaded: true,
        terrain_json: Some(json!({"enabled":true,"proposals":[]})),
        ..Default::default()
    };
    let mut map = EarthworkNodeMap::default();
    super::super::terrain_proposal::handle_brush_stroke(
        &sender,
        &mut terrain,
        &mut super::super::terrain_proposal::SculptToolState::default(),
        &mut map,
        "p".into(),
        vec![[2.0, 3.0]],
        1.0,
        1.0,
        "raise".into(),
        None,
        Some(2.0),
        "earth".into(),
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        DbCommand::SetPetalTerrain { .. }
    ));
    let region = match rx.try_recv().unwrap() {
        DbCommand::CreateNode {
            correlation_id: Some(id),
            ..
        } => super::super::terrain_proposal::earthwork_region_id_from_correlation(&id)
            .unwrap()
            .to_owned(),
        other => panic!("unexpected endpoint: {other:?}"),
    };
    map.record("p", &region, "node");
    let mut nav = NavigationManager::default();
    nav.navigate_to_verse("v".into(), "Verse".into());
    nav.navigate_to_fractal("f".into(), "Fractal".into());
    nav.navigate_to_petal("p".into());
    let manager = VerseManager {
        verses: vec![VerseEntry {
            id: "v".into(),
            fractals: vec![FractalEntry {
                id: "f".into(),
                petals: vec![PetalEntry {
                    id: "p".into(),
                    nodes: vec![NodeEntry {
                        id: "node".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let role = LocalUserRole {
        role: Some(fe_database::RoleLevel::Owner),
    };
    let raw_source = terrain
        .terrain_json
        .as_ref()
        .and_then(terrain_source_fingerprint)
        .unwrap();
    let binding = EarthworkTerrainBinding {
        petal_id: Some("p".into()),
        raw_fingerprint: Some(raw_source),
        config_fingerprint: Some([7; 32]),
        input_scale_valid: true,
    };
    let source = EarthworkVolumeSource {
        petal_id: "p".into(),
        terrain_revision: 1,
        height_generation: 1,
        config_fingerprint: [7; 32],
        raw_source_fingerprint: Some(raw_source),
        assignment_fingerprint: Some([7; 32]),
        world_scale: 1.0,
    };
    let mut report = EarthworkVolumeReport {
        source: source.clone(),
        region_id: region.clone(),
        region_fingerprint: [8; 32],
        expected_cells: 16,
        sampled_cells: 0,
        pair: None,
    };
    let mut volumes = EarthworkVolumeState {
        source: Some(source),
        reports: HashMap::from([(region.clone(), report.clone())]),
    };
    macro_rules! live {
        () => {
            Live {
                nav: &nav,
                map: &map,
                terrain: &terrain,
                volumes: &volumes,
                binding: &binding,
                manager: &manager,
                role: &role,
                raw_source: Some(raw_source),
                running: true,
                height_generation: Some(1),
            }
        };
    }
    let mut owner = VolumePersistence::default();
    owner.pump(&live!(), &sender.0, 0.0);
    assert!(
        rx.try_recv().is_err(),
        "missing sample coverage never becomes numeric zero or a capture/write"
    );
    report.sampled_cells = report.expected_cells;
    report.pair = Some(EarthworkVolumePair {
        cut_m3: 0.0,
        fill_m3: 0.0,
    });
    volumes.reports.insert(region.clone(), report.clone());
    let demand = live!()
        .demand(&report)
        .expect("fully covered true zero is a valid demand");
    let capture = EditorBatchCapture {
        target: EditorBatchTarget::Nodes,
        deleted_group: None,
        scope: demand.scope.clone(),
        petal_id: "p".into(),
        members: vec![EditorBatchMember {
            node_id: "node".into(),
            history_version: Some(1),
            stamp_path: None,
            state: fe_runtime::node_history::NodeRowSnapshot {
                node_id: "node".into(),
                petal_id: "p".into(),
                display_name: Some("Earthwork".into()),
                asset_id: None,
                position: [0.0; 3],
                elevation: 0.0,
                rotation: vec![0.0; 3],
                scale: [1.0; 3],
                interactive: true,
                created_at: "2026-11-03T00:00:00Z".into(),
                edit_seq: 0,
                tombstone: None,
                properties: serde_json::from_value(
                    json!({"node_kind":"earthwork_region","region_id":region,"material":"earth"}),
                )
                .unwrap(),
            },
        }],
        groups: vec![],
        petal_snapshot: "exact-cbor-fixture".into(),
        world_scale: 1.0,
        terrain_source_fingerprint: Some(raw_source),
    };
    let mut foreign = capture.clone();
    foreign.terrain_source_fingerprint = Some([9; 32]);
    assert!(
        !capture_matches(&foreign, &demand),
        "new DB terrain cannot be blessed by a lagging UI measurement"
    );
    foreign.terrain_source_fingerprint = None;
    assert!(
        !capture_matches(&foreign, &demand),
        "legacy/default DTO source is unknown"
    );
    foreign = capture.clone();
    foreign.members[0].state.petal_id = "foreign".into();
    assert!(!capture_matches(&foreign, &demand));

    owner.pump(&live!(), &sender.0, 1.0);
    let read_id = match rx.try_recv().unwrap() {
        DbCommand::ReadEditorBatch {
            target: EditorBatchTarget::Nodes,
            node_ids,
            correlation_id,
            ..
        } => {
            assert_eq!(node_ids, vec!["node"]);
            correlation_id
        }
        other => panic!("expected pure capture: {other:?}"),
    };
    owner.route(
        &DbResult::EditorBatchLoaded {
            capture: capture.clone(),
            correlation_id: read_id,
        },
        &live!(),
        1.1,
        false,
    );
    owner.pump(&live!(), &sender.0, 1.2);
    let save_id = match rx.try_recv().unwrap() {
        DbCommand::CommitCheckedEditorBatch {
            request,
            correlation_id,
        } => {
            let EditorBatchEdit::Targets(edits) = request.edit else {
                panic!("typed target batch")
            };
            assert_eq!(edits.len(), 1);
            let EditorTargetMutation::Properties(properties) = &edits[0].mutation else {
                panic!("pair properties")
            };
            assert_eq!(
                properties.len(),
                2,
                "both fields belong to one checked save"
            );
            correlation_id
        }
        other => panic!("expected atomic pair: {other:?}"),
    };
    assert!(owner.write.as_ref().unwrap().accepted);
    assert!(
        owner.confirmed.is_empty(),
        "channel acceptance cannot confirm a persisted pair"
    );
    owner.pump(&live!(), &sender.0, 2.0);
    assert!(
        rx.try_recv().is_err(),
        "accepted write is never automatically replayed"
    );
    let mut member = capture.members[0].clone();
    member.history_version = Some(3);
    member
        .state
        .properties
        .insert(KEY_CUT_VOLUME.into(), json!(0.0));
    member
        .state
        .properties
        .insert(KEY_FILL_VOLUME.into(), json!(0.0));
    let receipt = EditorBatchReceipt {
        target: EditorBatchTarget::Nodes,
        request_id: save_id.clone(),
        scope: demand.scope.clone(),
        petal_id: "p".into(),
        members: vec![member],
        groups: vec![],
        created_group_id: None,
    };
    assert!(!owner.owns_result(&DbResult::EditorBatchCommitted {
        receipt: receipt.clone(),
        correlation_id: "unrelated".into()
    }));
    assert!(owner.owns_result(&DbResult::EditorBatchCommitted {
        receipt: receipt.clone(),
        correlation_id: save_id.clone()
    }));
    assert!(
        owner
            .projection_receipt(
                &DbResult::EditorBatchCommitted {
                    receipt: receipt.clone(),
                    correlation_id: save_id.clone()
                },
                &live!()
            )
            .is_some(),
        "only this exact owned unchanged-TRS property receipt takes the property-only projection route"
    );
    owner.route(
        &DbResult::EditorBatchCommitted {
            receipt: receipt.clone(),
            correlation_id: "unrelated".into(),
        },
        &live!(),
        2.1,
        false,
    );
    assert!(owner.confirmed.is_empty());
    let mut partial = receipt.clone();
    partial.members[0].state.properties.remove(KEY_FILL_VOLUME);
    assert!(
        owner
            .projection_receipt(
                &DbResult::EditorBatchCommitted {
                    receipt: partial.clone(),
                    correlation_id: save_id.clone()
                },
                &live!()
            )
            .is_none(),
        "invalid owned receipts cannot fall through to generic full-TRS projection"
    );
    owner.route(
        &DbResult::EditorBatchCommitted {
            receipt: partial,
            correlation_id: save_id.clone(),
        },
        &live!(),
        2.2,
        false,
    );
    assert!(
        owner.confirmed.is_empty() && owner.write.is_some(),
        "incomplete pair receipt cannot consume accepted ownership"
    );
    owner.pump(&live!(), &sender.0, 20.0);
    let resolve_id = match rx.try_recv().unwrap() {
        DbCommand::ResolveCheckedEditorBatch {
            request_id,
            node_ids,
            correlation_id,
            ..
        } => {
            assert_eq!(request_id, save_id);
            assert_eq!(node_ids, vec!["node"]);
            correlation_id
        }
        other => panic!("read the exact unknown outcome, never resend: {other:?}"),
    };
    let mut current = capture.clone();
    current.members = receipt.members.clone();
    owner.route(
        &DbResult::EditorBatchResolved {
            correlation_id: resolve_id,
            resolution: EditorBatchResolution {
                request_id: save_id.clone(),
                receipt: Some(receipt.clone()),
                current: current.clone(),
            },
        },
        &live!(),
        20.1,
        true,
    );
    assert!(owner.write.is_none());
    assert!(
        owner.confirmed.contains_key("node"),
        "only exact durable receipt confirms the pair"
    );
    owner.pump(&live!(), &sender.0, 21.0);
    assert!(
        rx.try_iter()
            .all(|command| !matches!(command, DbCommand::CommitCheckedEditorBatch { .. })),
        "same measured pair is quiet after outcome routing"
    );

    for (changed_context, has_receipt) in [(true, true), (false, false)] {
        let mut outcome = VolumePersistence::default();
        outcome.write = Some(Write {
            id: save_id.clone(),
            demand: demand.clone(),
            capture: capture.clone(),
            accepted: true,
            retired: changed_context,
            at: 0.0,
            resolve: Some(("resolve-fixture".into(), true, 0.0)),
        });
        outcome.route(
            &DbResult::EditorBatchResolved {
                correlation_id: "resolve-fixture".into(),
                resolution: EditorBatchResolution {
                    request_id: save_id.clone(),
                    receipt: has_receipt.then(|| receipt.clone()),
                    current: if has_receipt {
                        current.clone()
                    } else {
                        capture.clone()
                    },
                },
            },
            &live!(),
            1.0,
            false,
        );
        assert!(outcome.write.is_none() && outcome.confirmed.is_empty());
        outcome.handled.get_mut("node").unwrap().access = Some(u128::MAX);
        outcome.handled.get_mut("node").unwrap().incarnation = u128::MAX;
        outcome.pump(&live!(), &sender.0, 2.0);
        assert!(
            rx.try_iter()
                .all(|command| !matches!(command, DbCommand::CommitCheckedEditorBatch { .. })),
            "retired or uncommitted outcome never replays"
        );
    }
    assert!(
        !Live {
            height_generation: Some(2),
            ..live!()
        }
        .matches(&demand),
        "a late height update invalidates even a not-yet-rebaked report"
    );
    let newer_binding = EarthworkTerrainBinding {
        raw_fingerprint: Some([9; 32]),
        petal_id: Some("p".into()),
        config_fingerprint: Some([7; 32]),
        input_scale_valid: true,
    };
    assert!(
        Live {
            binding: &newer_binding,
            raw_source: Some([9; 32]),
            ..live!()
        }
        .demand(&report)
        .is_none(),
        "new raw DB/UI source cannot retag the still-current rounded-equal old producer revision"
    );
    let mut changed_report = report.clone();
    changed_report.source.terrain_revision += 1;
    assert!(live!().demand(&changed_report).is_none());
    changed_report = report.clone();
    changed_report.region_fingerprint[0] ^= 1;
    assert!(live!().demand(&changed_report).is_none());
    changed_report = report.clone();
    changed_report.source.raw_source_fingerprint = None;
    assert!(
        live!().demand(&changed_report).is_none(),
        "an unbacked producer source never gets a raw-identity fallback"
    );
    let mut retagged = report.clone();
    retagged.source.raw_source_fingerprint = Some([9; 32]);
    let mut retagged_state = EarthworkVolumeState {
        source: Some(retagged.source.clone()),
        reports: HashMap::from([(region.clone(), retagged)]),
    };
    assert!(
        Live {
            volumes: &retagged_state,
            ..live!()
        }
        .demand(&report)
        .is_none(),
        "rounded-equal older source cannot be retagged by a newer raw document"
    );
    retagged_state.reports.clear();
    let mut foreign_nav = NavigationManager::default();
    foreign_nav.navigate_to_verse("foreign".into(), "Other".into());
    assert!(
        !Live {
            nav: &foreign_nav,
            ..live!()
        }
        .matches(&demand)
    );
    let viewer = LocalUserRole {
        role: Some(fe_database::RoleLevel::Viewer),
    };
    assert!(
        !Live {
            role: &viewer,
            ..live!()
        }
        .matches(&demand)
    );

    let (full_tx, full_rx) = crossbeam::channel::bounded(1);
    full_tx
        .try_send(DbCommand::GetNodeProperties {
            node_id: "occupied".into(),
            correlation_id: None,
        })
        .unwrap();
    let mut queued = VolumePersistence::default();
    queued.write = Some(Write {
        id: "queued-volume".into(),
        demand: demand.clone(),
        capture: capture.clone(),
        accepted: false,
        retired: false,
        at: 0.0,
        resolve: None,
    });
    queued.pump(&live!(), &full_tx, 0.0);
    assert!(!queued.write.as_ref().unwrap().accepted && queued.confirmed.is_empty());
    full_rx.try_recv().unwrap();
    queued.pump(&live!(), &full_tx, 1.0);
    assert!(queued.write.as_ref().unwrap().accepted);
    assert!(matches!(
        full_rx.try_recv().unwrap(),
        DbCommand::CommitCheckedEditorBatch { .. }
    ));
    queued.pump(&live!(), &full_tx, 2.0);
    assert!(full_rx.try_recv().is_err());
    drop(full_rx);
    queued.pump(&live!(), &full_tx, 20.0);
    assert!(
        queued.write.as_ref().unwrap().accepted && queued.confirmed.is_empty(),
        "disconnection after acceptance retains the unknown original owner"
    );
    let (gone_tx, gone_rx) = crossbeam::channel::bounded(1);
    drop(gone_rx);
    let mut unsent = VolumePersistence::default();
    unsent.write = Some(Write {
        id: "unsent-volume".into(),
        demand: demand.clone(),
        capture: capture.clone(),
        accepted: false,
        retired: false,
        at: 0.0,
        resolve: None,
    });
    unsent.pump(&live!(), &gone_tx, 0.0);
    assert!(
        unsent.write.is_none() && unsent.confirmed.is_empty(),
        "disconnected admission is never an accepted write"
    );
}

pub(crate) fn assert_dispatcher_ownership(
    app: &mut App,
    commands: &crossbeam::channel::Receiver<DbCommand>,
) {
    use crate::node_manager::NodeManager;
    use crate::plugin::{InspectorFormState, SpawnedNodeMarker};
    use bevy::ecs::system::RunSystemOnce;
    use fe_renderer::terrain_height::TerrainHeightField;
    use fe_runtime::messages::DbConfirmedTransform;

    let mut terminal_write = None;
    for scenario in [
        "direct", "resolved", "height", "raw", "epoch", "access", "retired",
    ] {
        let node = format!("volume-{scenario}");
        let mut nav = NavigationManager::default();
        nav.navigate_to_petal_in_fractal("volume-v", "V", "volume-f", "F", "volume-p");
        let mut manager = VerseManager {
            verses: vec![VerseEntry {
                id: "volume-v".into(),
                fractals: vec![FractalEntry {
                    id: "volume-f".into(),
                    petals: vec![PetalEntry {
                        id: "volume-p".into(),
                        nodes: vec![NodeEntry {
                            id: node.clone(),
                            name: "Earthwork".into(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        manager.synchronize_scene_navigation(&nav);
        let mut terrain = PetalMapState {
            petal_id: Some("volume-p".into()),
            loaded: true,
            ..Default::default()
        };
        terrain.confirm_terrain_document(Some(json!({"enabled":true,"proposals":[]})));
        let raw = terrain.confirmed_terrain_fingerprint().unwrap();
        let mut map = EarthworkNodeMap::default();
        map.record("volume-p", "region", &node);
        let binding = EarthworkTerrainBinding {
            petal_id: Some("volume-p".into()),
            raw_fingerprint: Some(raw),
            config_fingerprint: Some([7; 32]),
            input_scale_valid: true,
        };
        let report = EarthworkVolumeReport {
            source: EarthworkVolumeSource {
                petal_id: "volume-p".into(),
                terrain_revision: 1,
                height_generation: 0,
                config_fingerprint: [7; 32],
                raw_source_fingerprint: Some(raw),
                assignment_fingerprint: Some([7; 32]),
                world_scale: 1.0,
            },
            region_id: "region".into(),
            region_fingerprint: [8; 32],
            expected_cells: 4,
            sampled_cells: 4,
            pair: Some(EarthworkVolumePair {
                cut_m3: 0.0,
                fill_m3: 12.0,
            }),
        };
        let volumes = EarthworkVolumeState {
            source: Some(report.source.clone()),
            reports: HashMap::from([("region".into(), report.clone())]),
        };
        let role = LocalUserRole {
            role: Some(fe_database::RoleLevel::Owner),
        };
        let demand = Live {
            nav: &nav,
            map: &map,
            terrain: &terrain,
            volumes: &volumes,
            binding: &binding,
            manager: &manager,
            role: &role,
            raw_source: Some(raw),
            running: true,
            height_generation: Some(0),
        }
        .demand(&report)
        .unwrap();
        let capture = EditorBatchCapture {
            target:EditorBatchTarget::Nodes, deleted_group:None, scope:demand.scope.clone(), petal_id:"volume-p".into(),
            members:vec![EditorBatchMember { node_id:node.clone(), history_version:Some(1), stamp_path:None,
                state:fe_runtime::node_history::NodeRowSnapshot { node_id:node.clone(), petal_id:"volume-p".into(), display_name:Some("Earthwork".into()), asset_id:None,
                    position:[0.0;3], elevation:0.0, rotation:vec![0.0;3], scale:[1.0;3], interactive:true, created_at:"fixture".into(), edit_seq:0, tombstone:None,
                    properties:serde_json::from_value(json!({"node_kind":"earthwork_region","region_id":"region","material":"earth"})).unwrap() }
            }], groups:vec![], petal_snapshot:"exact-fixture".into(), world_scale:1.0, terrain_source_fingerprint:Some(raw),
        };
        let mut saved = capture.members[0].clone();
        saved.history_version = Some(3);
        saved
            .state
            .properties
            .insert(KEY_CUT_VOLUME.into(), json!(0.0));
        saved
            .state
            .properties
            .insert(KEY_FILL_VOLUME.into(), json!(12.0));
        let id = format!("accepted-{scenario}");
        let receipt = EditorBatchReceipt {
            target: EditorBatchTarget::Nodes,
            request_id: id.clone(),
            scope: demand.scope.clone(),
            petal_id: "volume-p".into(),
            members: vec![saved],
            groups: vec![],
            created_group_id: None,
        };
        let mut owner = VolumePersistence::default();
        if scenario == "retired" {
            terminal_write = Some(Write {
                id: "terminal-accepted".into(),
                demand: demand.clone(),
                capture: capture.clone(),
                accepted: true,
                retired: false,
                at: 0.0,
                resolve: Some(("terminal-resolve".into(), true, 0.0)),
            });
        }
        owner.write = Some(Write {
            id: id.clone(),
            demand,
            capture: capture.clone(),
            accepted: true,
            retired: scenario == "retired",
            at: 0.0,
            resolve: (scenario == "resolved").then(|| ("resolve-owned".into(), true, 0.0)),
        });
        app.insert_resource(nav)
            .insert_resource(manager)
            .insert_resource(terrain)
            .insert_resource(map)
            .insert_resource(binding)
            .insert_resource(volumes)
            .insert_resource(role)
            .insert_resource(owner)
            .insert_resource(TerrainHeightField::default());
        app.init_resource::<crate::undo::UndoState>();
        let preview = Transform::from_xyz(91.0, 2.0, 3.0);
        let entity = app
            .world_mut()
            .spawn((
                SpawnedNodeMarker {
                    node_id: node.clone(),
                    petal_id: "volume-p".into(),
                },
                crate::verse_manager::ResidentNode,
                preview,
                DbConfirmedTransform {
                    position: [0.0; 3],
                    rotation: [0.0; 3],
                    scale: [1.0; 3],
                },
            ))
            .id();
        {
            let mut nodes = app.world_mut().resource_mut::<NodeManager>();
            nodes.select(entity, &node);
            nodes
                .mark_transform_committed(entity, Transform::IDENTITY)
                .unwrap();
            nodes.gesture_active = true;
        }
        app.world_mut()
            .resource_mut::<InspectorFormState>()
            .node_properties = json!({"sentinel":"unchanged"});
        match scenario {
            "height" => app.world_mut().resource_mut::<TerrainHeightField>().clear(),
            "raw" => app
                .world_mut()
                .resource_mut::<PetalMapState>()
                .replace_terrain_document(Some(json!({"enabled":false}))),
            "epoch" => {
                app.world_mut()
                    .resource_mut::<VerseManager>()
                    .node_cache_epochs
                    .insert(node.clone(), 77);
            }
            "access" => {
                app.world_mut().write_message(DbResult::LocalRoleResolved {
                    scope: capture.scope.clone(),
                    role: "None".into(),
                });
            }
            _ => {}
        }
        let before_source = app
            .world()
            .resource::<VerseManager>()
            .scene_admission(&node, "volume-p");
        let before_undo = app
            .world()
            .resource::<crate::undo::UndoState>()
            .history_counts();
        let before_generation = app
            .world()
            .resource::<crate::undo::UndoState>()
            .normal_generation();
        let _ = commands.try_iter().collect::<Vec<_>>();
        let result = if scenario == "resolved" {
            let mut current = capture.clone();
            current.members = receipt.members.clone();
            DbResult::EditorBatchResolved {
                correlation_id: "resolve-owned".into(),
                resolution: EditorBatchResolution {
                    request_id: id,
                    receipt: Some(receipt),
                    current,
                },
            }
        } else {
            DbResult::EditorBatchCommitted {
                correlation_id: id,
                receipt,
            }
        };
        app.world_mut().write_message(result);
        app.update();
        let current = scenario == "direct" || scenario == "resolved";
        assert!(
            app.world().resource::<VolumePersistence>().write.is_none(),
            "an exact accepted outcome drains independently of projection eligibility: {scenario}"
        );
        assert_eq!(
            app.world()
                .resource::<VolumePersistence>()
                .confirmed
                .contains_key(&node),
            current,
            "only current complete projection confirms: {scenario}"
        );
        assert_eq!(
            app.world()
                .resource::<VerseManager>()
                .committed_versions
                .get(&node)
                .copied(),
            current.then_some(3),
            "stale receipt cannot advance versions: {scenario}"
        );
        if current {
            assert!(app.world().resource::<VerseManager>().current_scene_properties_match(&node,"volume-p",&json!({"node_kind":"earthwork_region","region_id":"region","material":"earth","cut_volume_m3":0.0,"fill_volume_m3":12.0})));
            assert_eq!(
                app.world()
                    .resource::<InspectorFormState>()
                    .node_properties
                    .get(KEY_FILL_VOLUME),
                Some(&json!(12.0))
            );
        } else {
            assert_eq!(
                app.world()
                    .resource::<VerseManager>()
                    .scene_admission(&node, "volume-p"),
                before_source
            );
            assert_eq!(
                app.world().resource::<InspectorFormState>().node_properties,
                json!({"sentinel":"unchanged"})
            );
        }
        assert_eq!(
            *app.world().get::<Transform>(entity).unwrap(),
            preview,
            "background properties must not overwrite newer live TRS: {scenario}"
        );
        assert_eq!(
            app.world()
                .get::<DbConfirmedTransform>(entity)
                .unwrap()
                .position,
            [0.0; 3]
        );
        let nodes = app.world().resource::<NodeManager>();
        assert_eq!(nodes.selected_entity(), Some(entity));
        assert!(
            nodes.primary().unwrap().drag_committed && nodes.gesture_active,
            "newer gesture state survives direct and resolved outcome: {scenario}"
        );
        assert_eq!(
            app.world()
                .resource::<crate::undo::UndoState>()
                .history_counts(),
            before_undo
        );
        assert_eq!(
            app.world()
                .resource::<crate::undo::UndoState>()
                .normal_generation(),
            before_generation
        );
        assert!(!commands.try_iter().any(|command|matches!(command,DbCommand::GetNodeProperties { node_id,.. }|DbCommand::ReadReplicatedNodeCurrent { node_id,.. } if node_id==node)),"the owned property-only outcome must never escape through generic full-TRS hydration: {scenario}");
        app.world_mut().despawn(entity);
    }

    let mut owner = VolumePersistence::default();
    owner.write = terminal_write;
    app.insert_resource(owner);
    app.init_resource::<fe_runtime::maintenance::TerminalMode>();
    app.world_mut()
        .resource_mut::<fe_runtime::maintenance::TerminalMode>()
        .enter();
    let _ = commands.try_iter().collect::<Vec<_>>();
    app.world_mut()
        .resource_mut::<UiManager>()
        .earthwork_volumes
        .status
        .insert("terminal".into(), "display-only");
    crate::maintenance::clear_read_models(app.world_mut());
    let owner = app.world().resource::<VolumePersistence>();
    assert!(
        owner
            .write
            .as_ref()
            .is_some_and(|write| write.accepted && write.retired && write.resolve.is_some()),
        "actual UI reset retains the exact accepted unknown owner and resolve identity"
    );
    assert!(
        app.world()
            .resource::<UiManager>()
            .earthwork_volumes
            .status
            .is_empty()
    );
    app.world_mut().run_system_once(persist).unwrap();
    assert!(
        commands.try_recv().is_err(),
        "terminal PostUpdate cannot emit a read, save or Resolve"
    );
    crate::maintenance::observe_terminal_db_result(
        app.world_mut(),
        &DbResult::RequestFailed {
            correlation_id: "terminal-accepted".into(),
            error: "unknown".into(),
        },
    );
    assert!(
        app.world().resource::<VolumePersistence>().write.is_some(),
        "failure or unconfirmed DB lifetime cannot clear accepted ownership"
    );
    crate::maintenance::clear_retained_outcomes_after_database_join(app.world_mut());
    assert!(
        app.world().resource::<VolumePersistence>().write.is_none(),
        "only the host's actual-successful-DB-join hook clears unresolved ownership"
    );
}
