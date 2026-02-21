use crate::remoteobjectid::*;
use crate::serialize::EntityVec;
use screeps::*;
use screeps_cache::*;
use screeps_foreman::constants::*;
use screeps_foreman::terrain::{FastRoomTerrain, TerrainFlags};
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use std::{cell::*, fmt::Display};

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

        for x in 0..ROOM_WIDTH {
            for y in 0..ROOM_HEIGHT {
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
    /// Cached exits from this room. Populated lazily from describe_exits().
    /// Static data -- never changes for a given room.
    #[serde(default, rename = "e")]
    exits: Option<Vec<(Direction, RoomName)>>,
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

    pub fn exits(&self) -> Option<&Vec<(Direction, RoomName)>> {
        self.exits.as_ref()
    }

    pub fn set_exits(&mut self, exits: Vec<(Direction, RoomName)>) {
        self.exits = Some(exits);
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
    pub fn name(&self) -> Option<String> {
        match self {
            RoomDisposition::Neutral => None,
            RoomDisposition::Mine => Some(crate::globals::user::name()),
            RoomDisposition::Friendly(name) => Some(name.clone()),
            RoomDisposition::Hostile(name) => Some(name.clone()),
        }
    }

    pub fn neutral(&self) -> bool {
        matches!(self, RoomDisposition::Neutral)
    }

    pub fn mine(&self) -> bool {
        matches!(self, RoomDisposition::Mine)
    }

    pub fn hostile(&self) -> bool {
        matches!(self, RoomDisposition::Hostile(_))
    }

    pub fn friendly(&self) -> bool {
        matches!(self, RoomDisposition::Friendly(_))
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
}

#[derive(Component)]
pub struct RoomData {
    pub name: RoomName,
    missions: EntityVec<Entity>,
    static_visibility_data: Option<RoomStaticVisibilityData>,
    dynamic_visibility_data: Option<RoomDynamicVisibilityData>,
    room_structure_data: RefCell<Option<RoomStructureData>>,
    room_construction_sites_data: RefCell<Option<ConstructionSiteData>>,
    room_creep_data: RefCell<Option<CreepData>>,
    room_dropped_resource_data: RefCell<Option<DroppedResourceData>>,
    room_nuke_data: RefCell<Option<NukeData>>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(bound = "MA: Marker")]
pub struct RoomDataSaveloadData<MA>
where
    MA: Marker + Serialize,
    for<'deser> MA: Deserialize<'deser>,
{
    #[serde(rename = "n")]
    pub name: <RoomName as ConvertSaveload<MA>>::Data,
    #[serde(rename = "m")]
    missions: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    #[serde(rename = "s")]
    static_visibility_data: <Option<RoomStaticVisibilityData> as ConvertSaveload<MA>>::Data,
    #[serde(rename = "d")]
    dynamic_visibility_data: <Option<RoomDynamicVisibilityData> as ConvertSaveload<MA>>::Data,
}

impl<MA> ConvertSaveload<MA> for RoomData
where
    MA: Marker + Serialize,
    for<'deser> MA: Deserialize<'deser>,
{
    type Data = RoomDataSaveloadData<MA>;
    #[allow(deprecated)]
    type Error = NoError;

    fn convert_into<F>(&self, mut ids: F) -> Result<Self::Data, Self::Error>
    where
        F: FnMut(Entity) -> Option<MA>,
    {
        Ok(RoomDataSaveloadData {
            name: ConvertSaveload::convert_into(&self.name, &mut ids)?,
            missions: ConvertSaveload::convert_into(&self.missions, &mut ids)?,
            static_visibility_data: ConvertSaveload::convert_into(&self.static_visibility_data, &mut ids)?,
            dynamic_visibility_data: ConvertSaveload::convert_into(&self.dynamic_visibility_data, &mut ids)?,
        })
    }

    fn convert_from<F>(data: Self::Data, mut ids: F) -> Result<Self, Self::Error>
    where
        F: FnMut(MA) -> Option<Entity>,
    {
        Ok(RoomData {
            name: ConvertSaveload::convert_from(data.name, &mut ids)?,
            missions: ConvertSaveload::convert_from(data.missions, &mut ids)?,
            static_visibility_data: ConvertSaveload::convert_from(data.static_visibility_data, &mut ids)?,
            dynamic_visibility_data: ConvertSaveload::convert_from(data.dynamic_visibility_data, &mut ids)?,
            room_structure_data: RefCell::new(None),
            room_construction_sites_data: RefCell::new(None),
            room_creep_data: RefCell::new(None),
            room_dropped_resource_data: RefCell::new(None),
            room_nuke_data: RefCell::new(None),
        })
    }
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
            room_dropped_resource_data: RefCell::new(None),
            room_nuke_data: RefCell::new(None),
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

    pub fn retain_missions<F>(&mut self, f: F)
    where
        F: FnMut(Entity) -> bool,
    {
        // EntityVec<Entity> derefs to Vec<Entity>; adapt the closure to take &Entity.
        let mut f = f;
        self.missions.retain(|e| f(*e));
    }

    pub fn update(&mut self, room: &Room) {
        if self.static_visibility_data.is_none() {
            self.static_visibility_data = Some(Self::create_static_visibility_data(room));
        }

        self.dynamic_visibility_data = Some(self.create_dynamic_visibility_data(room));
    }

    fn create_static_visibility_data(room: &Room) -> RoomStaticVisibilityData {
        let controller_id = room.controller().map(|c| c.remote_id());
        let source_ids = room.find(find::SOURCES, None).into_iter().map(|s| s.remote_id()).collect();
        let mineral_ids = room.find(find::MINERALS, None).into_iter().map(|s| s.remote_id()).collect();

        let terrain = room.get_terrain();
        let terrain = FastRoomTerrain::new(terrain.get_raw_buffer().to_vec());
        let terrain_statistics = RoomTerrainStatistics::from_terrain(&terrain);

        // Cache room exits (static, never changes).
        let exits = game::map::describe_exits(room.name());
        let exit_list: Vec<(Direction, RoomName)> = exits.entries().collect();

        RoomStaticVisibilityData {
            controller: controller_id,
            sources: source_ids,
            minerals: mineral_ids,
            terrain_statistics,
            exits: Some(exit_list),
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

        let controller_owner_name = controller.as_ref().and_then(|c| c.owner().map(|o| o.username()));
        let controller_owner_disposition = Self::name_option_to_disposition(controller_owner_name);

        let controller_reservation_name = controller.as_ref().and_then(|c| c.reservation()).map(|r| r.username());
        let controller_reservation_disposition = Self::name_option_to_disposition(controller_reservation_name);

        let sign = controller.as_ref().and_then(|c| c.sign()).map(|s| RoomSign {
            user: Self::name_to_disposition(s.username()),
            message: s.text(),
        });

        let structures = self.get_structures();

        //TODO: This is expensive - can really just be calculated for room number. Not possible to calculate given x/y coord is private.
        let source_keeper = structures.as_ref().map(|s| !s.keeper_lairs().is_empty()).unwrap_or(false);

        //TODO: Include power creeps?
        let hostile_creeps = self
            .get_creeps()
            .iter()
            .flat_map(|c| c.hostile())
            .flat_map(|c| c.body())
            .any(|p| matches!(p.part(), Part::Attack | Part::RangedAttack | Part::Work));

        let hostile_structures = structures
            .iter()
            .flat_map(|s| s.all())
            .filter(|s| !matches!(s.structure_type(), StructureType::KeeperLair))
            .filter_map(|s| s.as_owned())
            .any(|s| s.owner().is_some() && !s.my());

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

    pub fn get_structures(&self) -> Option<Ref<'_, RoomStructureData>> {
        let name = self.name;

        self.room_structure_data
            .maybe_access(
                |s| game::time() != s.last_updated,
                move || game::rooms().get(name).as_ref().map(RoomStructureData::new),
            )
            .take()
    }

    pub fn get_construction_sites(&self) -> Option<Ref<'_, Vec<ConstructionSite>>> {
        let name = self.name;

        self.room_construction_sites_data
            .maybe_access(
                |s| game::time() != s.last_updated,
                move || game::rooms().get(name).as_ref().map(ConstructionSiteData::new),
            )
            .take()
            .map(|s| Ref::map(s, |o| &o.construction_sites))
    }

    pub fn get_creeps(&self) -> Option<Ref<'_, CreepData>> {
        let name = self.name;

        self.room_creep_data
            .maybe_access(
                |s| game::time() != s.last_updated,
                move || game::rooms().get(name).as_ref().map(CreepData::new),
            )
            .take()
    }

    pub fn get_dropped_resources(&self) -> Option<Ref<'_, DroppedResourceData>> {
        let name = self.name;

        self.room_dropped_resource_data
            .maybe_access(
                |s| game::time() != s.last_updated,
                move || game::rooms().get(name).as_ref().map(DroppedResourceData::new),
            )
            .take()
    }

    pub fn get_nukes(&self) -> Option<Ref<'_, NukeData>> {
        let name = self.name;

        self.room_nuke_data
            .maybe_access(
                |s| game::time() != s.last_updated,
                move || game::rooms().get(name).as_ref().map(NukeData::new),
            )
            .take()
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct RoomStructureData {
    #[serde(skip)]
    last_updated: u32,
    #[serde(skip)]
    structures: Vec<StructureObject>,

    #[serde(skip)]
    containers: Vec<StructureContainer>,
    #[serde(skip)]
    controllers: Vec<StructureController>,
    #[serde(skip)]
    extensions: Vec<StructureExtension>,
    #[serde(skip)]
    extractors: Vec<StructureExtractor>,
    #[serde(skip)]
    factories: Vec<StructureFactory>,
    #[serde(skip)]
    invader_cores: Vec<StructureInvaderCore>,
    #[serde(skip)]
    keeper_lairs: Vec<StructureKeeperLair>,
    #[serde(skip)]
    labs: Vec<StructureLab>,
    #[serde(skip)]
    links: Vec<StructureLink>,
    #[serde(skip)]
    nukers: Vec<StructureNuker>,
    #[serde(skip)]
    observers: Vec<StructureObserver>,
    #[serde(skip)]
    power_banks: Vec<StructurePowerBank>,
    #[serde(skip)]
    power_spawns: Vec<StructurePowerSpawn>,
    #[serde(skip)]
    portals: Vec<StructurePortal>,
    #[serde(skip)]
    ramparts: Vec<StructureRampart>,
    #[serde(skip)]
    roads: Vec<StructureRoad>,
    #[serde(skip)]
    spawns: Vec<StructureSpawn>,
    #[serde(skip)]
    storages: Vec<StructureStorage>,
    #[serde(skip)]
    terminals: Vec<StructureTerminal>,
    #[serde(skip)]
    towers: Vec<StructureTower>,
    #[serde(skip)]
    walls: Vec<StructureWall>,
}

impl RoomStructureData {
    fn new(room: &Room) -> RoomStructureData {
        let structures = room.find(find::STRUCTURES, None);

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
                StructureObject::StructureContainer(data) => containers.push(data.clone()),
                StructureObject::StructureController(data) => controllers.push(data.clone()),
                StructureObject::StructureExtension(data) => extensions.push(data.clone()),
                StructureObject::StructureExtractor(data) => extractors.push(data.clone()),
                StructureObject::StructureFactory(data) => factories.push(data.clone()),
                StructureObject::StructureInvaderCore(data) => invader_cores.push(data.clone()),
                StructureObject::StructureKeeperLair(data) => keeper_lairs.push(data.clone()),
                StructureObject::StructureLab(data) => labs.push(data.clone()),
                StructureObject::StructureLink(data) => links.push(data.clone()),
                StructureObject::StructureNuker(data) => nukers.push(data.clone()),
                StructureObject::StructureObserver(data) => observers.push(data.clone()),
                StructureObject::StructurePowerBank(data) => power_banks.push(data.clone()),
                StructureObject::StructurePowerSpawn(data) => power_spawns.push(data.clone()),
                StructureObject::StructurePortal(data) => portals.push(data.clone()),
                StructureObject::StructureRampart(data) => ramparts.push(data.clone()),
                StructureObject::StructureRoad(data) => roads.push(data.clone()),
                StructureObject::StructureSpawn(data) => spawns.push(data.clone()),
                StructureObject::StructureStorage(data) => storages.push(data.clone()),
                StructureObject::StructureTerminal(data) => terminals.push(data.clone()),
                StructureObject::StructureTower(data) => towers.push(data.clone()),
                StructureObject::StructureWall(data) => walls.push(data.clone()),
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

    pub fn all(&self) -> &[StructureObject] {
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

#[derive(Clone, Serialize, Deserialize, Default)]
struct ConstructionSiteData {
    #[serde(skip)]
    last_updated: u32,
    #[serde(skip)]
    construction_sites: Vec<ConstructionSite>,
}

impl ConstructionSiteData {
    fn new(room: &Room) -> ConstructionSiteData {
        let construction_sites = room.find(find::CONSTRUCTION_SITES, None);

        ConstructionSiteData {
            last_updated: game::time(),
            construction_sites,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct CreepData {
    #[serde(skip)]
    last_updated: u32,
    #[serde(skip)]
    creeps: Vec<Creep>,

    #[serde(skip)]
    friendly: Vec<Creep>,
    #[serde(skip)]
    hostile: Vec<Creep>,
}

impl CreepData {
    fn new(room: &Room) -> CreepData {
        let creeps = room.find(find::CREEPS, None);

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

// ─── Dropped resources, tombstones, ruins ───────────────────────────────────

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct DroppedResourceData {
    #[serde(skip)]
    last_updated: u32,
    #[serde(skip)]
    resources: Vec<Resource>,
    #[serde(skip)]
    tombstones: Vec<Tombstone>,
    #[serde(skip)]
    ruins: Vec<Ruin>,
}

impl DroppedResourceData {
    fn new(room: &Room) -> DroppedResourceData {
        DroppedResourceData {
            last_updated: game::time(),
            resources: room.find(find::DROPPED_RESOURCES, None),
            tombstones: room.find(find::TOMBSTONES, None),
            ruins: room.find(find::RUINS, None),
        }
    }

    pub fn resources(&self) -> &[Resource] {
        &self.resources
    }

    pub fn tombstones(&self) -> &[Tombstone] {
        &self.tombstones
    }

    pub fn ruins(&self) -> &[Ruin] {
        &self.ruins
    }

    /// Total energy available from all dropped resources, tombstones, and ruins.
    pub fn total_energy(&self) -> u32 {
        let resource_energy: u32 = self
            .resources
            .iter()
            .filter(|r| r.resource_type() == ResourceType::Energy)
            .map(|r| r.amount())
            .sum();

        let tombstone_energy: u32 = self
            .tombstones
            .iter()
            .map(|t| t.store().get_used_capacity(Some(ResourceType::Energy)))
            .sum();

        let ruin_energy: u32 = self
            .ruins
            .iter()
            .map(|r| r.store().get_used_capacity(Some(ResourceType::Energy)))
            .sum();

        resource_energy + tombstone_energy + ruin_energy
    }

    /// Total value of all lootable resources (all types).
    pub fn total_loot_value(&self) -> u32 {
        let resource_total: u32 = self.resources.iter().map(|r| r.amount()).sum();

        let tombstone_total: u32 = self.tombstones.iter().map(|t| t.store().get_used_capacity(None)).sum();

        let ruin_total: u32 = self.ruins.iter().map(|r| r.store().get_used_capacity(None)).sum();

        resource_total + tombstone_total + ruin_total
    }
}

// ─── Incoming nukes ─────────────────────────────────────────────────────────

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct NukeData {
    #[serde(skip)]
    last_updated: u32,
    #[serde(skip)]
    nukes: Vec<Nuke>,
}

impl NukeData {
    fn new(room: &Room) -> NukeData {
        NukeData {
            last_updated: game::time(),
            nukes: room.find(find::NUKES, None),
        }
    }

    pub fn nukes(&self) -> &[Nuke] {
        &self.nukes
    }

    pub fn has_incoming(&self) -> bool {
        !self.nukes.is_empty()
    }
}
