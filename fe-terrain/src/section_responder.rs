//! Answers `SectionProfileRequest` with a sampled existing-vs-proposed section; see `src/AGENTS.md` §profile.

use bevy::prelude::*;
use fe_renderer::section_profile::{SectionProfileRequest, SectionProfileResult};
use fe_renderer::terrain_height::TerrainHeightField;
use fe_renderer::terrain_overlay::VolumeUnavailable;

use crate::petal_binding::ActivePetalTerrain;
use crate::profile::{sample_profile, PROFILE_MAX_SAMPLES};
use crate::sculpt::{composite_proposed_height, prepare_composite};
use crate::terrain_plugin::config_earthwork_regions;
use crate::terrain_proposal::{apply_proposal_over_base, point_in_polygon};

/// Reply once per request; an unservable request gets an `unavailable` result, never silence.
pub fn answer_section_profile_requests(
    mut requests: MessageReader<SectionProfileRequest>,
    active: Res<ActivePetalTerrain>,
    height_field: Option<Res<TerrainHeightField>>,
    mut results: MessageWriter<SectionProfileResult>,
) {
    if requests.is_empty() {
        return;
    }
    for request in requests.read() {
        let reply = if active.petal_id.as_deref() != Some(request.petal_id.as_str()) {
            unavailable_result(request, VolumeUnavailable::WrongPetal)
        } else if let Some(config) = active.config.as_ref().filter(|config| config.enabled) {
            let base = |x: f32, z: f32| height_field.as_ref().and_then(|field| field.height_at(x, z));
            let proposals: Vec<_> = config
                .proposals
                .iter()
                .filter(|proposal| !proposal.is_earthwork_region())
                .cloned()
                .collect();
            let regions = config_earthwork_regions(config);
            let composite = prepare_composite(&regions, &base);
            // Same combined surface as the volume report: palette proposals, then earthwork regions.
            let proposed = |x: f32, z: f32| {
                let mut height = base(x, z)?;
                for proposal in &proposals {
                    if point_in_polygon([x, z], &proposal.footprint) {
                        let current = height;
                        if let Some(next) =
                            apply_proposal_over_base(&|_, _| Some(current), proposal, &[[x, z]])
                                .into_iter()
                                .next()
                                .flatten()
                        {
                            height = next;
                        }
                    }
                }
                Some(composite_proposed_height(&composite, height, x, z))
            };
            match sample_profile(
                &base,
                &proposed,
                request.a,
                request.b,
                PROFILE_MAX_SAMPLES,
                config.effective_world_scale(),
            ) {
                Some(profile) => SectionProfileResult {
                    correlation_id: request.correlation_id.clone(),
                    petal_id: request.petal_id.clone(),
                    samples: profile
                        .samples
                        .iter()
                        .map(|sample| [sample.distance, sample.base_y, sample.proposed_y])
                        .collect(),
                    length: profile.length,
                    step: profile.step,
                    cut_area: profile.cut_area,
                    fill_area: profile.fill_area,
                    scaled: profile.scaled,
                    unavailable: None,
                },
                // Fewer than two covered stations: no coverage along the line.
                None => unavailable_result(request, VolumeUnavailable::NoMap),
            }
        } else {
            unavailable_result(request, VolumeUnavailable::NoMap)
        };
        results.write(reply);
    }
}

/// Empty reply telling the panel why the section cannot be drawn.
fn unavailable_result(
    request: &SectionProfileRequest,
    reason: VolumeUnavailable,
) -> SectionProfileResult {
    SectionProfileResult {
        correlation_id: request.correlation_id.clone(),
        petal_id: request.petal_id.clone(),
        samples: Vec::new(),
        length: 0.0,
        step: 0.0,
        cut_area: 0.0,
        fill_area: 0.0,
        scaled: false,
        unavailable: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TerrainConfig;
    use crate::terrain_proposal::{TerrainOp, TerrainProposal};
    use fe_renderer::terrain_height::HeightTile;

    #[derive(Resource, Default)]
    struct CapturedSections(Vec<SectionProfileResult>);

    fn capture_sections(
        mut reader: MessageReader<SectionProfileResult>,
        mut sink: ResMut<CapturedSections>,
    ) {
        sink.0.extend(reader.read().cloned());
    }

    fn request(id: &str, petal: &str) -> SectionProfileRequest {
        SectionProfileRequest {
            correlation_id: id.into(),
            petal_id: petal.into(),
            a: [-5.0, 5.0],
            b: [15.0, 5.0],
        }
    }

    // Raise 2 across a 10-wide footprint fills ~20 m2; every request is answered, none dropped.
    #[test]
    fn section_request_round_trip_answers_fill_and_unavailable() {
        let raise = TerrainProposal {
            id: "p1".into(),
            op: TerrainOp::Raise,
            footprint: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            target_height: None,
            delta: Some(2.0),
            material: None,
        };
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<CapturedSections>()
            .insert_resource(TerrainHeightField::default())
            .insert_resource(ActivePetalTerrain {
                raw_source_fingerprint: None,
                assignment_fingerprint: None,
                input_scale_valid: false,
                petal_id: Some("petal".into()),
                config: Some(TerrainConfig {
                    enabled: true,
                    proposals: vec![raise],
                    ..TerrainConfig::default()
                }),
                revision: 1,
            })
            .add_message::<SectionProfileRequest>()
            .add_message::<SectionProfileResult>()
            .add_systems(
                Update,
                (answer_section_profile_requests, capture_sections).chain(),
            );
        app.world_mut()
            .resource_mut::<TerrainHeightField>()
            .insert(
                (10, 0, 0),
                HeightTile {
                    anchor: Vec3::new(-10.0, 0.0, -10.0),
                    tile_size: 40.0,
                    grid: vec![0.0; 4],
                    width: 2,
                    height: 2,
                },
            );

        app.update();
        assert!(app.world().resource::<CapturedSections>().0.is_empty());

        app.world_mut().write_message(request("corr-1", "petal"));
        app.update();
        {
            let results = &app.world().resource::<CapturedSections>().0;
            assert_eq!(results.len(), 1, "exactly one reply per request");
            let result = &results[0];
            assert_eq!(result.correlation_id, "corr-1");
            assert!(result.unavailable.is_none() && result.scaled);
            assert!(
                (result.fill_area - 20.0).abs() < 1.0,
                "raise 2 across 10 wide fills ~20, got {}",
                result.fill_area
            );
            assert!(result.cut_area.abs() < 1e-6, "a pure raise has no cut");
        }

        app.world_mut().write_message(request("corr-wrong", "other"));
        app.update();
        let wrong = app.world().resource::<CapturedSections>().0.last().unwrap();
        assert_eq!(wrong.correlation_id, "corr-wrong");
        assert_eq!(wrong.unavailable, Some(VolumeUnavailable::WrongPetal));

        app.world_mut().resource_mut::<ActivePetalTerrain>().config = None;
        app.world_mut().write_message(request("corr-no-map", "petal"));
        app.update();
        let results = &app.world().resource::<CapturedSections>().0;
        assert_eq!(results.len(), 3, "every request answered exactly once");
        let no_map = results.last().unwrap();
        assert_eq!(no_map.correlation_id, "corr-no-map");
        assert_eq!(no_map.unavailable, Some(VolumeUnavailable::NoMap));
        assert!(no_map.samples.is_empty());
    }
}
