use super::memorysystem::*;
use crate::room::data::*;
use screeps::*;
use serde::ser::SerializeMap;
use serde::*;
use shrinkwraprs::*;
use specs::prelude::*;
use std::collections::HashMap;

#[derive(Serialize)]
pub struct CpuStats {
    bucket: u32,
    limit: u32,
    used: f64,
}

fn to_structure_name(val: StructureType) -> &'static str {
    match val {
        StructureType::Spawn => "spawn",
        StructureType::Extension => "extension",
        StructureType::Road => "road",
        StructureType::Wall => "wall",
        StructureType::Rampart => "rampart",
        StructureType::KeeperLair => "keeperLair",
        StructureType::Portal => "portal",
        StructureType::Controller => "controller",
        StructureType::Link => "link",
        StructureType::Storage => "storage",
        StructureType::Tower => "tower",
        StructureType::Observer => "observer",
        StructureType::PowerBank => "powerBank",
        StructureType::PowerSpawn => "powerSpawn",
        StructureType::Extractor => "extractor",
        StructureType::Lab => "lab",
        StructureType::Terminal => "terminal",
        StructureType::Container => "container",
        StructureType::Nuker => "nuker",
        StructureType::Factory => "factory",
        StructureType::InvaderCore => "invaderCore",
        _ => "unknown",
    }
}

fn to_resource_name(val: ResourceType) -> &'static str {
    match val {
        ResourceType::Energy => "energy",
        ResourceType::Power => "power",
        ResourceType::Hydrogen => "H",
        ResourceType::Oxygen => "O",
        ResourceType::Utrium => "U",
        ResourceType::Lemergium => "L",
        ResourceType::Keanium => "K",
        ResourceType::Zynthium => "Z",
        ResourceType::Catalyst => "X",
        ResourceType::Ghodium => "G",
        ResourceType::Hydroxide => "OH",
        ResourceType::ZynthiumKeanite => "ZK",
        ResourceType::UtriumLemergite => "UL",
        ResourceType::UtriumHydride => "UH",
        ResourceType::UtriumOxide => "UO",
        ResourceType::KeaniumHydride => "KH",
        ResourceType::KeaniumOxide => "KO",
        ResourceType::LemergiumHydride => "LH",
        ResourceType::LemergiumOxide => "LO",
        ResourceType::ZynthiumHydride => "ZH",
        ResourceType::ZynthiumOxide => "ZO",
        ResourceType::GhodiumHydride => "GH",
        ResourceType::GhodiumOxide => "GO",
        ResourceType::UtriumAcid => "UH2O",
        ResourceType::UtriumAlkalide => "UHO2",
        ResourceType::KeaniumAcid => "KH2O",
        ResourceType::KeaniumAlkalide => "KHO2",
        ResourceType::LemergiumAcid => "LH2O",
        ResourceType::LemergiumAlkalide => "LHO2",
        ResourceType::ZynthiumAcid => "ZH2O",
        ResourceType::ZynthiumAlkalide => "ZHO2",
        ResourceType::GhodiumAcid => "GH2O",
        ResourceType::GhodiumAlkalide => "GHO2",
        ResourceType::CatalyzedUtriumAcid => "XUH2O",
        ResourceType::CatalyzedUtriumAlkalide => "XUHO2",
        ResourceType::CatalyzedKeaniumAcid => "XKH2O",
        ResourceType::CatalyzedKeaniumAlkalide => "XKHO2",
        ResourceType::CatalyzedLemergiumAcid => "XLH2O",
        ResourceType::CatalyzedLemergiumAlkalide => "XLHO2",
        ResourceType::CatalyzedZynthiumAcid => "XZH2O",
        ResourceType::CatalyzedZynthiumAlkalide => "XZHO2",
        ResourceType::CatalyzedGhodiumAcid => "XGH2O",
        ResourceType::CatalyzedGhodiumAlkalide => "XGHO2",
        ResourceType::Ops => "ops",
        ResourceType::Silicon => "silicon",
        ResourceType::Metal => "metal",
        ResourceType::Biomass => "biomass",
        ResourceType::Mist => "mist",
        ResourceType::UtriumBar => "utrium_bar",
        ResourceType::LemergiumBar => "lemergium_bar",
        ResourceType::ZynthiumBar => "zynthium_bar",
        ResourceType::KeaniumBar => "keanium_bar",
        ResourceType::GhodiumMelt => "ghodium_melt",
        ResourceType::Oxidant => "oxidant",
        ResourceType::Reductant => "reductant",
        ResourceType::Purifier => "purifier",
        ResourceType::Battery => "battery",
        ResourceType::Composite => "composite",
        ResourceType::Crystal => "crystal",
        ResourceType::Liquid => "liquid",
        ResourceType::Wire => "wire",
        ResourceType::Switch => "switch",
        ResourceType::Transistor => "transistor",
        ResourceType::Microchip => "microchip",
        ResourceType::Circuit => "circuit",
        ResourceType::Device => "device",
        ResourceType::Cell => "cell",
        ResourceType::Phlegm => "phlegm",
        ResourceType::Tissue => "tissue",
        ResourceType::Muscle => "muscle",
        ResourceType::Organoid => "organoid",
        ResourceType::Organism => "organism",
        ResourceType::Alloy => "alloy",
        ResourceType::Tube => "tube",
        ResourceType::Fixtures => "fixtures",
        ResourceType::Frame => "frame",
        ResourceType::Hydraulics => "hydraulics",
        ResourceType::Machine => "machine",
        ResourceType::Condensate => "condensate",
        ResourceType::Concentrate => "concentrate",
        ResourceType::Extract => "extract",
        ResourceType::Spirit => "spirit",
        ResourceType::Emanation => "emanation",
        ResourceType::Essence => "essence",
        _ => "unknown",
    }
}

#[derive(Shrinkwrap, Default)]
#[shrinkwrap(mutable)]
struct StorageResource(HashMap<ResourceType, u32>);

impl Serialize for StorageResource {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut m = s.serialize_map(Some(self.0.len()))?;

        for (k, v) in &self.0 {
            let k = to_resource_name(*k);

            m.serialize_entry(k, v)?;
        }

        m.end()
    }
}

#[derive(Shrinkwrap, Default)]
#[shrinkwrap(mutable)]
struct StorageStructure(HashMap<StructureType, StorageResource>);

impl Serialize for StorageStructure {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut m = s.serialize_map(Some(self.0.len()))?;

        for (k, v) in &self.0 {
            let k = to_structure_name(*k);

            m.serialize_entry(k, v)?;
        }

        m.end()
    }
}

#[derive(Serialize)]
pub struct RoomStats {
    energy_available: u32,
    energy_capacity_available: u32,

    storage: StorageStructure,

    controller_progress: u32,
    controller_progress_total: u32,
    controller_level: u32,
}

#[derive(Serialize)]
pub struct GclStats {
    progress: f64,
    progress_total: f64,
    level: u32,
}

#[derive(Serialize)]
pub struct MarketStats {
    credits: f64,
}

#[derive(Serialize)]
pub struct ShardStats {
    time: u32,
    gcl: GclStats,
    cpu: CpuStats,
    room: HashMap<RoomName, RoomStats>,
    market: MarketStats,
}

#[derive(Serialize)]
pub struct Stats {
    shard: HashMap<String, ShardStats>,
}

pub struct StatsSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl StatsSystem {
    fn get_gcl_stats() -> GclStats {
        GclStats {
            progress: game::gcl::progress(),
            progress_total: game::gcl::progress_total(),
            level: game::gcl::level(),
        }
    }

    fn get_cpu_stats() -> CpuStats {
        CpuStats {
            bucket: game::cpu::bucket() as u32,
            limit: game::cpu::limit(),
            used: game::cpu::get_used(),
        }
    }

    fn get_room_stats(data: &StatsSystemData) -> HashMap<RoomName, RoomStats> {
        (&data.entities, &data.room_data)
            .join()
            .filter(|(_, room_data)| {
                room_data
                    .get_dynamic_visibility_data()
                    .map(|v| v.visible() && v.owner().mine())
                    .unwrap_or(false)
            })
            .filter_map(|(_, room_data)| {
                if let Some(room) = game::rooms().get(room_data.name) {
                    let controller = room.controller()?;

                    let structures = room_data.get_structures()?;
                    let mut storage: StorageStructure = StorageStructure::default();

                    for structure in structures.all().iter() {
                        let structure_type = structure.structure_type();

                        if let Some(store) = structure.as_has_store() {
                            let structure_storage = storage.entry(structure_type).or_insert_with(StorageResource::default);

                            for resource_type in store.store().store_types() {
                                let amount = store.store().get(resource_type).unwrap_or(0);

                                structure_storage
                                    .entry(resource_type)
                                    .and_modify(|e| *e += amount)
                                    .or_insert(amount);
                            }
                        }
                    }

                    let stats = RoomStats {
                        energy_available: room.energy_available(),
                        energy_capacity_available: room.energy_capacity_available(),

                        storage,

                        controller_progress: controller.progress().unwrap_or(0),
                        controller_progress_total: controller.progress_total().unwrap_or(0),
                        controller_level: controller.level() as u32,
                    };

                    Some((room_data.name, stats))
                } else {
                    None
                }
            })
            .collect()
    }

    fn get_market_stats() -> MarketStats {
        MarketStats {
            credits: game::market::credits(),
        }
    }

    fn get_shard_stats(data: &StatsSystemData) -> ShardStats {
        ShardStats {
            time: game::time(),
            gcl: Self::get_gcl_stats(),
            cpu: Self::get_cpu_stats(),
            room: Self::get_room_stats(data),
            market: Self::get_market_stats(),
        }
    }

    fn get_shards_stats(data: &StatsSystemData) -> HashMap<String, ShardStats> {
        let mut shards = HashMap::new();

        shards.insert(game::shard::name(), Self::get_shard_stats(data));

        shards
    }
}

#[derive(SystemData)]
pub struct StatsSystemData<'a> {
    entities: Entities<'a>,
    room_data: ReadStorage<'a, RoomData>,
    memory_arbiter: WriteExpect<'a, MemoryArbiter>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for StatsSystem {
    type SystemData = StatsSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        data.memory_arbiter.request(99);

        if data.memory_arbiter.is_active(99) {
            let stats = Stats {
                shard: Self::get_shards_stats(&data),
            };

            if let Ok(stats_data) = serde_json::to_string(&stats) {
                data.memory_arbiter.set(99, &stats_data);
            }
        }
    }
}
