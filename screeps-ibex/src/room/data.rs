use crate::remoteobjectid::*;
use crate::serialize::EntityVec;
use screeps::*;
use screeps_cache::*;
use screeps_foreman::constants::*;
use screeps_foreman::planner::{FastRoomTerrain, TerrainFlags};
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use std::{fmt::Display, cell::*};
use crate::visualize::*;
use crate::ui::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct RoomTerrainStatistics {
    walkable_tiles: u32,
    swamp_tiles: u32,
    plain_tiles: u32,
    wall_tiles: u32,
}

impl RoomTerrainStatistics {
    fn from_terrain(terrain: &FastRoomTerrain) -> RoomTerrainStatistics {
        let mut walkable_tiles = 0;
        let mut swamp_tiles = 0;
        let mut plain_tiles = 0;
        let mut wall_tiles = 0;

        for x in 0..ROOM_WIDTH as u8 {
            for y in 0..ROOM_HEIGHT as u8 {
                let terrain_mask = terrain.get_xy(x, y);

                if terrain_mask.contains(TerrainFlags::WALL) {
                    wall_tiles += 1;
                } else {
                    walkable_tiles += 1;

                    if terrain_mask.contains(TerrainFlags::SWAMP) {
                        swamp_tiles += 1;
                    } else {
                        plain_tiles += 1;
                    }
                }
            }
        }

        RoomTerrainStatistics {
            walkable_tiles,
            swamp_tiles,
            plain_tiles,
            wall_tiles,
        }
    }

    pub fn walkable_tiles(&self) -> u32 {
        self.walkable_tiles
    }

    pub fn swamp_tiles(&self) -> u32 {
        self.swamp_tiles
    }

    pub fn plain_tiles(&self) -> u32 {
        self.plain_tiles
    }

    pub fn wall_tiles(&self) -> u32 {
        self.wall_tiles
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RoomStaticVisibilityData {
    #[serde(rename = "c")]
    controller: Option<RemoteObjectId<StructureController>>,
    #[serde(rename = "s")]
    sources: Vec<RemoteObjectId<Source>>,
    #[serde(rename = "m")]
    minerals: Vec<RemoteObjectId<Mineral>>,
    #[serde(rename = "r")]
    terrain_statistics: RoomTerrainStatistics,
}

impl RoomStaticVisibilityData {
    pub fn controller(&self) -> Option<&RemoteObjectId<StructureController>> {
        self.controller.as_ref()
    }

    pub fn sources(&self) -> &Vec<RemoteObjectId<Source>> {
        &self.sources
    }

    pub fn minerals(&self) -> &Vec<RemoteObjectId<Mineral>> {
        &self.minerals
    }

    pub fn terrain_statistics(&self) -> &RoomTerrainStatistics {
        &self.terrain_statistics
    }

    pub fn visualize(&self, _room_visualizer: &mut RoomVisualizer, _list_state: &mut ListVisualizerState) {
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RoomDisposition {
    #[serde(rename = "n")]
    Neutral,
    #[serde(rename = "m")]
    Mine,
    #[serde(rename = "f")]
    Friendly(String),
    #[serde(rename = "h")]
    Hostile(String),
}

impl Display for RoomDisposition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoomDisposition::Neutral => write!(f, "Neutral"),
            RoomDisposition::Mine => write!(f, "Mine"),
            RoomDisposition::Friendly(name) => write!(f, "Friendly: {}", name),
            RoomDisposition::Hostile(name) => write!(f, "Hostile: {}", name), 
        }
    }    
}

impl RoomDisposition {
    pub fn name(&self) -> Option<&str> {
        match self {
            RoomDisposition::Neutral => None,
            RoomDisposition::Mine => Some(crate::globals::user::name()),
            RoomDisposition::Friendly(name) => Some(name),
            RoomDisposition::Hostile(name) => Some(name),
        }
    }

    pub fn neutral(&self) -> bool {
        if let RoomDisposition::Neutral = self {
            true
        } else {
            false
        }
    }

    pub fn mine(&self) -> bool {
        if let RoomDisposition::Mine = self {
            true
        } else {
            false
        }
    }

    pub fn hostile(&self) -> bool {
        if let RoomDisposition::Hostile(_) = self {
            true
        } else {
            false
        }
    }

    pub fn friendly(&self) -> bool {
        if let RoomDisposition::Friendly(_) = self {
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RoomSign {
    #[serde(rename = "u")]
    user: RoomDisposition,
    #[serde(rename = "m")]
    message: String,
}

impl RoomSign {
    pub fn user(&self) -> &RoomDisposition {
        &self.user
    }

    pub fn message(&self) -> &String {
        &self.message
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RoomDynamicVisibilityData {
    #[serde(rename = "u")]
    update_tick: u32,
    #[serde(rename = "o")]
    owner: RoomDisposition,
    #[serde(rename = "r")]
    reservation: RoomDisposition,
    #[serde(rename = "sk")]
    source_keeper: bool,
    #[serde(rename = "s")]
    sign: Option<RoomSign>,
    #[serde(rename = "hc")]
    hostile_creeps: bool,
    #[serde(rename = "h")]
    hostile_structures: bool,    
}

impl RoomDynamicVisibilityData {
    pub fn last_updated(&self) -> u32 {
        self.update_tick
    }

    pub fn age(&self) -> u32 {
        game::time() - self.update_tick
    }

    pub fn visible(&self) -> bool {
        self.age() == 0
    }

    pub fn updated_within(&self, ticks: u32) -> bool {
        self.age() <= ticks
    }

    pub fn owner(&self) -> &RoomDisposition {
        &self.owner
    }

    pub fn reservation(&self) -> &RoomDisposition {
        &self.reservation
    }

    pub fn source_keeper(&self) -> bool {
        self.source_keeper
    }

    pub fn sign(&self) -> &Option<RoomSign> {
        &self.sign
    }

    pub fn hostile_creeps(&self) -> bool {
        self.hostile_creeps
    }

    pub fn hostile_structures(&self) -> bool {
        self.hostile_structures
    }

    pub fn visualize(&self, room_visualizer: &mut RoomVisualizer, list_state: &mut ListVisualizerState) {
        let mut list_visualizer = list_state.visualize(room_visualizer);

        list_visualizer.add_text(format!("Visible: {} - Age: {}", self.visible(), self.age()), None);
        list_visualizer.add_text(format!("Owner: {}", self.owner()), None);
        list_visualizer.add_text(format!("Reservation: {}", self.reservation()), None);
        list_visualizer.add_text(format!("Source Keeper: {}", self.source_keeper()), None);
        list_visualizer.add_text(format!("Hostile creeps: {}", self.hostile_creeps()), None);
        list_visualizer.add_text(format!("Hostile structures: {}", self.hostile_structures()), None);
    }
}

#[derive(Component, ConvertSaveload)]
pub struct RoomData {
    #[convert_save_load_attr(serde(rename = "n"))]
    pub name: RoomName,
    #[convert_save_load_attr(serde(rename = "m"))]
    missions: EntityVec<Entity>,
    #[convert_save_load_attr(serde(rename = "s"))]
    static_visibility_data: Option<RoomStaticVisibilityData>,
    #[convert_save_load_attr(serde(rename = "d"))]
    dynamic_visibility_data: Option<RoomDynamicVisibilityData>,
    #[convert_save_load_skip_convert]
    #[convert_save_load_attr(serde(skip))]
    room_structure_data: RefCell<Option<RoomStructureData>>,
    #[convert_save_load_skip_convert]
    #[convert_save_load_attr(serde(skip))]
    room_construction_sites_data: RefCell<Option<ConstructionSiteData>>,
    #[convert_save_load_skip_convert]
    #[convert_save_load_attr(serde(skip))]
    room_creep_data: RefCell<Option<CreepData>>,
}

impl RoomData {
    pub fn new(room_name: RoomName) -> RoomData {
        RoomData {
            name: room_name,
            missions: EntityVec::new(),
            static_visibility_data: None,
            dynamic_visibility_data: None,
            room_structure_data: RefCell::new(None),
            room_construction_sites_data: RefCell::new(None),
            room_creep_data: RefCell::new(None),
        }
    }

    pub fn visualize(&self, room_visualizer: &mut RoomVisualizer) {
        let missions_text_style = TextStyle::default().font(0.5).align(TextAlign::Left);
        let mut list_state = ListVisualizerState::new(ROOM_DATA_POS, (0.0, 1.0), Some(missions_text_style));

        if let Some(static_visibility_data) = self.get_static_visibility_data() {
            static_visibility_data.visualize(room_visualizer, &mut list_state);
        }

        if let Some(dynamic_visibility_data) = self.get_dynamic_visibility_data() {
            dynamic_visibility_data.visualize(room_visualizer, &mut list_state);
        }
    }

    pub fn get_missions(&self) -> &EntityVec<Entity> {
        &self.missions
    }

    pub fn add_mission(&mut self, mission: Entity) {
        self.missions.push(mission);
    }

    pub fn remove_mission(&mut self, mission: Entity) {
        self.missions.retain(|other| *other != mission);
    }

    pub fn update(&mut self, room: &Room) {
        if self.static_visibility_data.is_none() {
            self.static_visibility_data = Some(Self::create_static_visibility_data(&room));
        }

        self.dynamic_visibility_data = Some(self.create_dynamic_visibility_data(&room));
    }
    
    fn create_static_visibility_data(room: &Room) -> RoomStaticVisibilityData {
        let controller_id = room.controller().map(|c| c.remote_id());
        let source_ids = room.find(find::SOURCES).into_iter().map(|s| s.remote_id()).collect();
        let mineral_ids = room.find(find::MINERALS).into_iter().map(|s| s.remote_id()).collect();

        let terrain = room.get_terrain();
        let terrain = FastRoomTerrain::new(terrain.get_raw_buffer());
        let terrain_statistics = RoomTerrainStatistics::from_terrain(&terrain);

        RoomStaticVisibilityData {
            controller: controller_id,
            sources: source_ids,
            minerals: mineral_ids,
            terrain_statistics,
        }
    }

    fn name_option_to_disposition(name: Option<String>) -> RoomDisposition {
        name.map(Self::name_to_disposition).unwrap_or_else(|| RoomDisposition::Neutral)
    }

    fn name_to_disposition(name: String) -> RoomDisposition {
        let friends: &[String] = &[];

        if name == crate::globals::user::name() {
            RoomDisposition::Mine
        } else if friends.iter().any(|friend_name| &name == friend_name) {
            RoomDisposition::Friendly(name)
        } else {
            RoomDisposition::Hostile(name)
        }
    }

    fn create_dynamic_visibility_data(&self, room: &Room) -> RoomDynamicVisibilityData {
        let controller = room.controller();

        let controller_owner_name = controller.as_ref().and_then(|c| c.owner_name());
        let controller_owner_disposition = Self::name_option_to_disposition(controller_owner_name);

        let controller_reservation_name = controller.as_ref().and_then(|c| c.reservation()).map(|r| r.username);
        let controller_reservation_disposition = Self::name_option_to_disposition(controller_reservation_name);

        let sign = controller.as_ref().and_then(|c| c.sign()).map(|s| RoomSign {
            user: Self::name_to_disposition(s.username),
            message: s.text,
        });

        let structures = self.get_structures();

        //TODO: This is expensive - can really just be calculated for room number. Not possible to calculate given x/y coord is private.
        let source_keeper = structures.as_ref().map(|s| !s.keeper_lairs().is_empty()).unwrap_or(false);

        //TODO: Include power creeps?
        let hostile_creeps = self.get_creeps()
            .iter()
            .flat_map(|c| c.hostile())
            .flat_map(|c| c.body())
            .any(|p| match p.part {
                Part::Attack | Part::RangedAttack | Part::Work => true,
                _ => false
            });

        let hostile_structures = structures
            .iter()
            .flat_map(|s| s.all())
            .filter_map(|s| s.as_owned())
            .any(|s| s.has_owner() && !s.my());

        RoomDynamicVisibilityData {
            update_tick: game::time(),
            owner: controller_owner_disposition,
            reservation: controller_reservation_disposition,
            source_keeper,
            sign,
            hostile_creeps,
            hostile_structures,
        }
    }

    pub fn get_static_visibility_data(&self) -> Option<&RoomStaticVisibilityData> {
        self.static_visibility_data.as_ref()
    }

    pub fn get_dynamic_visibility_data(&self) -> Option<&RoomDynamicVisibilityData> {
        self.dynamic_visibility_data.as_ref()
    }

    pub fn get_structures(&self) -> Option<Ref<RoomStructureData>> {
        let name = self.name;

        self.room_structure_data
            .maybe_access(
                |s| game::time() != s.last_updated,
                move || game::rooms::get(name).as_ref().map(|room| RoomStructureData::new(room)),
            )
            .take()
    }

    pub fn get_construction_sites(&self) -> Option<Ref<Vec<ConstructionSite>>> {
        let name = self.name;

        self.room_construction_sites_data
            .maybe_access(
                |s| game::time() != s.last_updated,
                move || game::rooms::get(name).as_ref().map(|room| ConstructionSiteData::new(room)),
            )
            .take()
            .map(|s| Ref::map(s, |o| &o.construction_sites))
    }

    pub fn get_creeps(&self) -> Option<Ref<CreepData>> {
        let name = self.name;

        self.room_creep_data
            .maybe_access(
                |s| game::time() != s.last_updated,
                move || game::rooms::get(name).as_ref().map(|room| CreepData::new(room)),
            )
            .take()
    }
}

#[derive(Clone)]
pub struct RoomStructureData {
    last_updated: u32,
    structures: Vec<Structure>,

    containers: Vec<StructureContainer>,
    controllers: Vec<StructureController>,
    extensions: Vec<StructureExtension>,
    extractors: Vec<StructureExtractor>,
    factories: Vec<StructureFactory>,
    invader_cores: Vec<StructureInvaderCore>,
    keeper_lairs: Vec<StructureKeeperLair>,
    labs: Vec<StructureLab>,
    links: Vec<StructureLink>,
    nukers: Vec<StructureNuker>,
    observers: Vec<StructureObserver>,
    power_banks: Vec<StructurePowerBank>,
    power_spawns: Vec<StructurePowerSpawn>,
    portals: Vec<StructurePortal>,
    ramparts: Vec<StructureRampart>,
    roads: Vec<StructureRoad>,
    spawns: Vec<StructureSpawn>,
    storages: Vec<StructureStorage>,
    terminals: Vec<StructureTerminal>,
    towers: Vec<StructureTower>,
    walls: Vec<StructureWall>,
}

impl RoomStructureData {
    fn new(room: &Room) -> RoomStructureData {
        let structures = room.find(find::STRUCTURES);

        let mut containers = Vec::new();
        let mut controllers = Vec::new();
        let mut extensions = Vec::new();
        let mut extractors = Vec::new();
        let mut factories = Vec::new();
        let mut invader_cores = Vec::new();
        let mut keeper_lairs = Vec::new();
        let mut labs = Vec::new();
        let mut links = Vec::new();
        let mut nukers = Vec::new();
        let mut observers = Vec::new();
        let mut power_banks = Vec::new();
        let mut power_spawns = Vec::new();
        let mut portals = Vec::new();
        let mut ramparts = Vec::new();
        let mut roads = Vec::new();
        let mut spawns = Vec::new();
        let mut storages = Vec::new();
        let mut terminals = Vec::new();
        let mut towers = Vec::new();
        let mut walls = Vec::new();

        for structure in structures.iter() {
            match structure {
                Structure::Container(data) => containers.push(data.clone()),
                Structure::Controller(data) => controllers.push(data.clone()),
                Structure::Extension(data) => extensions.push(data.clone()),
                Structure::Extractor(data) => extractors.push(data.clone()),
                Structure::Factory(data) => factories.push(data.clone()),
                Structure::InvaderCore(data) => invader_cores.push(data.clone()),
                Structure::KeeperLair(data) => keeper_lairs.push(data.clone()),
                Structure::Lab(data) => labs.push(data.clone()),
                Structure::Link(data) => links.push(data.clone()),
                Structure::Nuker(data) => nukers.push(data.clone()),
                Structure::Observer(data) => observers.push(data.clone()),
                Structure::PowerBank(data) => power_banks.push(data.clone()),
                Structure::PowerSpawn(data) => power_spawns.push(data.clone()),
                Structure::Portal(data) => portals.push(data.clone()),
                Structure::Rampart(data) => ramparts.push(data.clone()),
                Structure::Road(data) => roads.push(data.clone()),
                Structure::Spawn(data) => spawns.push(data.clone()),
                Structure::Storage(data) => storages.push(data.clone()),
                Structure::Terminal(data) => terminals.push(data.clone()),
                Structure::Tower(data) => towers.push(data.clone()),
                Structure::Wall(data) => walls.push(data.clone()),
            }
        }

        RoomStructureData {
            last_updated: game::time(),

            structures,

            containers,
            controllers,
            extensions,
            extractors,
            factories,
            invader_cores,
            keeper_lairs,
            labs,
            links,
            nukers,
            observers,
            power_banks,
            power_spawns,
            portals,
            ramparts,
            roads,
            spawns,
            storages,
            terminals,
            towers,
            walls,
        }
    }

    pub fn all(&self) -> &[Structure] {
        &self.structures
    }

    pub fn containers(&self) -> &[StructureContainer] {
        &self.containers
    }

    pub fn controllers(&self) -> &[StructureController] {
        &self.controllers
    }

    pub fn extensions(&self) -> &[StructureExtension] {
        &self.extensions
    }

    pub fn extractors(&self) -> &[StructureExtractor] {
        &self.extractors
    }

    pub fn factories(&self) -> &[StructureFactory] {
        &self.factories
    }

    pub fn invader_cores(&self) -> &[StructureInvaderCore] {
        &self.invader_cores
    }

    pub fn keeper_lairs(&self) -> &[StructureKeeperLair] {
        &self.keeper_lairs
    }

    pub fn labs(&self) -> &[StructureLab] {
        &self.labs
    }

    pub fn links(&self) -> &[StructureLink] {
        &self.links
    }

    pub fn nukers(&self) -> &[StructureNuker] {
        &self.nukers
    }

    pub fn observers(&self) -> &[StructureObserver] {
        &self.observers
    }

    pub fn power_banks(&self) -> &[StructurePowerBank] {
        &self.power_banks
    }

    pub fn power_spawns(&self) -> &[StructurePowerSpawn] {
        &self.power_spawns
    }

    pub fn portals(&self) -> &[StructurePortal] {
        &self.portals
    }

    pub fn ramparts(&self) -> &[StructureRampart] {
        &self.ramparts
    }

    pub fn roads(&self) -> &[StructureRoad] {
        &self.roads
    }

    pub fn spawns(&self) -> &[StructureSpawn] {
        &self.spawns
    }

    pub fn storages(&self) -> &[StructureStorage] {
        &self.storages
    }

    pub fn terminals(&self) -> &[StructureTerminal] {
        &self.terminals
    }

    pub fn towers(&self) -> &[StructureTower] {
        &self.towers
    }

    pub fn walls(&self) -> &[StructureWall] {
        &self.walls
    }
}

#[derive(Clone)]
struct ConstructionSiteData {
    last_updated: u32,
    construction_sites: Vec<ConstructionSite>,
}

impl ConstructionSiteData {
    fn new(room: &Room) -> ConstructionSiteData {
        let construction_sites = room.find(find::CONSTRUCTION_SITES);

        ConstructionSiteData {
            last_updated: game::time(),
            construction_sites,
        }
    }
}

#[derive(Clone)]
pub struct CreepData {
    last_updated: u32,
    creeps: Vec<Creep>,

    friendly: Vec<Creep>,
    hostile: Vec<Creep>,
}

impl CreepData {
    fn new(room: &Room) -> CreepData {
        let creeps = room.find(find::CREEPS);

        let (friendly, hostile) = creeps.iter().cloned().partition(|c| c.my());

        CreepData {
            last_updated: game::time(),
            creeps,
            friendly,
            hostile,
        }
    }

    pub fn all(&self) -> &[Creep] {
        &self.creeps
    }

    pub fn friendly(&self) -> &[Creep] {
        &self.friendly
    }

    pub fn hostile(&self) -> &[Creep] {
        &self.hostile
    }
}
