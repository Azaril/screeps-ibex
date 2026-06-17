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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Tower DPS at room edge (hostile towers only). Set when we have visibility; used to size drain bodies.
    #[serde(default, rename = "td")]
    tower_dps_at_edge: Option<f32>,
    /// Non-my ACTIVE spawns present (derelict classification input: a working
    /// spawn means the owner can produce defenders). Inactive spawns — RCL
    /// decayed below their tier, or the room lost its owner entirely — are
    /// harmless husks and deliberately excluded so neutral ruins stay
    /// salvageable.
    #[serde(default, rename = "hsp")]
    hostile_spawns: bool,
    /// Non-my ACTIVE towers with enough energy to fire (derelict
    /// classification input). Energy is checked at observation time only — a
    /// dead-room tower can in principle be refilled by foreign haulers
    /// between sightings, which is why any threat-capable hostile creep
    /// sighting also resets the derelict clock (`hostile_threat_creeps`).
    #[serde(default, rename = "ht")]
    hostile_towers: bool,
    /// Hostile creeps with ANY part beyond Move/Tough (haulers, claimers,
    /// healers, workers — anything that could refill towers, attack the
    /// controller, or sustain a fight). Wider than `hostile_creeps` (combat
    /// parts only) so that an owner quietly servicing a "dead" room resets
    /// the derelict confirmation clock; pure-Move scouts wandering through do
    /// not.
    #[serde(default, rename = "htc")]
    hostile_threat_creeps: bool,
    /// Absolute tick at which the controller's safe mode ends, if it was
    /// active when last observed. Safe mode blocks all our offensive actions
    /// (withdraw/dismantle/attack) inside the room.
    #[serde(default, rename = "sme")]
    safe_mode_end: Option<u32>,
    /// Controller level when last observed (0 = unowned).
    #[serde(default, rename = "cl")]
    controller_level: Option<u8>,
    /// Controller downgrade timer when last observed. Counts down in real time
    /// while the owner is not upgrading, so for an abandoned room
    /// `update_tick + ttd` predicts the next level drop.
    #[serde(default, rename = "ctd")]
    controller_ticks_to_downgrade: Option<u32>,
    /// Tick at which the room was first observed derelict (hostile-owned but
    /// militarily dead), carried forward across visibility updates while the
    /// classification holds. None = not currently derelict.
    #[serde(default, rename = "dsi")]
    derelict_since: Option<u32>,
}

impl RoomDynamicVisibilityData {
    pub fn last_updated(&self) -> u32 {
        self.update_tick
    }

    pub fn age(&self) -> u32 {
        game::time().saturating_sub(self.update_tick)
    }

    pub fn visible(&self) -> bool {
        self.age() == 0
    }

    pub fn updated_within(&self, ticks: u32) -> bool {
        self.age() <= ticks
    }

    /// Tower DPS at room edge from last time we had visibility (hostile towers only). Used for drain body sizing.
    pub fn tower_dps_at_edge(&self) -> Option<f32> {
        self.tower_dps_at_edge
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

    pub fn hostile_spawns(&self) -> bool {
        self.hostile_spawns
    }

    pub fn hostile_towers(&self) -> bool {
        self.hostile_towers
    }

    pub fn hostile_threat_creeps(&self) -> bool {
        self.hostile_threat_creeps
    }

    /// Safe mode active as of the current tick. Extrapolated from the last
    /// observation — safe mode runs on a fixed timer, so this stays accurate
    /// without fresh visibility.
    pub fn safe_mode_active(&self) -> bool {
        self.safe_mode_end.map(|end| game::time() < end).unwrap_or(false)
    }

    pub fn controller_level(&self) -> Option<u8> {
        self.controller_level
    }

    pub fn controller_ticks_to_downgrade(&self) -> Option<u32> {
        self.controller_ticks_to_downgrade
    }

    /// Earliest tick at which the controller could drop a level, extrapolated
    /// from the last observed downgrade timer. Exact for an abandoned room
    /// (nothing is feeding the timer); a lower bound for a maintained one.
    pub fn predicted_downgrade_tick(&self) -> Option<u32> {
        self.controller_ticks_to_downgrade.map(|ttd| self.update_tick.saturating_add(ttd))
    }

    /// Any observed military capability: combat-capable creeps (attack /
    /// ranged / work parts), active spawns (defender production), or armed
    /// active towers.
    pub fn militarily_active(&self) -> bool {
        self.hostile_creeps || self.hostile_spawns || self.hostile_towers
    }

    /// Claimed by another player but dead: no military capability AND no
    /// threat-capable creeps (haulers/claimers/healers count — an owner
    /// quietly servicing the room is not derelict). Raw single-observation
    /// classification — use [`Self::confirmed_derelict`] before treating the
    /// room as safe to path through or act in.
    pub fn derelict(&self) -> bool {
        self.owner.hostile() && !self.militarily_active() && !self.hostile_threat_creeps
    }

    /// Observed-span the derelict classification has held: ticks between the
    /// FIRST derelict sighting and the LATEST sighting (None = not currently
    /// derelict). Deliberately not wall-clock — a single snapshot plus
    /// elapsed blind time proves nothing; confirmation requires two sightings
    /// at least the confirm window apart with no threat sighting between.
    pub fn derelict_for(&self) -> Option<u32> {
        self.derelict_since.map(|since| self.update_tick.saturating_sub(since))
    }

    /// Derelict, observed so over a span of at least `confirm_ticks` (no
    /// militarised/threat sighting in between), with intel no older than
    /// `max_age`. Consumers pick `max_age` per use: looser for pathing,
    /// tighter for committing creeps to work inside the room
    /// (`features.derelict.*`).
    pub fn confirmed_derelict(&self, confirm_ticks: u32, max_age: u32) -> bool {
        self.confirmed_derelict_at(game::time(), confirm_ticks, max_age)
    }

    /// Pure kernel of [`Self::confirmed_derelict`] (host-tested): `now` is
    /// passed in rather than read ambiently.
    pub fn confirmed_derelict_at(&self, now: u32, confirm_ticks: u32, max_age: u32) -> bool {
        self.derelict()
            && now.saturating_sub(self.update_tick) <= max_age
            && self.derelict_for().map(|held| held >= confirm_ticks).unwrap_or(false)
    }

    /// A sighting NOW would tip the derelict pipeline over a threshold it
    /// cannot cross on its own: the room was derelict at last sighting, the
    /// confirm window has elapsed since the first derelict sighting (so a
    /// fresh observation extends the observed span past `confirm_ticks`),
    /// and the classification is not already confirmed with actionable-age
    /// intel. Drives deliberate intel scouting (`SalvageOperation`) —
    /// confirmation otherwise waits on incidental re-scouts from the
    /// outpost/claim gathers, which only reach orthogonal distance-1 rooms
    /// and never refresh a diagonal neighbour at all.
    pub fn derelict_sighting_due_at(&self, now: u32, confirm_ticks: u32, max_age: u32) -> bool {
        self.derelict()
            && self
                .derelict_since
                .map(|since| now.saturating_sub(since) >= confirm_ticks)
                .unwrap_or(false)
            && !self.confirmed_derelict_at(now, confirm_ticks, max_age)
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

    pub fn update(&mut self, room: &Room, username: &str) {
        if self.static_visibility_data.is_none() {
            self.static_visibility_data = Some(Self::create_static_visibility_data(room));
        }

        self.dynamic_visibility_data = Some(self.create_dynamic_visibility_data(room, username));
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

    fn name_option_to_disposition(name: Option<String>, username: &str) -> RoomDisposition {
        name.map(|name| Self::name_to_disposition(name, username))
            .unwrap_or_else(|| RoomDisposition::Neutral)
    }

    /// Friendly/hostile classification against the bot's own identity
    /// (`username` from the [`crate::identity::BotIdentity`] Resource —
    /// statics-review M6).
    fn name_to_disposition(name: String, username: &str) -> RoomDisposition {
        let friends: &[String] = &[];

        if name == username {
            RoomDisposition::Mine
        } else if friends.iter().any(|friend_name| &name == friend_name) {
            RoomDisposition::Friendly(name)
        } else {
            RoomDisposition::Hostile(name)
        }
    }

    fn create_dynamic_visibility_data(&self, room: &Room, username: &str) -> RoomDynamicVisibilityData {
        let controller = room.controller();

        let controller_owner_name = controller.as_ref().and_then(|c| c.owner().map(|o| o.username()));
        let controller_owner_disposition = Self::name_option_to_disposition(controller_owner_name, username);

        let controller_reservation_name = controller.as_ref().and_then(|c| c.reservation()).map(|r| r.username());
        let controller_reservation_disposition = Self::name_option_to_disposition(controller_reservation_name, username);

        let sign = controller.as_ref().and_then(|c| c.sign()).map(|s| RoomSign {
            user: Self::name_to_disposition(s.username(), username),
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

        let tower_dps_at_edge = structures.as_ref().and_then(|s| {
            let positions: Vec<Position> = s.towers().iter().filter(|t| !t.my()).map(|t| t.pos()).collect();
            // None when there are NO hostile towers — NOT Some(0.0). With
            // `.map()` this was Some(0.0) for any room we had structure
            // visibility of, and `.is_some()` consumers (notably
            // is_claim_target_safe) read that as "hostile towers present",
            // vetoing the claim of every scouted neutral room — so no bot
            // could ever expand. Some(dps) iff there is real tower DPS.
            if positions.is_empty() {
                None
            } else {
                Some(crate::military::damage::tower_dps_at_room_edge(self.name, &positions))
            }
        });

        // is_active() filters out RCL-decayed / ownerless husks: a spawn that
        // cannot spawn and a tower that cannot fire are loot, not threats.
        let hostile_spawns = structures
            .as_ref()
            .map(|s| s.spawns().iter().any(|spawn| !spawn.my() && spawn.is_active()))
            .unwrap_or(false);

        let hostile_towers = structures
            .as_ref()
            .map(|s| {
                s.towers()
                    .iter()
                    .any(|t| !t.my() && t.is_active() && t.store().get_used_capacity(Some(ResourceType::Energy)) >= TOWER_ENERGY_COST)
            })
            .unwrap_or(false);

        let hostile_threat_creeps = self
            .get_creeps()
            .iter()
            .flat_map(|c| c.hostile())
            .flat_map(|c| c.body())
            .any(|p| !matches!(p.part(), Part::Move | Part::Tough));

        let safe_mode_end = controller
            .as_ref()
            .and_then(|c| c.safe_mode())
            .map(|remaining| game::time().saturating_add(remaining));

        let controller_level = controller.as_ref().map(|c| c.level());
        let controller_ticks_to_downgrade = controller.as_ref().and_then(|c| c.ticks_to_downgrade());

        let derelict =
            controller_owner_disposition.hostile() && !(hostile_creeps || hostile_spawns || hostile_towers || hostile_threat_creeps);
        let derelict_since = Self::next_derelict_since(
            derelict,
            &controller_owner_disposition,
            self.dynamic_visibility_data.as_ref().map(|previous| &previous.owner),
            self.dynamic_visibility_data.as_ref().and_then(|previous| previous.derelict_since),
            game::time(),
        );

        RoomDynamicVisibilityData {
            update_tick: game::time(),
            owner: controller_owner_disposition,
            reservation: controller_reservation_disposition,
            source_keeper,
            sign,
            hostile_creeps,
            hostile_structures,
            tower_dps_at_edge,
            hostile_spawns,
            hostile_towers,
            hostile_threat_creeps,
            safe_mode_end,
            controller_level,
            controller_ticks_to_downgrade,
            derelict_since,
        }
    }

    /// Carry-forward rule for the derelict-since mark (pure; host-tested):
    /// keep the existing mark only while the room stays derelict under the
    /// SAME hostile owner. Any threat/militarised sighting clears it (the
    /// `derelict_now` input is already false then), and an ownership change —
    /// a different player claiming the husk — restarts the confirmation clock
    /// from this sighting.
    fn next_derelict_since(
        derelict_now: bool,
        owner_now: &RoomDisposition,
        previous_owner: Option<&RoomDisposition>,
        previous_since: Option<u32>,
        now: u32,
    ) -> Option<u32> {
        if !derelict_now {
            return None;
        }

        let same_owner = previous_owner.map(|previous| previous == owner_now).unwrap_or(false);

        if same_owner {
            previous_since.or(Some(now))
        } else {
            Some(now)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hostile() -> RoomDisposition {
        RoomDisposition::Hostile("enemy".to_string())
    }

    fn dvd(update_tick: u32, owner: RoomDisposition, derelict_since: Option<u32>) -> RoomDynamicVisibilityData {
        RoomDynamicVisibilityData {
            update_tick,
            owner,
            reservation: RoomDisposition::Neutral,
            source_keeper: false,
            sign: None,
            hostile_creeps: false,
            hostile_structures: false,
            tower_dps_at_edge: None,
            hostile_spawns: false,
            hostile_towers: false,
            hostile_threat_creeps: false,
            safe_mode_end: None,
            controller_level: Some(3),
            controller_ticks_to_downgrade: Some(10_000),
            derelict_since,
        }
    }

    #[test]
    fn militarily_active_requires_capability() {
        let quiet = dvd(100, hostile(), Some(100));
        assert!(!quiet.militarily_active());

        for flag in ["creeps", "spawns", "towers"] {
            let mut armed = dvd(100, hostile(), None);
            match flag {
                "creeps" => armed.hostile_creeps = true,
                "spawns" => armed.hostile_spawns = true,
                _ => armed.hostile_towers = true,
            }
            assert!(armed.militarily_active(), "{} should militarise", flag);
            assert!(!armed.derelict(), "{} should break derelict", flag);
        }
    }

    #[test]
    fn threat_creeps_break_derelict_without_militarising() {
        // A hauler/claimer/healer sighting (no combat parts) must reset the
        // derelict clock even though it does not count as militarily active —
        // it could be refilling towers or maintaining the controller.
        let mut serviced = dvd(100, hostile(), Some(100));
        serviced.hostile_threat_creeps = true;

        assert!(!serviced.militarily_active());
        assert!(!serviced.derelict());
    }

    #[test]
    fn derelict_requires_hostile_owner() {
        assert!(!dvd(100, RoomDisposition::Neutral, None).derelict());
        assert!(!dvd(100, RoomDisposition::Mine, None).derelict());
        assert!(dvd(100, hostile(), Some(100)).derelict());
    }

    #[test]
    fn confirmation_requires_observed_span_not_wall_clock() {
        // One sighting at tick 100: zero observed span. Elapsed blind time
        // proves nothing - never confirmed, no matter how long we wait.
        let single = dvd(100, hostile(), Some(100));
        assert!(!single.confirmed_derelict_at(5_000, 2_000, 10_000));

        // Second sighting at 2_500 with the mark carried from 100: the span
        // (2_400) clears a 2_000-tick confirm window.
        let confirmed = dvd(2_500, hostile(), Some(100));
        assert!(confirmed.confirmed_derelict_at(2_600, 2_000, 10_000));

        // Same sightings, tighter window: not confirmed.
        assert!(!confirmed.confirmed_derelict_at(2_600, 3_000, 10_000));
    }

    #[test]
    fn stale_intel_falls_back_to_hostile() {
        let confirmed = dvd(2_500, hostile(), Some(100));

        assert!(confirmed.confirmed_derelict_at(12_500, 2_000, 10_000));
        assert!(!confirmed.confirmed_derelict_at(12_501, 2_000, 10_000));
    }

    #[test]
    fn sighting_due_waits_for_the_confirm_window() {
        // Sighting #1 at tick 100. A second sighting before 100 + confirm
        // (2_000) cannot confirm, so it is not requested; from 2_100 on it
        // can, so it is due.
        let single = dvd(100, hostile(), Some(100));
        assert!(!single.derelict_sighting_due_at(2_099, 2_000, 5_000));
        assert!(single.derelict_sighting_due_at(2_100, 2_000, 5_000));
    }

    #[test]
    fn sighting_not_due_while_confirmed_and_fresh() {
        // Confirmed (span 2_400) with intel well inside the action window:
        // no scouting needed, admission can act on what we have.
        let confirmed = dvd(2_500, hostile(), Some(100));
        assert!(!confirmed.derelict_sighting_due_at(2_600, 2_000, 5_000));

        // Same room once the intel ages out of the action window: the
        // confirmation has lapsed and a sighting would restore it.
        assert!(confirmed.derelict_sighting_due_at(7_501, 2_000, 5_000));
    }

    #[test]
    fn sighting_never_due_without_derelict_classification() {
        // Militarily active or threat-serviced rooms are not part of the
        // confirmation pipeline (their recheck runs on a slow cadence at the
        // call site instead).
        let mut armed = dvd(100, hostile(), None);
        armed.hostile_towers = true;
        assert!(!armed.derelict_sighting_due_at(50_000, 2_000, 5_000));

        assert!(!dvd(100, RoomDisposition::Neutral, None).derelict_sighting_due_at(50_000, 2_000, 5_000));
    }

    /// Relation pin: whenever a sighting is due, a sighting at `now` (same
    /// derelict owner, mark carried forward) yields a confirmed
    /// classification — the request is never wasted. And the two predicates
    /// are mutually exclusive: confirmed intel never asks for more eyes.
    #[test]
    fn sighting_due_implies_a_sighting_now_confirms() {
        let confirm_ticks = 2_000;
        let max_age = 5_000;

        for since in [100u32, 1_000, 4_000] {
            for update_tick in [100u32, 2_500, 4_000] {
                if update_tick < since {
                    continue;
                }
                for now in [update_tick, update_tick + 1_999, update_tick + 2_000, update_tick + 10_000] {
                    let state = dvd(update_tick, hostile(), Some(since));

                    assert!(
                        !(state.derelict_sighting_due_at(now, confirm_ticks, max_age)
                            && state.confirmed_derelict_at(now, confirm_ticks, max_age)),
                        "due and confirmed must be mutually exclusive (since {}, seen {}, now {})",
                        since,
                        update_tick,
                        now
                    );

                    if state.derelict_sighting_due_at(now, confirm_ticks, max_age) {
                        // Simulate the requested sighting: update_tick moves to
                        // `now`, the derelict-since mark carries (same owner,
                        // still derelict).
                        let after_sighting = dvd(now, hostile(), Some(since));
                        assert!(
                            after_sighting.confirmed_derelict_at(now, confirm_ticks, max_age),
                            "a due sighting must confirm (since {}, seen {}, now {})",
                            since,
                            update_tick,
                            now
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn derelict_since_carries_only_under_same_owner() {
        let enemy = hostile();
        let other = RoomDisposition::Hostile("other".to_string());

        // Same owner, still derelict: mark carried.
        assert_eq!(
            RoomData::next_derelict_since(true, &enemy, Some(&enemy), Some(100), 2_500),
            Some(100)
        );
        // Ownership changed hands: confirmation clock restarts.
        assert_eq!(
            RoomData::next_derelict_since(true, &other, Some(&enemy), Some(100), 2_500),
            Some(2_500)
        );
        // First derelict sighting ever: clock starts now.
        assert_eq!(RoomData::next_derelict_since(true, &enemy, None, None, 2_500), Some(2_500));
        // Not derelict: no mark, regardless of history.
        assert_eq!(RoomData::next_derelict_since(false, &enemy, Some(&enemy), Some(100), 2_500), None);
    }
}
