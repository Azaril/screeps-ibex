use super::planner::*;
use crate::constants::*;
use itertools::*;
use screeps::*;
use std::convert::*;

struct StateScore {
    score: f32,
    weight: f32,
}

fn has_mandatory_buildings(state: &PlannerState, context: &mut NodeContext) -> bool {
    state.get_count(StructureType::Spawn) >= 3
        && state.get_count(StructureType::Extension) >= 60
        && state.get_count(StructureType::Storage) >= 1
        && state.get_count(StructureType::Terminal) >= 1
        && state.get_count(StructureType::Lab) >= 10
        && state.get_count(StructureType::Factory) >= 1
        && state.get_count(StructureType::Observer) >= 1
        && state.get_count(StructureType::PowerSpawn) >= 1
        && state.get_count(StructureType::Nuker) >= 1
        && state.get_count(StructureType::Tower) >= 6
        && (state.get_count(StructureType::Extractor) as usize) == context.minerals().len()
}

fn has_source_containers(state: &PlannerState, context: &mut NodeContext) -> bool {
    let mut source_locations = context.sources().to_vec();
    let mut container_locations = state.get_locations(StructureType::Container);

    let mut matched_sources = Vec::new();

    for (source_index, source_location) in source_locations.iter().enumerate() {
        if let Some(index) = container_locations
            .iter()
            .position(|container_location| source_location.distance_to(container_location.into()) <= 1)
        {
            container_locations.remove(index);
            matched_sources.push(source_index)
        }
    }

    for index in matched_sources.iter().rev() {
        source_locations.remove(*index);
    }

    source_locations.is_empty()
}

fn has_source_links(state: &PlannerState, context: &mut NodeContext) -> bool {
    let source_locations = context.sources().to_vec();
    let link_locations = state.get_locations(StructureType::Link);
    let container_locations = state.get_locations(StructureType::Container);

    let matching_containers = state.with_structure_distances(StructureType::Storage, context.terrain(), |storage_distances| {
        if let Some((storage_distances, _max_distance)) = storage_distances {
            container_locations
                .iter()
                .filter(|&container_location| {
                    source_locations
                        .iter()
                        .any(|source_location| source_location.distance_to(container_location.into()) <= 1)
                })
                .filter(|&container_location| {
                    storage_distances
                        .get(container_location.x() as usize, container_location.y() as usize)
                        .map(|d| d >= 8)
                        .unwrap_or(false)
                })
                .collect()
        } else {
            Vec::new()
        }
    });

    !matching_containers.iter().any(|&container_location| {
        !link_locations
            .iter()
            .any(|link_location| link_location.distance_to(*container_location) <= 1)
    })
}

fn has_mineral_extractors(state: &PlannerState, context: &mut NodeContext) -> bool {
    let mineral_locations = context.minerals();
    let extractor_locations = state.get_locations(StructureType::Extractor);

    mineral_locations.iter().all(|mineral_location| {
        if let Ok(mineral_location) = mineral_location.try_into() {
            extractor_locations.contains(&mineral_location)
        } else {
            false
        }
    })
}

fn has_mineral_containers(state: &PlannerState, _context: &mut NodeContext) -> bool {
    let mut extractor_locations = state.get_locations(StructureType::Extractor);
    let mut container_locations = state.get_locations(StructureType::Container);

    let mut matched_extractors = Vec::new();

    for (extractor_index, extractor_location) in extractor_locations.iter().enumerate() {
        if let Some(index) = container_locations
            .iter()
            .position(|container_location| extractor_location.distance_to(*container_location) <= 1)
        {
            container_locations.remove(index);
            matched_extractors.push(extractor_index)
        }
    }

    for index in matched_extractors.iter().rev() {
        extractor_locations.remove(*index);
    }

    extractor_locations.is_empty()
}

fn has_reachable_structures(state: &PlannerState, context: &mut NodeContext) -> bool {
    let placements: Vec<_> = state.get_all().collect();

    state
        .with_structure_distances(StructureType::Storage, context.terrain(), |storage_distances| {
            storage_distances.map(|(distances, _max_distance)| {
                for (location, item) in placements.iter() {
                    let reachability_range: i8 = match item.structure_type() {
                        StructureType::Wall => 3,
                        _ => 1,
                    };

                    let mut found_reach = false;

                    for x in -reachability_range..=reachability_range {
                        for y in -reachability_range..=reachability_range {
                            let position = (location.x() as i8 + x, location.y() as i8 + y);

                            if position.in_room_bounds() && distances.get(position.0 as usize, position.1 as usize).is_some() {
                                found_reach = true;

                                break;
                            }
                        }

                        if found_reach {
                            break;
                        }
                    }

                    if !found_reach {
                        return false;
                    }
                }

                true
            })
        })
        .unwrap_or(false)
}

fn has_reachable_sources(state: &PlannerState, context: &mut NodeContext) -> bool {
    let sources = context.sources().to_vec();

    state.with_structure_distances(StructureType::Storage, context.terrain(), |storage_distances| {
        if let Some((storage_distances, _max_distance)) = storage_distances {
            sources.iter().all(|source_location| {
                ONE_OFFSET_SQUARE.iter().any(|offset| {
                    let offset_location = *source_location + offset;

                    if offset_location.in_room_bounds() {
                        storage_distances
                            .get(offset_location.x() as usize, offset_location.y() as usize)
                            .is_some()
                    } else {
                        false
                    }
                })
            })
        } else {
            false
        }
    })
}

fn source_distance_score(state: &PlannerState, context: &mut NodeContext) -> Vec<StateScore> {
    let sources = context.sources().to_vec();

    let source_distances = state.with_structure_distances(StructureType::Storage, context.terrain(), |storage_distances| {
        if let Some((storage_distances, max_distance)) = storage_distances {
            sources
                .iter()
                .filter_map(|source_location| {
                    ONE_OFFSET_SQUARE
                        .iter()
                        .filter_map(|offset| {
                            let offset_location = *source_location + offset;

                            if offset_location.in_room_bounds() {
                                *storage_distances.get(offset_location.x() as usize, offset_location.y() as usize)
                            } else {
                                None
                            }
                        })
                        .min_by_key(|distance| *distance)
                        .map(|distance| (distance, max_distance))
                })
                .collect()
        } else {
            Vec::new()
        }
    });

    let mut scores = Vec::new();

    for (storage_distance, max_distance) in source_distances.iter() {
        let source_score = 1.0 - (*storage_distance as f32 / *max_distance as f32);

        scores.push(StateScore {
            score: source_score,
            weight: 3.0,
        })
    }

    scores
}

fn source_distance_balance_score(state: &PlannerState, context: &mut NodeContext) -> Vec<StateScore> {
    let mut scores = Vec::new();

    let storage_locations = state.get_locations(StructureType::Storage);

    let source_distances: Vec<_> = context
        .source_distances()
        .iter()
        .filter_map(|(data, max_distance)| {
            let storage_distance = storage_locations
                .iter()
                .filter_map(|location| *data.get(location.x() as usize, location.y() as usize))
                .min();

            storage_distance.map(|distance| (distance, *max_distance))
        })
        .collect();

    if source_distances.len() > 1 {
        let source_delta_score: f32 = source_distances
            .iter()
            .map(|(storage_distance, _)| storage_distance)
            .combinations(2)
            .map(|items| {
                let delta = ((*items[0] as i32) - (*items[1] as i32)).abs();

                1.0 - ((delta as f32) / (ROOM_WIDTH.max(ROOM_HEIGHT) as f32))
            })
            .product();

        scores.push(StateScore {
            score: source_delta_score,
            weight: 1.0,
        })
    }

    scores
}

fn extension_distance_score(state: &PlannerState, _context: &mut NodeContext) -> Vec<StateScore> {
    let storage_locations = state.get_locations(StructureType::Storage);
    let extension_locations = state.get_locations(StructureType::Extension);

    //TODO: Use storage distance flood fill for accurate extension distance.

    let total_extension_distance: f32 = extension_locations
        .iter()
        .map(|extension| {
            storage_locations
                .iter()
                .map(|storage| storage.distance_to(*extension))
                .min()
                .unwrap() as f32
        })
        .sum();

    let average_distance = total_extension_distance / (extension_locations.len() as f32);
    let average_distance_score = 1.0 - (average_distance / ROOM_WIDTH.max(ROOM_HEIGHT) as f32);

    vec![StateScore {
        score: average_distance_score,
        weight: 1.0,
    }]
}

pub fn score_state(state: &PlannerState, context: &mut NodeContext) -> Option<f32> {
    //TODO: Add more validators.
    /*
        Validators needed:
        - Link is within pathable range 2 of storage.
        - Terminal is within pathable range 2 of storage.
    */

    //
    // NOTE: Order these from cheapest to most expensive for faster rejection.
    //

    let validators = [
        has_mandatory_buildings,
        has_source_containers,
        has_mineral_extractors,
        has_mineral_containers,
        has_source_links,
        has_reachable_structures,
        has_reachable_sources,
    ];

    if !validators.iter().all(|v| (v)(state, context)) {
        return None;
    }

    //TODO: Add more scoring.
    /*
        Scoring needed:
        - Mineral to storage length.
        - Controller to storage length.
    */

    let scorers = [source_distance_score, source_distance_balance_score, extension_distance_score];

    let weights: Vec<_> = scorers.iter().flat_map(|scorer| (scorer)(state, context)).collect();

    let total_score: f32 = weights.iter().map(|s| s.score).sum();
    let total_weight: f32 = weights.iter().map(|s| s.weight).sum();

    if total_weight > 0.0 {
        let score = total_score / total_weight;

        Some(score)
    } else {
        None
    }
}
