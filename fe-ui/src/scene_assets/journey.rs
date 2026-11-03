//! Extends the existing DB-result/residency App journey; see AGENTS.md.

use bevy::prelude::*;
use fe_runtime::{
    messages::{DbCommand, DbResult},
    scene_asset::{
        SceneAssetAdoptionCandidate, SceneAssetAdoptionReceipt, SceneAssetNeeded,
        SceneAssetReferenceWitness,
    },
    scene_replication::{ReplicaAssetUnavailable, ReplicaNodeAsset, ReplicatedNodeCurrent},
};

use super::{Frame, SceneAssets};
use crate::{
    actions::UiManager, navigation_manager::NavigationManager, node_manager::NodeManager,
    plugin::LocalUserRole, verse_manager::VerseManager,
};

fn begin(app: &mut App, node: &str) {
    app.world_mut()
        .resource_scope(|world, mut state: Mut<SceneAssets>| {
            world.resource_scope(|world, mut manager: Mut<VerseManager>| {
                state.begin_review(
                    node,
                    &mut manager,
                    world.resource(),
                    world.resource(),
                    world.resource(),
                    world
                        .resource::<crate::notice::NoticeClock>()
                        .elapsed_secs_f64(),
                );
            });
        });
}

fn confirm(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut state: Mut<SceneAssets>| {
            let id = state
                .adoption
                .preview()
                .expect("reviewed candidate")
                .id
                .clone();
            state.confirm_review(
                &id,
                world.resource(),
                world.resource(),
                world.resource(),
                world.resource(),
                world
                    .resource::<crate::notice::NoticeClock>()
                    .elapsed_secs_f64(),
            );
        });
}

fn read_id(receiver: &crossbeam::channel::Receiver<DbCommand>) -> String {
    let commands: Vec<_> = receiver.try_iter().collect();
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, DbCommand::AdoptSceneAsset { .. })),
        "a review alone never adopts"
    );
    let ids: Vec<_> = commands
        .into_iter()
        .filter_map(|command| match command {
            DbCommand::ReadSceneAssetAdoption { request_id, .. } => Some(request_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        ids.len(),
        1,
        "exactly one private candidate read is submitted"
    );
    ids.into_iter().next().unwrap()
}

fn candidate_reply(app: &mut App, id: String, candidate: &SceneAssetAdoptionCandidate) {
    app.world_mut()
        .write_message(DbResult::SceneAssetAdoptionCandidate {
            node_id: candidate.node_id.clone(),
            request_id: id,
            result: Ok(candidate.clone()),
        });
    app.update();
}

fn accepted_adoption(
    app: &mut App,
    receiver: &crossbeam::channel::Receiver<DbCommand>,
    candidate: &SceneAssetAdoptionCandidate,
) -> String {
    begin(app, &candidate.node_id);
    app.update();
    candidate_reply(app, read_id(receiver), candidate);
    confirm(app);
    app.update();
    candidate_reply(app, read_id(receiver), candidate);
    let commands: Vec<_> = receiver.try_iter().collect();
    let writes: Vec<_> = commands
        .into_iter()
        .filter_map(|command| match command {
            DbCommand::AdoptSceneAsset {
                candidate: stored,
                request_id,
            } => {
                assert_eq!(stored, *candidate);
                Some(request_id)
            }
            _ => None,
        })
        .collect();
    assert_eq!(writes.len(), 1, "fresh exact confirmation submits once");
    writes.into_iter().next().unwrap()
}

fn fetch_id(receiver: &crossbeam::channel::Receiver<fe_sync::SyncCommand>) -> String {
    let ids: Vec<_> = receiver
        .try_iter()
        .filter_map(|command| match command {
            fe_sync::SyncCommand::FetchSceneAsset { request_id, .. } => Some(request_id),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 1);
    ids.into_iter().next().unwrap()
}

fn event(app: &mut App, event: fe_sync::SyncEvent) {
    app.world_mut()
        .resource_mut::<fe_sync::SceneAssetEventBuffer>()
        .events
        .push(event);
    app.update();
}

pub(crate) fn extend_asset_workflow(
    app: &mut App,
    mut current: ReplicatedNodeCurrent,
    receiver: &crossbeam::channel::Receiver<DbCommand>,
) {
    let (sync_tx, sync_rx) = crossbeam::channel::bounded(8);
    app.init_resource::<SceneAssets>()
        .init_resource::<fe_sync::SceneAssetEventBuffer>()
        .insert_resource(fe_sync::SyncCommandSenderRes(sync_tx));
    let _ = receiver.try_iter().collect::<Vec<_>>();
    let node = current.state.node_id.clone();
    let asset = current.state.asset_id.clone().unwrap();
    let hash = [0xaa; 32];
    let needed = SceneAssetNeeded {
        verse_id: "v1".into(),
        scope: current.scope.clone(),
        node_id: node.clone(),
        asset_id: asset.clone(),
        content_hash: hash,
        size_bytes: 128,
        provenance: current.provenance.clone(),
    };
    current.asset = ReplicaNodeAsset::Unavailable {
        asset_id: asset.clone(),
        reason: ReplicaAssetUnavailable::MissingMetadata,
    };
    app.world_mut()
        .write_message(DbResult::ReplicatedNodeCurrent {
            current: current.clone(),
        });
    app.world_mut().write_message(DbResult::SceneAssetNeeded {
        needed: needed.clone(),
    });
    app.update();
    assert!(
        sync_rx.try_recv().is_err(),
        "missing metadata cannot invent a hash or download admission"
    );
    current.asset = ReplicaNodeAsset::Unavailable {
        asset_id: asset.clone(),
        reason: ReplicaAssetUnavailable::MissingBlob,
    };
    let mut wrong = needed.clone();
    wrong.asset_id = "different-asset".into();
    app.world_mut()
        .write_message(DbResult::ReplicatedNodeCurrent {
            current: current.clone(),
        });
    app.world_mut()
        .write_message(DbResult::SceneAssetNeeded { needed: wrong });
    app.update();
    assert!(
        sync_rx.try_recv().is_err(),
        "a Need must match the admitted node/asset source"
    );
    app.world_mut().write_message(DbResult::SceneAssetNeeded {
        needed: needed.clone(),
    });
    app.update();
    let first = fetch_id(&sync_rx);
    event(
        app,
        fe_sync::SyncEvent::SceneAssetFetchSettled {
            verse_id: "v1".into(),
            hash,
            request_ids: vec!["foreign".into()],
            result: Ok(()),
        },
    );
    assert_eq!(app.world().resource::<SceneAssets>().fetches.pending(), 1);
    event(app, fe_sync::SyncEvent::BlobReady { hash });
    assert_eq!(
        app.world().resource::<SceneAssets>().fetches.pending(),
        1,
        "verified byte hints do not settle owned transfer results"
    );
    app.update();
    let fresh = receiver
        .try_iter()
        .find_map(|command| match command {
            DbCommand::ReadReplicatedNodeCurrent {
                node_id,
                request_id,
            } if node_id == node => Some(request_id),
            _ => None,
        })
        .expect("BlobReady asks for a fresh owned current read");
    current.asset = ReplicaNodeAsset::LocalBlob {
        asset_id: asset.clone(),
        content_hash: fe_runtime::blob_store::hash_to_hex(&hash),
        content_type: "model/gltf-binary".into(),
        asset_path: "models/replica.glb".into(),
    };
    app.world_mut()
        .write_message(DbResult::ReplicatedNodeCurrentRead {
            node_id: node.clone(),
            request_id: fresh,
            result: Ok(current.clone()),
        });
    app.update();
    assert_eq!(
        app.world().resource::<SceneAssets>().fetches.pending(),
        1,
        "newly verified availability remains separate from an earlier owned finality"
    );
    assert!(
        app.world()
            .resource::<VerseManager>()
            .all_nodes()
            .find(|entry| entry.id == node)
            .unwrap()
            .asset_path
            .is_some()
    );
    event(
        app,
        fe_sync::SyncEvent::SceneAssetFetchSettled {
            verse_id: "v1".into(),
            hash,
            request_ids: vec![first.clone()],
            result: Err("final admission changed after verified persistence".into()),
        },
    );
    assert_eq!(app.world().resource::<SceneAssets>().fetches.pending(), 0);
    app.update();
    assert!(receiver.try_iter().any(|command| matches!(command, DbCommand::ReadReplicatedNodeCurrent { node_id, .. } if node_id == node)), "even an owned failure re-reads real availability");

    current.asset = ReplicaNodeAsset::Unavailable {
        asset_id: asset.clone(),
        reason: ReplicaAssetUnavailable::MissingBlob,
    };
    app.world_mut()
        .write_message(DbResult::ReplicatedNodeCurrent {
            current: current.clone(),
        });
    app.world_mut().write_message(DbResult::SceneAssetNeeded {
        needed: needed.clone(),
    });
    app.update();
    let second = fetch_id(&sync_rx);
    assert_ne!(first, second);
    event(
        app,
        fe_sync::SyncEvent::SceneAssetFetchSettled {
            verse_id: "v1".into(),
            hash,
            request_ids: vec![first],
            result: Err("old cancellation".into()),
        },
    );
    assert_eq!(
        app.world().resource::<SceneAssets>().fetches.pending(),
        1,
        "an old same-hash job cannot settle a newer owner"
    );
    event(
        app,
        fe_sync::SyncEvent::SceneAssetFetchSettled {
            verse_id: "v1".into(),
            hash,
            request_ids: vec![second],
            result: Ok(()),
        },
    );
    assert!(
        app.world()
            .resource::<VerseManager>()
            .all_nodes()
            .find(|entry| entry.id == node)
            .unwrap()
            .asset_path
            .is_none(),
        "successful transport never replays a held DTO as current"
    );
    app.update();
    assert!(receiver.try_iter().any(|command| matches!(command, DbCommand::ReadReplicatedNodeCurrent { node_id, .. } if node_id == node)));
    app.world_mut().write_message(DbResult::SceneAssetNeeded {
        needed: needed.clone(),
    });
    app.update();
    let _third = fetch_id(&sync_rx);
    app.world_mut()
        .resource_mut::<fe_sync::SceneAssetEventBuffer>()
        .overflowed = true;
    app.update();
    assert_eq!(
        app.world().resource::<SceneAssets>().fetches.pending(),
        0,
        "lost finality reports unconfirmed and reconciles, without fabricating availability"
    );
    assert!(
        app.world()
            .resource::<SceneAssets>()
            .status
            .as_ref()
            .unwrap()
            .1
            .contains("overflowed")
    );
    assert!(
        app.world()
            .resource::<VerseManager>()
            .all_nodes()
            .find(|entry| entry.id == node)
            .unwrap()
            .asset_path
            .is_none()
    );

    let scope = current.scope.clone();
    app.world_mut().write_message(DbResult::LocalRoleResolved {
        scope: scope.clone(),
        role: "none".into(),
    });
    app.world_mut()
        .write_message(DbResult::SceneAssetNeeded { needed });
    app.update();
    assert_eq!(app.world().resource::<SceneAssets>().fetches.pending(), 0);
    assert!(sync_rx.try_recv().is_err());
    app.world_mut().write_message(DbResult::LocalRoleResolved {
        scope: scope.clone(),
        role: "owner".into(),
    });
    current.asset = ReplicaNodeAsset::LocalBlob {
        asset_id: asset.clone(),
        content_hash: fe_runtime::blob_store::hash_to_hex(&hash),
        content_type: "model/gltf-binary".into(),
        asset_path: "models/replica.glb".into(),
    };
    app.world_mut()
        .write_message(DbResult::ReplicatedNodeCurrent {
            current: current.clone(),
        });
    app.update();
    let entity = app
        .world_mut()
        .query::<(Entity, &crate::plugin::SpawnedNodeMarker)>()
        .iter(app.world())
        .find_map(|(entity, marker)| (marker.node_id == node).then_some(entity))
        .unwrap();
    assert!(
        app.world().get::<bevy::scene::SceneRoot>(entity).is_some(),
        "late authorized metadata and verified bytes restore the real SceneRoot branch"
    );
    app.world_mut()
        .resource_mut::<NodeManager>()
        .select(entity, &node);
    let candidate = SceneAssetAdoptionCandidate {
        node_id: node.clone(),
        asset_id: asset,
        scope: scope.clone(),
        content_hash: hash,
        content_type: "model/gltf-binary".into(),
        size_bytes: 128,
        asset_fingerprint: "asset-row-1".into(),
        reference_fingerprints: vec![SceneAssetReferenceWitness {
            node_id: node.clone(),
            fingerprint: "node-row-1".into(),
        }],
        model_fingerprint: Some("portal-row-1".into()),
    };
    let _ = receiver.try_iter().collect::<Vec<_>>();
    begin(app, &node);
    app.update();
    let initial = read_id(receiver);
    candidate_reply(app, "foreign-review".into(), &candidate);
    assert!(!app.world().resource::<SceneAssets>().adoption.ready());
    candidate_reply(app, initial, &candidate);
    assert_eq!(
        app.world()
            .resource::<SceneAssets>()
            .adoption
            .preview()
            .unwrap()
            .candidate,
        candidate
    );
    confirm(app);
    app.update();
    let recheck = read_id(receiver);
    let mut changed = candidate.clone();
    changed.reference_fingerprints[0].fingerprint = "node-row-2".into();
    candidate_reply(app, recheck, &changed);
    assert!(
        app.world().resource::<SceneAssets>().adoption.ready(),
        "changed references require another explicit review"
    );
    assert!(
        !receiver
            .try_iter()
            .any(|command| matches!(command, DbCommand::AdoptSceneAsset { .. }))
    );
    app.world_mut()
        .resource_mut::<SceneAssets>()
        .cancel_review();
    let accepted = accepted_adoption(app, receiver, &candidate);
    app.update();
    assert!(
        !receiver
            .try_iter()
            .any(|command| matches!(command, DbCommand::AdoptSceneAsset { .. })),
        "accepted writes are never replayed"
    );
    app.world_mut()
        .resource_mut::<crate::notice::NoticeClock>()
        .advance_by(std::time::Duration::from_secs(6));
    app.update();
    assert_eq!(app.world().resource::<SceneAssets>().adoption.pending(), 1);
    assert!(
        app.world()
            .resource::<SceneAssets>()
            .adoption
            .status(&node)
            .unwrap()
            .contains("unknown")
    );
    app.world_mut().write_message(DbResult::LocalRoleResolved {
        scope: scope.clone(),
        role: "none".into(),
    });
    app.update();
    let intent = app.world().resource::<NavigationManager>().intent_revision;
    let selection = app.world().resource::<NodeManager>().generation();
    let wrong_receipt = SceneAssetAdoptionReceipt {
        candidate: changed,
        operation_id: "wrong-op".into(),
        adopter_did: "did:key:owner".into(),
        adopted_at: "fixture".into(),
    };
    app.world_mut().write_message(DbResult::SceneAssetAdopted {
        request_id: accepted.clone(),
        result: Ok(wrong_receipt),
    });
    app.update();
    assert_eq!(
        app.world().resource::<SceneAssets>().adoption.pending(),
        1,
        "an unrelated candidate cannot confirm the accepted write"
    );
    app.world_mut().write_message(DbResult::SceneAssetAdopted {
        request_id: accepted,
        result: Err("exact final refusal".into()),
    });
    app.update();
    assert_eq!(
        app.world().resource::<SceneAssets>().adoption.pending(),
        0,
        "late outcomes still drain while access is unavailable"
    );
    assert_eq!(
        app.world().resource::<NavigationManager>().intent_revision,
        intent
    );
    assert_eq!(
        app.world().resource::<NodeManager>().generation(),
        selection
    );
    assert!(
        app.world()
            .resource::<VerseManager>()
            .committed_versions
            .get(&node)
            .is_none(),
        "adoption is not an object-history receipt"
    );

    app.world_mut().write_message(DbResult::LocalRoleResolved {
        scope,
        role: "owner".into(),
    });
    app.world_mut()
        .write_message(DbResult::ReplicatedNodeCurrent {
            current: current.clone(),
        });
    app.update();
    let _ = receiver.try_iter().collect::<Vec<_>>();
    let accepted = accepted_adoption(app, receiver, &candidate);
    app.world_mut()
        .resource_mut::<SceneAssets>()
        .retire_for_terminal_reset();
    assert_eq!(
        app.world().resource::<SceneAssets>().adoption.pending(),
        1,
        "freeze keeps accepted mutation ownership until the database join"
    );
    let receipt = SceneAssetAdoptionReceipt {
        candidate: candidate.clone(),
        operation_id: "adopt-exact-copy".into(),
        adopter_did: "did:key:owner".into(),
        adopted_at: "fixture".into(),
    };
    app.world_mut().resource_mut::<LocalUserRole>().role = None;
    app.world_mut().write_message(DbResult::SceneAssetAdopted {
        request_id: accepted,
        result: Ok(receipt),
    });
    app.update();
    assert_eq!(app.world().resource::<SceneAssets>().adoption.pending(), 0);
    assert!(
        app.world()
            .resource::<SceneAssets>()
            .status
            .as_ref()
            .unwrap()
            .1
            .contains("Peer delivery is tracked separately")
    );
    assert_eq!(
        app.world().resource::<NavigationManager>().intent_revision,
        intent
    );

    app.world_mut().resource_mut::<LocalUserRole>().role = Some(fe_policy::RoleLevel::Owner);
    app.world_mut()
        .write_message(DbResult::ReplicatedNodeCurrent { current });
    app.update();
    let manager = app.world().resource::<VerseManager>();
    let nav = app.world().resource::<NavigationManager>();
    let nodes = app.world().resource::<NodeManager>();
    let role = app.world().resource::<LocalUserRole>();
    let now = app
        .world()
        .resource::<crate::notice::NoticeClock>()
        .elapsed_secs_f64();
    let frame = Frame {
        manager,
        nav,
        nodes,
        role,
        now,
    };
    let mut isolated = super::adoption::Adoption::default();
    let (full_tx, full_rx) = crossbeam::channel::bounded(1);
    full_tx.try_send(DbCommand::LoadHierarchy).unwrap();
    isolated.begin(&node, &frame).unwrap();
    isolated.pump(&frame, &full_tx);
    assert!(matches!(full_rx.try_recv(), Ok(DbCommand::LoadHierarchy)));
    isolated.pump(&frame, &full_tx);
    let id = read_id(&full_rx);
    let expired = Frame {
        now: now + 6.0,
        ..frame
    };
    isolated.route(
        &DbResult::SceneAssetAdoptionCandidate {
            node_id: node.clone(),
            request_id: id,
            result: Ok(candidate.clone()),
        },
        &expired,
    );
    assert!(
        !isolated.ready(),
        "deadline is checked at reply use even without an intervening pump"
    );
    isolated.begin(&node, &expired).unwrap();
    isolated.pump(&expired, &full_tx);
    let id = read_id(&full_rx);
    isolated.route(
        &DbResult::SceneAssetAdoptionCandidate {
            node_id: node.clone(),
            request_id: id,
            result: Ok(candidate.clone()),
        },
        &expired,
    );
    let id = isolated.preview().unwrap().id.clone();
    isolated.confirm(&id, &expired).unwrap();
    isolated.pump(&expired, &full_tx);
    let id = read_id(&full_rx);
    isolated.route(
        &DbResult::SceneAssetAdoptionCandidate {
            node_id: node.clone(),
            request_id: id,
            result: Ok(candidate.clone()),
        },
        &expired,
    );
    full_tx.try_send(DbCommand::LoadHierarchy).unwrap();
    isolated.pump(&expired, &full_tx);
    assert!(matches!(full_rx.try_recv(), Ok(DbCommand::LoadHierarchy)));
    let late = Frame {
        now: now + 12.0,
        ..expired
    };
    isolated.pump(&late, &full_tx);
    assert_eq!(
        isolated.pending(),
        0,
        "Full never turns an expired unsent candidate into an adoption"
    );
    assert!(full_rx.try_recv().is_err());
    let mut other_nav = NavigationManager::default();
    other_nav.navigate_to_petal_in_fractal("v1", "Verse", "f1", "Fractal", "p1");
    while other_nav.intent_revision == nav.intent_revision {
        other_nav.navigate_to_petal("p1");
    }
    isolated.begin(&node, &late).unwrap();
    isolated.pump(&late, &full_tx);
    let id = read_id(&full_rx);
    let new_intent = Frame {
        nav: &other_nav,
        ..late
    };
    isolated.route(
        &DbResult::SceneAssetAdoptionCandidate {
            node_id: node.clone(),
            request_id: id,
            result: Ok(candidate.clone()),
        },
        &new_intent,
    );
    assert!(
        !isolated.ready(),
        "same-Petal reentry cannot continue an earlier review"
    );
    let replacement = VerseManager::default();
    let replaced = Frame {
        manager: &replacement,
        ..new_intent
    };
    assert!(
        isolated.begin(&node, &replaced).is_err(),
        "a new manager cannot reuse a former admitted asset source"
    );
    let editor = LocalUserRole {
        role: Some(fe_policy::RoleLevel::Editor),
        ..Default::default()
    };
    let editor_frame = Frame {
        role: &editor,
        ..late
    };
    assert!(
        isolated.begin(&node, &editor_frame).is_err(),
        "Editor access cannot adopt a local copy"
    );
    crate::maintenance::clear_scene_asset_outcomes_after_database_join(app.world_mut());
    assert_eq!(app.world().resource::<SceneAssets>().pending(), 0);
    assert!(
        app.world()
            .resource::<UiManager>()
            .scene_asset_view
            .preview
            .is_none(),
        "the next derived view must not retain a cleared workflow"
    );
}
