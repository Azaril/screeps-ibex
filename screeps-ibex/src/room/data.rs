use crate::remoteobjectid::*;
use crate::serialize::EntityVec;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use std::cell::*;
use screeps_cache::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct RoomStaticVisibilityData {
    #[serde(default)]
    #[serde(rename = "c")]
    controller: Option<RemoteObjectId<StructureController>>,
    #[serde(default)]
    #[serde(rename = "s")]
    sources: Vec<RemoteObjectId<Source>>,
    #[serde(default)]
    #[serde(rename = "m")]
    minerals: Vec<RemoteObjectId<Mineral>>,
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
}

impl RoomDynamicVisibilityData {
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
}

#[derive(Component, ConvertSaveload)]
pub struct RoomData {
    #[convert_save_load_attr(serde(rename = "n"))]
    pub name: RoomName,
    #[convert_save_load_skip_convert]
    #[convert_save_load_attr(serde(skip))]    
    visible: bool,
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
            visible: false,
            missions: EntityVec::new(),
            static_visibility_data: None,
            dynamic_visibility_data: None,
            room_structure_data: RefCell::new(None),
            room_construction_sites_data: RefCell::new(None),
            room_creep_data: RefCell::new(None),
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

    pub fn clear_visible(&mut self) {
        self.visible = false;
    }

    pub fn update(&mut self, room: &Room) {
        self.visible = true;

        if self.static_visibility_data.is_none() {
            self.static_visibility_data = Some(Self::create_static_visibility_data(&room));
        }

        self.dynamic_visibility_data = Some(Self::create_dynamic_visibility_data(&room));
    }

    fn create_static_visibility_data(room: &Room) -> RoomStaticVisibilityData {
        let controller_id = room.controller().map(|c| c.remote_id());
        let source_ids = room.find(find::SOURCES).into_iter().map(|s| s.remote_id()).collect();
        let mineral_ids = room.find(find::MINERALS).into_iter().map(|s| s.remote_id()).collect();

        RoomStaticVisibilityData {
            controller: controller_id,
            sources: source_ids,
            minerals: mineral_ids,
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

    fn create_dynamic_visibility_data(room: &Room) -> RoomDynamicVisibilityData {
        let controller = room.controller();

        let controller_owner_name = controller.as_ref().and_then(|c| c.owner_name());
        let controller_owner_disposition = Self::name_option_to_disposition(controller_owner_name);

        let controller_reservation_name = controller.as_ref().and_then(|c| c.reservation()).map(|r| r.username);
        let controller_reservation_disposition = Self::name_option_to_disposition(controller_reservation_name);

        let sign = controller.as_ref().and_then(|c| c.sign()).map(|s| RoomSign {
            user: Self::name_to_disposition(s.username),
            message: s.text,
        });

        //TODO: This is expensive - can really just be calculated for room number. Not possible to calculate given x/y coord is private.
        let source_keeper = room.find(find::HOSTILE_STRUCTURES).into_iter().any(|s| {
            if let Structure::KeeperLair(_) = s.as_structure() {
                true
            } else {
                false
            }
        });

        RoomDynamicVisibilityData {
            update_tick: game::time(),
            owner: controller_owner_disposition,
            reservation: controller_reservation_disposition,
            source_keeper,
            sign,
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

        self.room_structure_data.maybe_access(
            |s| game::time() != s.last_updated,
            move || game::rooms::get(name).as_ref().map(|room| RoomStructureData::new(room))
        ).take()
    }

    pub fn get_construction_sites(&self) -> Option<Ref<Vec<ConstructionSite>>> {
        let name = self.name;

        self.room_construction_sites_data.maybe_access(
            |s| game::time() != s.last_updated,
            move || game::rooms::get(name).as_ref().map(|room| ConstructionSiteData::new(room))
        ).take().map(|s| Ref::map(s, |o| &o.construction_sites))
    }

    pub fn get_creeps(&self) -> Option<Ref<CreepData>> {
        let name = self.name;

        self.room_creep_data.maybe_access(
            |s| game::time() != s.last_updated,
            move || game::rooms::get(name).as_ref().map(|room| CreepData::new(room))
        ).take()
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
    construction_sites: Vec<ConstructionSite>
}

impl ConstructionSiteData {
    fn new(room: &Room) -> ConstructionSiteData {
        let construction_sites = room.find(find::CONSTRUCTION_SITES);

        ConstructionSiteData {
            last_updated: game::time(),
            construction_sites
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
            hostile
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