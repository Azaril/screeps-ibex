use screeps::*;
use super::planner::*;

//
// Nodes
//

const LABS: &FixedPlanNode = &FixedPlanNode {
    id: uuid::Uuid::from_u128(0xd2d0_407f_9f30_4f98_9f40_8d1d_4c05_5981u128),
    must_place: false,
    placements: &[
        placement(StructureType::Lab, 0, 1),
        placement(StructureType::Lab, 0, 2),
        placement(StructureType::Lab, 1, 2),
        placement(StructureType::Lab, 1, 3),
        placement(StructureType::Lab, 2, 3),

        placement(StructureType::Lab, 1, 0),
        placement(StructureType::Lab, 2, 0),
        placement(StructureType::Lab, 2, 1),
        placement(StructureType::Lab, 3, 1),
        placement(StructureType::Lab, 3, 2),

        placement(StructureType::Road, 0, 0),
        placement(StructureType::Road, 1, 1),
        placement(StructureType::Road, 2, 2),
        placement(StructureType::Road, 3, 3),
    ],
    child: PlanNodeStorage::Empty,
    desires_placement: |_, state| state.get_count(StructureType::Lab) == 0 && state.get_count(StructureType::Storage) > 0,
    desires_location: |_, _, _| true,
    scorer: |_, _, _| Some(1.0)
};

const EXTENSION_CROSS: &FixedPlanNode = &FixedPlanNode {
    id: uuid::Uuid::from_u128(0x68fd_8e22_e7b9_46f4_b798_5efa_0924_8095u128),
    must_place: false,
    placements: &[
        placement(StructureType::Extension, 0, 0),
        placement(StructureType::Extension, 0, 1),
        placement(StructureType::Extension, 1, 0),
        placement(StructureType::Extension, 0, -1),
        placement(StructureType::Extension, -1, 0),

        placement(StructureType::Road, 0, -2),
        placement(StructureType::Road, -1, -1),
        placement(StructureType::Road, -2, 0),
        placement(StructureType::Road, -1, 1),
        placement(StructureType::Road, 0, 2),
        placement(StructureType::Road, 1, 1),
        placement(StructureType::Road, 2, 0),
        placement(StructureType::Road, 1, -1),
    ],
    child: PlanNodeStorage::Empty,
    desires_placement: |_, state| state.get_count(StructureType::Extension) <= 55 && state.get_count(StructureType::Storage) > 0,
    desires_location: |_, _, _| true,
    scorer: |location, context, state| {
        if location.in_room_bounds() {
            state.with_structure_distances(StructureType::Storage, context.terrain(), |storage_distances| {
                storage_distances
                    .and_then(|(distances, max_distance)| distances.get(location.x() as usize, location.y() as usize).map(|distance| (distance,  max_distance)))
                    .map(|(distance, max_distance)| 1.0 - (distance as f32 / max_distance as f32))
            })
        } else {
            None
        }
    }
};

const EXTENSION: &FixedPlanNode = &FixedPlanNode {
    id: uuid::Uuid::from_u128(0x7405_b6a1_f235_4f7a_b20e_c283_d19b_3e88u128),
    must_place: false,
    placements: &[
        placement(StructureType::Extension, 0, 0),

        placement(StructureType::Road, -1, -0),
        placement(StructureType::Road, 0, 1),
        placement(StructureType::Road, 1, 0),
        placement(StructureType::Road, 0, -1),
    ],
    child: PlanNodeStorage::Empty,
    desires_placement: |_, state| state.get_count(StructureType::Extension) < 60 && state.get_count(StructureType::Storage) > 0,
    desires_location: |_, _, _| true,
    scorer: |location, context, state| {
        if location.in_room_bounds() {
            state.with_structure_distances(StructureType::Storage, context.terrain(), |storage_distances| {
                storage_distances
                    .and_then(|(distances, max_distance)| distances.get(location.x() as usize, location.y() as usize).map(|distance| (distance, max_distance)))
                    .map(|(distance, max_distance)| 1.0 - (distance as f32 / max_distance as f32))
            })
        } else {
            None
        }
    }
};

const UTILITY_CROSS: &FixedPlanNode = &FixedPlanNode {
    id: uuid::Uuid::from_u128(0x03e1_1bc4_e469_44b0_80dc_1b88_88c2_616eu128),
    must_place: false,
    placements: &[
        placement(StructureType::Observer, 0, 0),
        placement(StructureType::Spawn, 0, 1),
        placement(StructureType::Factory, 1, 0),
        placement(StructureType::PowerSpawn, -1, 0),
        placement(StructureType::Spawn, 0, -1),

        placement(StructureType::Road, 0, -2),
        placement(StructureType::Road, -1, -1),
        placement(StructureType::Road, -2, 0),
        placement(StructureType::Road, -1, 1),
        placement(StructureType::Road, 0, 2),
        placement(StructureType::Road, 1, 1),
        placement(StructureType::Road, 2, 0),
        placement(StructureType::Road, 1, -1),
    ],
    child: PlanNodeStorage::Empty,
    desires_placement: |_, state| { 
        state.get_count(StructureType::Observer) == 0 && 
        state.get_count(StructureType::Spawn) <= 1 &&
        state.get_count(StructureType::Factory) == 0 &&
        state.get_count(StructureType::PowerSpawn) == 0        
    },
    desires_location: |_, _, _| true,
    scorer: |location, context, state| {
        if location.in_room_bounds() {
            state.with_structure_distances(StructureType::Storage, context.terrain(), |storage_distances| {
                storage_distances
                    .and_then(|(distances, max_distance)| distances.get(location.x() as usize, location.y() as usize).map(|distance| (distance,  max_distance)))
                    .map(|(distance, max_distance)| 1.0 - (distance as f32 / max_distance as f32))
            })
        } else {
            None
        }
    }
};

const SOURCES: PlanNodeStorage = PlanNodeStorage::GlobalExpansion(&FixedLocationPlanNode {
    locations: |context| {
        context.sources().to_vec()
    },
    child: PlanNodeStorage::LocationExpansion(&NearestToStructureExpansionPlanNode {
        structure_type: StructureType::Storage,
        allowed_offsets: ONE_OFFSET_SQUARE,
        child: SOURCE_CONTAINER,
        desires_placement: |_, _| true,
        desires_location: |_, _, _| true,
        scorer: |_, _, _| Some(1.0),
    })
});

const EXTRACTOR_CONTAINER: PlanNodeStorage = PlanNodeStorage::LocationPlacement(&FixedPlanNode {
    id: uuid::Uuid::from_u128(0x414d_d6b4_93f8_4539_81c5_89b5_1311_2a4fu128),
    must_place: true,
    placements: &[
        placement(StructureType::Container, 0, 0),
    ],
    child: PlanNodeStorage::Empty,
    desires_placement: |_context, state| {
        state.get_count(StructureType::Container) < 5
    },
    desires_location: |location, _context, state| {
        let mut extractor_locations = state.get_locations(StructureType::Extractor);
        let mut container_locations = state.get_locations(StructureType::Container);

        let mut matched_extractors = Vec::new();

        for (extractor_index, extractor_location) in extractor_locations.iter().enumerate() {
            if let Some(index) = container_locations.iter().position(|container_location| extractor_location.distance_to(*container_location) <= 1) {
                container_locations.remove(index);
                matched_extractors.push(extractor_index)
            }
        }

        for index in matched_extractors.iter().rev() {
            extractor_locations.remove(*index);
        }

        extractor_locations.iter().any(|extractor_location| location.distance_to(extractor_location.into()) <= 1)
    },
    scorer: |_, _, _| Some(1.0),
});

const EXTRACTOR: PlanNodeStorage = PlanNodeStorage::LocationPlacement(&FixedPlanNode {
    id: uuid::Uuid::from_u128(0x3726_8895_d11a_4aa4_9898_12a9_efc8_b968u128),
    must_place: true,
    placements: &[
        placement(StructureType::Extractor, 0, 0),
    ],
    child: PlanNodeStorage::LocationExpansion(&NearestToStructureExpansionPlanNode {
        structure_type: StructureType::Storage,
        allowed_offsets: ONE_OFFSET_SQUARE,
        child: EXTRACTOR_CONTAINER,
        desires_placement: |_, _| true,
        desires_location: |_, _, _| true,
        scorer: |_, _, _| Some(1.0),
    }),
    desires_placement: |context, state| (state.get_count(StructureType::Extractor) as usize) < context.minerals().len(),
    desires_location: |location, context, _| context.minerals().contains(&location),
    scorer: |_, _, _| Some(1.0),
});

const MINERALS_NODE: &FixedLocationPlanNode = &FixedLocationPlanNode {
    locations: |context| {
        context.minerals().to_vec()
    },
    child: EXTRACTOR
};

const MINERALS: PlanNodeStorage = PlanNodeStorage::GlobalExpansion(MINERALS_NODE);

const POST_BUNKER_NODES: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&MultiPlacementExpansionNode {
    children: &[SOURCES, MINERALS]
});

const BUNKER_CORE: PlanNodeStorage = PlanNodeStorage::LocationPlacement(&FixedPlanNode {
    id: uuid::Uuid::from_u128(0x1533_4930_d790_4a49_b1e0_1e30_acc4_eb46u128),
    must_place: false,
    placements: &[
        placement(StructureType::Spawn, -2, 0),
        
        placement(StructureType::Storage, 0, -1),

        placement(StructureType::Terminal, 1, 0),

        placement(StructureType::Link, -1, 1),

        placement(StructureType::Tower, -2, 1),
        placement(StructureType::Tower, -1, 2),
        placement(StructureType::Tower, -1, -2),
        placement(StructureType::Tower, 0, -2),
        placement(StructureType::Tower, 2, 0),
        placement(StructureType::Tower, 2, 1),

        placement(StructureType::Nuker, 1, -1),

        placement(StructureType::Extension, -2, -1),
        placement(StructureType::Extension, -3, 0),
        placement(StructureType::Extension, -3, 1),
        placement(StructureType::Extension, -4, 1),
        placement(StructureType::Extension, -3, 2),
        placement(StructureType::Extension, -2, 2),
        placement(StructureType::Extension, -2, 3),
        placement(StructureType::Extension, -1, 3),
        placement(StructureType::Extension, -1, 4),
        placement(StructureType::Extension, 0, 3),
        placement(StructureType::Extension, 0, 2),
        placement(StructureType::Extension, 1, 2),
        placement(StructureType::Extension, 0, -3),
        placement(StructureType::Extension, 1, -3),
        placement(StructureType::Extension, 1, -2),
        placement(StructureType::Extension, 2, -2),
        placement(StructureType::Extension, 2, -1),
        placement(StructureType::Extension, 3, -1),
        placement(StructureType::Extension, 3, 0),

        placement(StructureType::Road, -1, -1),
        placement(StructureType::Road, -1, 0),
        placement(StructureType::Road, 0, 0),
        placement(StructureType::Road, 0, 1),
        placement(StructureType::Road, 1, 1),
    ],
    child: PlanNodeStorage::LocationExpansion(&MultiPlacementExpansionNode {
        children: &[
            POST_BUNKER_NODES,
            PlanNodeStorage::LocationExpansion(&OffsetPlanNode {
                offsets: &[(-2, -2), (2, 2)],
                child: PlanNodeStorage::LocationPlacement(LABS)
            }),
            PlanNodeStorage::LocationPlacement(&FloodFillPlanNode {
                id: uuid::Uuid::from_u128(0xeff2_1b89_0149_4bc9_b4f4_8138_5cd6_5232u128),
                must_place: false,
                start_offsets: &[(-3, -3), (-1, -5), (-5, -1), (3, 3), (5, 1), (1, 5)],
                expansion_offsets: &[(-4, 0), (-2, 2), (0, 4), (2, 2), (4, 0), (2, -2), (0, -4), (-2, -2)],
                maximum_expansion: 20,
                maximum_nodes: 100,
                levels: &[FloodFillPlanNodeLevel {
                    offsets: &[(0, 0)],
                    node: UTILITY_CROSS,
                    node_cost: 0,
                },
                FloodFillPlanNodeLevel {
                    offsets: &[(0, 0)],
                    node: EXTENSION_CROSS,
                    node_cost: 5
                }, FloodFillPlanNodeLevel {
                    offsets: ONE_OFFSET_DIAMOND,
                    node: EXTENSION,
                    node_cost: 1
                }],
                desires_placement: |_, _| true,
                scorer: |_, _, _| Some(0.5)
            })
        ]
    }),
    desires_placement: |_, state| state.get_count(StructureType::Spawn) == 0,
    desires_location: |_, _, _| true,
    scorer: |_, _, _| Some(1.0),
});

const ROOT_BUNKER: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&MultiPlacementExpansionNode {
    children: &[
        BUNKER_CORE
    ]
});

const SOURCE_LINK: PlanNodeStorage = PlanNodeStorage::LocationPlacement(&FixedPlanNode {
    id: uuid::Uuid::from_u128(0xc551_f09c_70d8_4148_a6a0_23af_6d95_e1bcu128),
    must_place: true,
    placements: &[
        placement(StructureType::Link, 0, 0),
    ],
    child: PlanNodeStorage::Empty,
    desires_placement: |_context, state| state.get_count(StructureType::Link) < 6,
    desires_location: |location, _context, state| {
        let link_locations = state.get_locations(StructureType::Link);
        let container_locations = state.get_locations(StructureType::Container);

        let matching_containers: Vec<_> = container_locations
            .iter()
            .filter(|&container_location| location.distance_to(container_location.into()) <= 1)
            .collect();

        matching_containers.iter().any(|container_location| !link_locations.iter().any(|link_location| link_location.distance_to(**container_location) <= 1))
    },
    scorer: |_, _, _| Some(1.0),
});

const SOURCE_CONTAINER: PlanNodeStorage = PlanNodeStorage::LocationPlacement(&FixedPlanNode {
    id: uuid::Uuid::from_u128(0x865a_77b5_df18_418f_826f_e3d4_e934_4bd6u128),
    must_place: true,
    placements: &[
        placement(StructureType::Container, 0, 0),
    ],
    child: PlanNodeStorage::LocationExpansion(&NearestToStructureExpansionPlanNode {
        structure_type: StructureType::Storage,
        allowed_offsets: ONE_OFFSET_SQUARE,
        child: SOURCE_LINK,
        desires_placement: |_, _| true,
        desires_location: |location, context, state| {
            if location.in_room_bounds() {
                state.with_structure_distances(StructureType::Storage, context.terrain(), |storage_distances| {
                    storage_distances.and_then(|(distances, _max_distance)| distances.get(location.x() as usize, location.y() as usize).map(|distance| distance >= 8)).unwrap_or(false)
                })
            } else {
                false
            }
        },
        scorer: |_, _, _| Some(1.0),
    }),
    desires_placement: |_context, state| state.get_count(StructureType::Container) < 5,
    desires_location: |location, context, state| {
        let mut source_locations = context.sources().to_vec();
        let mut container_locations = state.get_locations(StructureType::Container);

        let mut matched_sources = Vec::new();

        for (source_index, source_location) in source_locations.iter().enumerate() {
            if let Some(index) = container_locations.iter().position(|container_location| source_location.distance_to(container_location.into()) <= 1) {
                container_locations.remove(index);
                matched_sources.push(source_index)
            }
        }

        for index in matched_sources.iter().rev() {
            source_locations.remove(*index);
        }

        source_locations.iter().any(|source_location| location.distance_to(*source_location) <= 1)
    },
    scorer: |_, _, _| Some(1.0),
});

pub const ALL_ROOT_NODES: &[&dyn PlanGlobalExpansionNode] = &[
    &PlaceAwayFromWallsNode {
        wall_distance: 4,
        child: ROOT_BUNKER
    }
];