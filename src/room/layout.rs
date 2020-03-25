use screeps::*;
use super::planner::*;

//
// Patterns
//

const ONE_OFFSET_SQUARE: &[(i8, i8)] = &[(-1, -1), (-1, 0), (-1, 1), (0, 1), (1, 1), (1, 0), (1, -1), (0, -1)];
const TWO_OFFSET_SQUARE: &[(i8, i8)] = &[(-2, -2), (-2, -1), (-2, 0), (-2, 1), (-2, 2), (-1, 2), (0, 2), (1, 2), (2, 2), (2, 1), (2, 0), (2, -1), (2, -2), (1, -2), (0, -2), (-1, -2)];

const ONE_OFFSET_DIAMOND: &[(i8, i8)] = &[(-1, 0), (0, 1), (1, 0), (-1, 0)];
const TWO_OFFSET_DIAMOND: &[(i8, i8)] = &[(0, -2), (-1, -1), (-2, 0), (-1, 1), (0, 2), (1, 1), (2, 0), (1, -1)];
const TWO_OFFSET_DIAMOND_POINTS: &[(i8, i8)] = &[(0, -2), (-2, 0), (0, 2), (2, 0)];

//
// Nodes
//

const ALL_NODES: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&MultiPlanNode {
    children: &[STORAGE]
});

const ALL_NODES_LAZY: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&LazyPlanNode {
    child: || ALL_NODES,
});

const ALL_NODES_ONE_OFFSET_SQUARE: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&OffsetPlanNode { 
    offsets: ONE_OFFSET_SQUARE,
    child: ALL_NODES_LAZY
});

const ALL_NODES_TWO_OFFSET_SQUARE: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&OffsetPlanNode { 
    offsets: TWO_OFFSET_SQUARE,
    child: ALL_NODES_LAZY
});

const ALL_NODES_ONE_OFFSET_DIAMOND: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&OffsetPlanNode { 
    offsets: ONE_OFFSET_DIAMOND,
    child: ALL_NODES_LAZY
});

const ALL_NODES_TWO_OFFSET_DIAMOND: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&OffsetPlanNode { 
    offsets: TWO_OFFSET_DIAMOND,
    child: ALL_NODES_LAZY
});

const ALL_NODES_TWO_OFFSET_DIAMOND_POINTS: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&OffsetPlanNode { 
    offsets: TWO_OFFSET_DIAMOND_POINTS,
    child: ALL_NODES_LAZY
});

const EXTENSION_CROSS: &FixedPlanNode = &FixedPlanNode {
    id: uuid::Uuid::from_u128(0x68fd_8e22_e7b9_46f4_b798_5efa_0924_8095u128),
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
    scorer: |location, _, state| {
        let storage_locations = state.get_locations(StructureType::Storage);

        storage_locations
            .iter()
            .map(|storage| storage.distance_to_xy(location.x(), location.y()))
            .min()
            .map(|d| {
                1.0 - (d as f32 / ROOM_WIDTH.max(ROOM_HEIGHT) as f32)
            })
    }
};

const EXTENSION: &FixedPlanNode = &FixedPlanNode {
    id: uuid::Uuid::from_u128(0x7405_b6a1_f235_4f7a_b20e_c283_d19b_3e88u128),
    placements: &[
        placement(StructureType::Extension, 0, 0),

        placement(StructureType::Road, -1, -0),
        placement(StructureType::Road, 0, 1),
        placement(StructureType::Road, 1, 0),
        placement(StructureType::Road, 0, -1),
    ],
    child: PlanNodeStorage::Empty,
    desires_placement: |_, state| state.get_count(StructureType::Extension) < 60 && state.get_count(StructureType::Storage) > 0,
    scorer: |location, _, state| {
        let storage_locations = state.get_locations(StructureType::Storage);

        storage_locations
            .iter()
            .map(|storage| storage.distance_to_xy(location.x(), location.y()))
            .min()
            .map(|d| {
                1.0 - (d as f32 / ROOM_WIDTH.max(ROOM_HEIGHT) as f32)
            })
    }
};

const STORAGE: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&OffsetPlanNode {
    offsets: ONE_OFFSET_DIAMOND,
    child: PlanNodeStorage::LocationPlacement(&FixedPlanNode {
        id: uuid::Uuid::from_u128(0x7f7e_e145_d350_4aa1_9493_0c7c_ecb3_26cdu128),
        placements: &[
            placement(StructureType::Storage, 0, 0),

            placement(StructureType::Road, -1, 0),
            placement(StructureType::Road, 0, 1),
            placement(StructureType::Road, 1, 0),
            placement(StructureType::Road, 0, -1),
        ],
        child: PlanNodeStorage::LocationExpansion(&OffsetPlanNode {
            offsets: ONE_OFFSET_DIAMOND,
            child: PlanNodeStorage::LocationExpansion(&MultiPlanNode {
                children: &[TERMINAL, STORAGE_LINK]
            })
        }),
        desires_placement: |_, state| state.get_count(StructureType::Storage) == 0,
        scorer: |location, _, state| {
            const MAX_DISTANCE: u8 = 2;

            let mut score = 1.0;

            let spawn_locations = state.get_locations(StructureType::Spawn);
            let spawn_distance = spawn_locations.iter().map(|spawn| spawn.distance_to_xy(location.x(), location.y())).min().filter(|d| *d <= MAX_DISTANCE)?;

            score *= 1.0 - (spawn_distance as f32 / MAX_DISTANCE as f32);
            
            let terminal_locations = state.get_locations(StructureType::Terminal);
            if !terminal_locations.is_empty() {
                let terminal_distance = terminal_locations.iter().map(|terminal| terminal.distance_to_xy(location.x(), location.y())).min().filter(|d| *d <= MAX_DISTANCE)?;
            
                score *= 1.0 - (terminal_distance as f32 / MAX_DISTANCE as f32);
            }

            let link_locations = state.get_locations(StructureType::Link);
            if !link_locations.is_empty() {
                let link_distance = link_locations.iter().map(|terminal| terminal.distance_to_xy(location.x(), location.y())).min().filter(|d| *d <= MAX_DISTANCE)?;
            
                score *= 1.0 - (link_distance as f32 / MAX_DISTANCE as f32);
            }

            Some(score)
        }
    })
});

const TERMINAL: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&OffsetPlanNode {
    offsets: ONE_OFFSET_DIAMOND,
    child: PlanNodeStorage::LocationPlacement(&FixedPlanNode {
        id: uuid::Uuid::from_u128(0x8213_221e_29f3_4325_b333_79fa_a5e2_b8e8),
        placements: &[
            placement(StructureType::Terminal, 0, 0),

            placement(StructureType::Road, -1, 0),
            placement(StructureType::Road, 0, 1),
            placement(StructureType::Road, 1, 0),
            placement(StructureType::Road, 0, -1),
        ],
        child: ALL_NODES_ONE_OFFSET_DIAMOND,
        desires_placement: |_, state| state.get_count(StructureType::Terminal) == 0,
        scorer: |location, _, state| {
            const MAX_DISTANCE: u8 = 2;

            let mut score = 1.0;

            let storage_locations = state.get_locations(StructureType::Storage);
            if !storage_locations.is_empty() {
                let storage_distance = storage_locations.iter().map(|spawn| spawn.distance_to_xy(location.x(), location.y())).min().filter(|d| *d <= MAX_DISTANCE)?;
            
                score *= 1.0 - (storage_distance as f32 / MAX_DISTANCE as f32);
            }

            let link_locations = state.get_locations(StructureType::Link);
            if !link_locations.is_empty() {
                let link_distance = link_locations.iter().map(|terminal| terminal.distance_to_xy(location.x(), location.y())).min().filter(|d| *d <= MAX_DISTANCE)?;
            
                score *= 1.0 - (link_distance as f32 / MAX_DISTANCE as f32);
            }

            Some(score)
        }
    })
});

const STORAGE_LINK: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&OffsetPlanNode {
    offsets: ONE_OFFSET_DIAMOND,
    child: PlanNodeStorage::LocationPlacement(&FixedPlanNode {
        id: uuid::Uuid::from_u128(0xacd2_b536_5666_48d7_b9de_97eb_b687_5d74u128),
        placements: &[
            placement(StructureType::Link, 0, 0),

            placement(StructureType::Road, -1, 0),
            placement(StructureType::Road, 0, 1),
            placement(StructureType::Road, 1, 0),
            placement(StructureType::Road, 0, -1),
        ],
        child: ALL_NODES_ONE_OFFSET_DIAMOND,
        desires_placement: |_, state| state.get_count(StructureType::Link) == 0,
        scorer: |location, _, state| {
            const MAX_DISTANCE: u8 = 2;

            let mut score = 1.0;

            let storage_locations = state.get_locations(StructureType::Storage);
            if !storage_locations.is_empty() {
                let storage_distance = storage_locations.iter().map(|spawn| spawn.distance_to_xy(location.x(), location.y())).min().filter(|d| *d <= MAX_DISTANCE)?;

                score *= 1.0 - (storage_distance as f32 / MAX_DISTANCE as f32);
            }

            let terminal_locations = state.get_locations(StructureType::Terminal);
            if !terminal_locations.is_empty() {
                let terminal_distance = terminal_locations.iter().map(|terminal| terminal.distance_to_xy(location.x(), location.y())).min().filter(|d| *d <= MAX_DISTANCE)?;
            
                score *= 1.0 - (terminal_distance as f32 / MAX_DISTANCE as f32);
            }

            Some(score)
        }
    })
});

const SPAWN: PlanNodeStorage = PlanNodeStorage::LocationPlacement(&FixedPlanNode {
    id: uuid::Uuid::from_u128(0x1533_4930_d790_4a49_b1e0_1e30_acc4_eb46u128),
    placements: &[
        placement(StructureType::Spawn, 0, 0),

        placement(StructureType::Road, -1, 0),
        placement(StructureType::Road, 0, 1),
        placement(StructureType::Road, 1, 0),
        placement(StructureType::Road, 0, -1),
    ],
    child: PlanNodeStorage::Empty,
    desires_placement: |_, state| state.get_count(StructureType::Spawn) == 0,
    scorer: |_, _, _| Some(0.0),
});

const BUNKER_CORE: PlanNodeStorage = PlanNodeStorage::LocationPlacement(&FixedPlanNode {
    id: uuid::Uuid::from_u128(0x1533_4930_d790_4a49_b1e0_1e30_acc4_eb46u128),
    placements: &[
        placement(StructureType::Spawn, -2, 0),
        
        placement(StructureType::Storage, 0, -1),

        placement(StructureType::Terminal, 1, 0),

        placement(StructureType::Link, -1, 1),

        placement(StructureType::Tower, -2, 1),
        placement(StructureType::Tower, -1, 2),
        placement(StructureType::Tower, -1, -2),
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
        placement(StructureType::Extension, 0, -2),
        placement(StructureType::Extension, 0, -3),
        placement(StructureType::Extension, 1, -3),
        placement(StructureType::Extension, 1, -2),
        placement(StructureType::Extension, 2, -2),
        placement(StructureType::Extension, 2, -1),
        placement(StructureType::Extension, 3, -1),
        placement(StructureType::Extension, 3, 0),
    ],
    child: PlanNodeStorage::LocationExpansion(&MultiPlanNode {
        children: &[PlanNodeStorage::LocationPlacement(&FloodFillPlanNode {
            id: uuid::Uuid::from_u128(0xeff2_1b89_0149_4bc9_b4f4_8138_5cd6_5232u128),
            start_offsets: &[(-3, -3), (3, 3)],
            expansion_offsets: &[(-4, 0), (-2, 2), (0, 4), (2, 2), (4, 0), (2, -2), (0, -4), (-2, -2)],
            maximum_expansion: 20,
            maximum_nodes: 60,
            levels: &[FloodFillPlanNodeLevel {
                offsets: &[(0, 0)],
                node: EXTENSION_CROSS,
                node_cost: 5
            }, FloodFillPlanNodeLevel {
                offsets: ONE_OFFSET_DIAMOND,
                node: EXTENSION,
                node_cost: 1
            }],
            desires_placement: |_, _| true,
            scorer: |_, _, _| Some(0.5),
        })]
    }),
    desires_placement: |_, state| state.get_count(StructureType::Spawn) == 0,
    scorer: |_, _, _| Some(1.0),
});

const ROOT_BUNKER_NODE: PlanNodeStorage = PlanNodeStorage::LocationExpansion(&MultiPlanNode {
    children: &[
        BUNKER_CORE
    ]
});

pub const ALL_ROOT_NODES: &[&dyn PlanGlobalExpansionNode] = &[
    &PlaceAwayFromWallsNode {
        wall_distance: 4,
        child: ROOT_BUNKER_NODE
    }
];