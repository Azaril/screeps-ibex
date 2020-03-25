use crate::findnearest::*;
use crate::visualize::*;
use screeps::*;
use serde::*;
use std::collections::*;
use std::collections::hash_map::*;
use std::convert::*;
use bitflags::*;
use log::*;

const ROOM_WIDTH: u8 = 50;
const ROOM_HEIGHT: u8 = 50;
const ROOM_BUILD_BORDER: u8 = 2;

fn in_room_bounds<T>(x: T, y: T) -> bool where T: Into<i32> {
    let x = x.into();
    let y = y.into();

    x >= 0 && x < (ROOM_WIDTH as i32) && 
    y >= 0 && y < (ROOM_HEIGHT as i32)
}

fn in_room_bounds_unsigned<T>(x: T, y: T) -> bool where T: Into<u32> {
    let x = x.into();
    let y = y.into();

    x < (ROOM_WIDTH as u32) && 
    y < (ROOM_HEIGHT as u32)
}

fn in_room_build_bounds<T>(x: T, y: T) -> bool where T: Into<i32> {
    let x = x.into();
    let y = y.into();

    (x >= (0 + ROOM_BUILD_BORDER) as i32) && 
    (x < (ROOM_WIDTH - ROOM_BUILD_BORDER) as i32) && 
    (y >= 0 + (ROOM_BUILD_BORDER) as i32) && 
    (y < (ROOM_HEIGHT - ROOM_BUILD_BORDER) as i32)
}

fn in_room_build_bounds_unsigned<T>(x: T, y: T) -> bool where T: Into<u32> {
    let x = x.into();
    let y = y.into();

    (x >= (0 + ROOM_BUILD_BORDER) as u32) && 
    (x < (ROOM_WIDTH - ROOM_BUILD_BORDER) as u32) && 
    (y >= 0 + (ROOM_BUILD_BORDER) as u32) && 
    (y < (ROOM_HEIGHT - ROOM_BUILD_BORDER) as u32)
}

trait InBounds {
    fn in_room_bounds(&self) -> bool;

    fn in_room_build_bounds(&self) -> bool;
}

trait InBoundsUnsigned {
    fn in_room_bounds(&self) -> bool;

    fn in_room_build_bounds(&self) -> bool;
}

impl<T> InBounds for (T, T) where T: Into<i32> + Copy {
    fn in_room_bounds(&self) -> bool {
        in_room_bounds(self.0, self.1)
    }

    fn in_room_build_bounds(&self) -> bool {
        in_room_build_bounds(self.0, self.1)
    }
}

impl<T> InBoundsUnsigned for (T, T) where T: Into<u32> + Copy {
    fn in_room_bounds(&self) -> bool {
        in_room_bounds_unsigned(self.0, self.1)
    }

    fn in_room_build_bounds(&self) -> bool {
        in_room_build_bounds_unsigned(self.0, self.1)
    }
}

#[derive(Copy, Clone, Serialize, Deserialize, Debug)]
pub struct RoomItem {
    #[serde(rename = "s")]
    structure_type: StructureType,
    #[serde(rename = "r")]
    required_rcl: u32,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
#[repr(transparent)]
pub struct Location {
    packed: u16,
}

impl Location {
    fn from_coords(x: u32, y: u32) -> Self {
        Location {
            packed: ((x << 8) | y) as u16,
        }
    }

    fn from_pos(pos: RoomPosition) -> Self {
        Self::from_coords(pos.x(), pos.y())
    }

    #[inline]
    pub fn x(self) -> u8 {
        ((self.packed >> 8) & 0xFF) as u8
    }

    #[inline]
    pub fn y(self) -> u8 {
        (self.packed & 0xFF) as u8
    }

    #[inline]
    pub fn packed_repr(self) -> u16 {
        self.packed
    }

    #[inline]
    pub fn from_packed(packed: u16) -> Self {
        Location { packed }
    }

    pub fn to_room_position(self, room: RoomName) -> RoomPosition {
        RoomPosition::new(self.x() as u32, self.y() as u32, room)
    }

    pub fn distance_to(self, other: Self) -> u8 {
        let dx = (self.x() as i8) - (other.x() as i8);
        let dy = (self.y() as i8) - (other.y() as i8);

        dx.abs().max(dy.abs()) as u8
    }

    pub fn distance_to_xy(self, x: i8, y: i8) -> u8 {
        let dx = (self.x() as i8) - x;
        let dy = (self.y() as i8) - y;

        dx.abs().max(dy.abs()) as u8
    }
}

impl InBoundsUnsigned for Location {
    fn in_room_bounds(&self) -> bool {
        in_room_bounds_unsigned(self.x(), self.y())
    }

    fn in_room_build_bounds(&self) -> bool {
        in_room_build_bounds_unsigned(self.x(), self.y())
    }   
}

impl TryFrom<PlanLocation> for Location {
    type Error = ();

    fn try_from(value: PlanLocation) -> Result<Self, Self::Error> {
        if value.in_room_bounds() {
            Ok(Location::from_coords(value.x() as u32, value.y() as u32))
        } else {
            Err(())
        }
    }
}

impl Serialize for Location {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.packed_repr().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Location {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        u16::deserialize(deserializer).map(Location::from_packed)
    }
}

pub type PlanState = HashMap<Location, RoomItem>;

#[derive(Clone, Serialize, Deserialize)]
pub struct PlannerStateLayer {
    #[serde(rename = "d")]
    data: HashMap<Location, RoomItem>,
    #[serde(rename = "s")]
    structure_counts: HashMap<StructureType, u8>
}

impl PlannerStateLayer {
    pub fn new() -> PlannerStateLayer {
        PlannerStateLayer {
            data: HashMap::new(),
            structure_counts: HashMap::new()
        }
    }

    pub fn insert(&mut self, location: Location, item: RoomItem) {
        if let Some(current) = self.data.insert(location, item) {
            let old_count = self.structure_counts
                .entry(current.structure_type)
                .or_insert(0);

            *old_count -= 1;
        }

        let current_count = self.structure_counts
            .entry(item.structure_type)
            .or_insert(0);

        *current_count += 1;
    }

    pub fn get(&self, location: &Location) -> Option<&RoomItem> {
        self.data.get(location)
    }

    pub fn get_count(&self, structure_type: StructureType) -> u8 {
        *self.structure_counts.get(&structure_type).unwrap_or(&0)
    }

    pub fn get_locations(&self, structure_type: StructureType) -> Vec<Location> {
        if self.get_count(structure_type) > 0 {
            self.data
                .iter()
                .filter(|(_, entry)| entry.structure_type == structure_type)
                .map(|(location, _)| *location)
                .collect()
        } else {
            Vec::new()
        }
    }

    pub fn complete(self) -> HashMap<Location, RoomItem> {
        self.data
    }

    pub fn visualize(&self, visualizer: &mut RoomVisualizer) {
        visualize_room_items(&self.data, visualizer);
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PlannerState {
    #[serde(rename = "l")]
    layers: Vec<PlannerStateLayer>,
}

impl PlannerState {
    pub fn new() -> PlannerState {
        PlannerState {
            layers: vec![PlannerStateLayer::new()]
        }
    }

    pub fn push_layer(&mut self) {
        self.layers.push(PlannerStateLayer::new());
    }

    fn pop_layer(&mut self) {
        self.layers.pop();
    }

    pub fn get(&self, location: &Location) -> Option<&RoomItem> {
        for layer in self.layers.iter().rev() {
            let entry = layer.get(location);

            if entry.is_some() {
                return entry;
            }
        }

        None
    }

    pub fn get_count(&self, structure_type: StructureType) -> u8 {
        self.layers.iter().map(|l| l.get_count(structure_type)).sum()
    }

    pub fn get_locations(&self, structure_type: StructureType) -> Vec<Location> {
        self.layers.iter().flat_map(|l| l.get_locations(structure_type)).collect()
    }

    pub fn insert(&mut self, location: Location, item: RoomItem) {
        let layer = self.layers.last_mut().unwrap();

        layer.insert(location, item);
    }

    pub fn snapshot(&self) -> PlanState {
        let mut state = PlanState::new();

        for layer in &self.layers {
            for (location, item) in layer.data.iter() {
                state.insert(*location, *item);
            }
        }

        state
    }

    pub fn visualize(&self, visualizer: &mut RoomVisualizer) {
        for layer in &self.layers {
            layer.visualize(visualizer);
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub struct PlanLocation {
    x: i8,
    y: i8
}

impl PlanLocation {
    pub fn new(x: i8, y: i8) -> PlanLocation {
        PlanLocation {
            x,
            y
        }
    }

    pub fn x(&self) -> i8 {
        self.x
    }
    
    pub fn y(&self) -> i8 {
        self.y
    }

    pub fn as_location(&self) -> Option<Location> {
        if self.in_room_bounds() {
            Some(Location::from_coords(self.x as u32, self.y as u32))
        } else {
            None
        }
    }

    pub fn as_build_location(&self) -> Option<Location> {
        if self.in_room_build_bounds() {
            Some(Location::from_coords(self.x as u32, self.y as u32))
        } else {
            None
        }
    }

    pub fn distance_to(self, other: Self) -> u8 {
        let dx = self.x() - other.x();
        let dy = self.y() - other.y();

        dx.abs().max(dy.abs()) as u8
    }

    pub fn distance_to_xy(self, x: i8, y: i8) -> u8 {
        let dx = self.x() - x;
        let dy = self.y() - y;

        dx.abs().max(dy.abs()) as u8
    }
    
    #[inline]
    pub fn packed_repr(self) -> u16 {
        let x = (self.x as u8) as u16;
        let y = (self.y as u8) as u16;

        x | (y << 8)
    }

    #[inline]
    pub fn from_packed(packed: u16) -> Self {
        let x = ((packed & 0xFF) as u8) as i8;
        let y = (((packed >> 8) & 0xFF) as u8) as i8;

        PlanLocation { x, y }
    }
}

impl Serialize for PlanLocation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.packed_repr().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PlanLocation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        u16::deserialize(deserializer).map(PlanLocation::from_packed)
    }
}

impl From<Location> for PlanLocation {
    fn from(value: Location) -> PlanLocation {
        PlanLocation {
            x: value.x() as i8,
            y: value.y() as i8
        }
    }
}

impl From<&Location> for PlanLocation {
    fn from(value: &Location) -> PlanLocation {
        PlanLocation::from(*value)
    }
}


impl InBounds for PlanLocation {
    fn in_room_bounds(&self) -> bool {
        in_room_bounds(self.x(), self.y())
    }

    fn in_room_build_bounds(&self) -> bool {
        in_room_build_bounds(self.x(), self.y())
    }   
}

impl std::ops::Add for PlanLocation {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
        }
    }
}

impl std::ops::Add<(i8, i8)> for PlanLocation {
    type Output = Self;

    fn add(self, other: (i8, i8)) -> Self {
        Self {
            x: self.x + other.0,
            y: self.y + other.1,
        }
    }
}


impl std::ops::Add<&(i8, i8)> for PlanLocation {
    type Output = Self;

    fn add(self, other: &(i8, i8)) -> Self {
        Self {
            x: self.x + other.0,
            y: self.y + other.1,
        }
    }
}

impl std::ops::Sub for PlanLocation {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
        }
    }
}

fn visualize_room_items<'a, T: IntoIterator<Item = (&'a Location, &'a RoomItem)>>(data: T, visualizer: &mut RoomVisualizer) {
    for (loc, entry) in data.into_iter() {
        match entry {
            RoomItem { structure_type: StructureType::Spawn, .. } => {
                visualizer.circle(
                    loc.x() as f32,
                    loc.y() as f32,
                    Some(CircleStyle::default().fill("green").opacity(1.0)),
                );
            }
            RoomItem { structure_type: StructureType::Extension, .. } => {
                visualizer.circle(
                    loc.x() as f32,
                    loc.y() as f32,
                    Some(CircleStyle::default().fill("purple").opacity(1.0)),
                );
            }
            RoomItem { structure_type: StructureType::Container, .. } => {
                visualizer.circle(
                    loc.x() as f32,
                    loc.y() as f32,
                    Some(CircleStyle::default().fill("blue").opacity(1.0)),
                );
            }
            RoomItem { structure_type: StructureType::Storage, .. } => {
                visualizer.circle(
                    loc.x() as f32,
                    loc.y() as f32,
                    Some(CircleStyle::default().fill("red").opacity(1.0)),
                );
            }
            RoomItem { structure_type: StructureType::Link, .. } => {
                visualizer.circle(
                    loc.x() as f32,
                    loc.y() as f32,
                    Some(CircleStyle::default().fill("orange").opacity(1.0)),
                );
            }
            RoomItem { structure_type: StructureType::Terminal, .. } => {
                visualizer.circle(
                    loc.x() as f32,
                    loc.y() as f32,
                    Some(CircleStyle::default().fill("pink").opacity(1.0)),
                );
            }
            RoomItem { structure_type: StructureType::Nuker, .. } => {
                visualizer.circle(
                    loc.x() as f32,
                    loc.y() as f32,
                    Some(CircleStyle::default().fill("black").opacity(1.0)),
                );
            }
            RoomItem { .. } => {
                visualizer.circle(
                    loc.x() as f32,
                    loc.y() as f32,
                    Some(CircleStyle::default().fill("yellow").opacity(1.0)),
                );
            }
        }
    }
}


#[derive(Clone, Serialize, Deserialize)]
pub struct Plan {
    #[serde(rename = "s")]
    state: PlanState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Plan {
    pub fn execute(&self, room: &Room) {
        let room_name = room.name();
        let room_level = room.controller().map(|c| c.level()).unwrap_or(0);

        for (loc, entry) in self.state.iter() {
            if room_level >= entry.required_rcl {
                room.create_construction_site(&RoomPosition::new(loc.x() as u32, loc.y() as u32, room_name), entry.structure_type);
            }
        }
    }

    pub fn visualize(&self, visualizer: &mut RoomVisualizer) {
        visualize_room_items(&self.state, visualizer);
    }
}

struct RoomDataArrayIterator<'a, T> where T: Copy {
    data: &'a RoomDataArray<T>,
    x: u8,
    y: u8
}

impl<'a, T> Iterator for RoomDataArrayIterator<'a, T> where T: Copy {
    type Item = ((usize, usize), &'a T);

     fn next(&mut self) -> Option<Self::Item> {
        if self.x < ROOM_WIDTH && self.y < ROOM_HEIGHT {
            let current_x = self.x as usize;
            let current_y = self.y as usize;

            self.x += 1;
    
            if self.x >= ROOM_WIDTH {
                self.x = 0;
                self.y += 1;
            }
            
            Some(((current_x, current_y), self.data.get(current_x, current_y)))
        } else { 
            None
        }
    }
}

struct RoomDataArray<T> where T: Copy {
    data: [T; (ROOM_WIDTH as usize) * (ROOM_HEIGHT as usize)]
}

impl<T> RoomDataArray<T> where T: Copy {
    pub fn new(initial: T) -> Self {
        RoomDataArray {
            data: [initial; (ROOM_WIDTH as usize) * (ROOM_HEIGHT as usize)]
        }
    }

    pub fn get(&self, x: usize, y: usize) -> &T {
        let index = (y * (ROOM_WIDTH as usize)) + x;
        &self.data[index]
    }

    pub fn get_mut(&mut self, x: usize, y: usize) -> &mut T {
        let index = (y * (ROOM_WIDTH as usize)) + x;
        &mut self.data[index]
    }

    pub fn set(&mut self, x: usize, y: usize , value: T) {
        *self.get_mut(x, y) = value;
    }

    pub fn iter(&self) -> impl Iterator<Item = ((usize, usize), &T)> {
        RoomDataArrayIterator {
            data: &self,
            x: 0,
            y: 0
        }
    }
}

#[derive(Clone)]
pub enum PlanNodeChild<'a> {
    GlobalPlacement(&'a dyn PlanGlobalPlacementNode),
    LocationPlacement(PlanLocation, &'a dyn PlanLocationPlacementNode)
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanNodeChild<'a> {
    fn place(&self, terrain: &FastRoomTerrain, state: &mut PlannerState) -> Result<(), ()> {
        match self {
            PlanNodeChild::GlobalPlacement(node) => node.place(terrain, state),
            PlanNodeChild::LocationPlacement(location, node) => node.place(*location, terrain, state)
        }
    }

    fn get_score(&self, terrain: &FastRoomTerrain, state: &PlannerState) -> Option<f32> {
        match self {
            PlanNodeChild::GlobalPlacement(node) => node.get_score(terrain, state),
            PlanNodeChild::LocationPlacement(location, node) => node.get_score(*location, terrain, state)
        }
    }

    fn mark_visited(&self, gather_data: &mut PlanGatherChildrenData<'a>) {
        match self {
            PlanNodeChild::GlobalPlacement(node) => gather_data.mark_visited_global(node.as_global()),
            PlanNodeChild::LocationPlacement(location, node) => gather_data.mark_visited_location(*location, node.as_location()),
        }
    }

    fn get_children(&self, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'a>) {
        match self {
            PlanNodeChild::GlobalPlacement(node) => node.get_children(terrain, state, gather_data),
            PlanNodeChild::LocationPlacement(location, node) => node.get_children(*location, terrain, state, gather_data),
        }
    }

    fn desires_placement(&self, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'a>) -> bool {
        match self {
            PlanNodeChild::GlobalPlacement(node) => gather_data.desires_placement(node.as_base(), terrain, state),
            PlanNodeChild::LocationPlacement(_, node) => gather_data.desires_placement(node.as_base(), terrain, state),
        }
    }

    fn desires_location(&self, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'a>) -> bool {
        match self {
            PlanNodeChild::GlobalPlacement(_) => true,
            PlanNodeChild::LocationPlacement(location, node) => gather_data.desires_location(*location, node.as_location(), terrain, state),
        }
    }

    fn insert(&self, gather_data: &mut PlanGatherChildrenData<'a>) -> bool {
        match self {
            PlanNodeChild::GlobalPlacement(node) => gather_data.insert_global_placement(*node),
            PlanNodeChild::LocationPlacement(location, node) => gather_data.insert_location_placement(*location, *node),
        }
    }

    fn to_serialized(&self, index_lookup: &HashMap<uuid::Uuid, usize>) -> SerializedPlanNodeChild {
        match self {
            PlanNodeChild::GlobalPlacement(node) => {
                let node_type = 0;
                let node = index_lookup.get(node.id()).unwrap();
                if (node & !0x7F) != 0 {
                    panic!("Not enough bits to represent packed value!");
                }
                let node = node & 0x7F;

                let packed = (node_type) | ((node as u32) << 1);

                SerializedPlanNodeChild {
                    packed
                }
            },
            PlanNodeChild::LocationPlacement(location, node) => {
                let node_type = 1;
                let location = location.packed_repr();
                let node = index_lookup.get(node.id()).unwrap();
                if (node & !0x7F) != 0 {
                    panic!("Not enough bits to represent packed value!");
                }
                let node = node & 0x7F;

                let packed = (node_type) | ((node as u32) << 1) | ((location as u32) << 16);

                SerializedPlanNodeChild {
                    packed
                }
            }
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
struct SerializedPlanNodeChild {
    packed: u32
}

impl SerializedPlanNodeChild {
    pub fn as_entry<'b>(&self, nodes: &PlanGatherNodesData<'b>, index_lookup: &Vec<uuid::Uuid>) -> Result<PlanNodeChild<'b>, ()> {
        let node_type = self.packed & 0x1;

        match node_type {
            0 => {
                let node_index = (self.packed >> 1) & 0x7F;
                let node_id = index_lookup.get(node_index as usize).ok_or(())?;
                let node = nodes.global_placement_nodes.get(node_id).ok_or(())?;
        
                Ok(PlanNodeChild::GlobalPlacement(*node))
            }
            1 => {
                let node_index = (self.packed >> 1) & 0x7F;
                let node_id = index_lookup.get(node_index as usize).ok_or(())?;
                let node = nodes.location_placement_nodes.get(node_id).ok_or(())?;

                let location = PlanLocation::from_packed((self.packed >> 16) as u16);
        
                Ok(PlanNodeChild::LocationPlacement(location, *node))
            }
            _ => Err(())
        }
    }
}

pub struct PlanGatherNodesData<'b> {
    global_placement_nodes: HashMap<uuid::Uuid, &'b dyn PlanGlobalPlacementNode>,
    location_placement_nodes: HashMap<uuid::Uuid, &'b dyn PlanLocationPlacementNode>
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'b> PlanGatherNodesData<'b> {
    pub fn new<'a>() -> PlanGatherNodesData<'a> {
        PlanGatherNodesData {
            global_placement_nodes: HashMap::new(),
            location_placement_nodes: HashMap::new()
        }
    }

    pub fn get_all_ids(&self) -> Vec<uuid::Uuid> {
        self.global_placement_nodes.keys().chain(self.location_placement_nodes.keys()).cloned().collect()
    }

    pub fn insert_global_placement(&mut self, id: uuid::Uuid, node: &'b dyn PlanGlobalPlacementNode) -> bool {
        match self.global_placement_nodes.entry(id) {
            Entry::Occupied(_) => false,
            Entry::Vacant(e) => {
                e.insert(node);

                true
            }
        }
    }

    pub fn insert_location_placement(&mut self, id: uuid::Uuid, node: &'b dyn PlanLocationPlacementNode) -> bool {
        match self.location_placement_nodes.entry(id) {
            Entry::Occupied(_) => false,
            Entry::Vacant(e) => {
                e.insert(node);

                true
            }
        }
    }
}
struct PlanGatherChildrenGlobalData<'s> {
    visited: Vec<&'s dyn PlanGlobalNode>,
    inserted: Vec<&'s dyn PlanGlobalPlacementNode>
}

impl<'s> PlanGatherChildrenGlobalData<'s> {
    pub fn has_visited(&self, node: &dyn PlanGlobalNode) -> bool {
        self.visited.iter().any(|other| std::ptr::eq(node, *other))
    }

    pub fn mark_visited(&mut self, node: &'s dyn PlanGlobalNode) {
        if !self.has_visited(node) {
            self.visited.push(node);
        }
    }

    pub fn insert(&mut self, node: &'s dyn PlanGlobalPlacementNode) -> bool {
        if !self.inserted.iter().any(|other| std::ptr::eq(node, *other)) {
            self.inserted.push(node);

            true
        } else {
            false
        }
    }
}

struct PlanGatherChildrenLocationData<'s> {
    desires_location_cache: Vec<(&'s dyn PlanLocationNode, bool)>,
    visited: Vec<&'s dyn PlanLocationNode>,
    inserted: Vec<&'s dyn PlanLocationPlacementNode>
}

impl<'s> PlanGatherChildrenLocationData<'s> {
    pub fn has_visited(&self, node: &dyn PlanLocationNode) -> bool {
        self.visited.iter().any(|other| std::ptr::eq(node, *other))
    }

    pub fn mark_visited(&mut self, node: &'s dyn PlanLocationNode) {
        if !self.has_visited(node) {
            self.visited.push(node);
        }
    }

    pub fn insert(&mut self, node: &'s dyn PlanLocationPlacementNode) -> bool {
        if !self.inserted.iter().any(|other| std::ptr::eq(node, *other)) {
            self.inserted.push(node);

            true
        } else {
            false
        }
    }
}

pub struct PlanGatherChildrenData<'a> {
    desires_placement_cache: Vec<(&'a dyn PlanBaseNode, bool)>,
    global_nodes: PlanGatherChildrenGlobalData<'a>,
    location_nodes: HashMap<PlanLocation, PlanGatherChildrenLocationData<'a>>
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanGatherChildrenData<'a> {
    pub fn new<'b>() -> PlanGatherChildrenData<'b> {
        PlanGatherChildrenData {
            desires_placement_cache: Vec::new(),
            global_nodes: PlanGatherChildrenGlobalData {
                visited: Vec::new(),
                inserted: Vec::new(),
            },
            location_nodes: HashMap::new(),
        }
    }
    
    pub fn desires_placement(&mut self, node: &'a dyn PlanBaseNode, terrain: &FastRoomTerrain, state: &PlannerState) -> bool {
        match self.desires_placement_cache.iter().position(|(other, _)| std::ptr::eq(node, *other)) {
            Some(index) => self.desires_placement_cache[index].1,
            None => {
                let desires_placement = node.desires_placement(terrain, state, self);

                self.desires_placement_cache.push((node, desires_placement));

                desires_placement
            }
        }
    }

    pub fn desires_location(&mut self, position: PlanLocation, node: &'a dyn PlanLocationNode, terrain: &FastRoomTerrain, state: &PlannerState) -> bool {
        {
            if let Some(location_data) = self.try_get_location_data(position) {
                if let Some(index) = location_data.desires_location_cache.iter().position(|(other, _)| std::ptr::eq(node, *other)) {
                    return location_data.desires_location_cache[index].1;
                }
            }
        }

        let desires_location = node.desires_location(position, terrain, state, self);

        let location_data = self.get_location_data(position);

        if !location_data.desires_location_cache.iter().any(|(other, _)| std::ptr::eq(node, *other)) {
            location_data.desires_location_cache.push((node, desires_location));
        }

        desires_location
    }

    fn get_location_data(&mut self, position: PlanLocation) -> &mut PlanGatherChildrenLocationData<'a> {
        self.location_nodes
            .entry(position)
            .or_insert_with(|| {
                PlanGatherChildrenLocationData {
                    desires_location_cache: Vec::new(),
                    visited: Vec::new(),
                    inserted: Vec::new()
                }
            })
    }

    fn try_get_location_data(&self, position: PlanLocation) -> Option<&PlanGatherChildrenLocationData<'a>> {
        self.location_nodes.get(&position)
    }

    pub fn has_visited_global(&self, node: &'a dyn PlanGlobalNode) -> bool {
        self.global_nodes.has_visited(node)
    }

    pub fn mark_visited_global(&mut self, node: &'a dyn PlanGlobalNode) {
        self.global_nodes.mark_visited(node);
    }

    pub fn has_visited_location(&self, position: PlanLocation, node: &'a dyn PlanLocationNode) -> bool {
        self.try_get_location_data(position).map(|l| l.has_visited(node)).unwrap_or(false)
    }

    pub fn mark_visited_location(&mut self, position: PlanLocation, node: &'a dyn PlanLocationNode) {
        let location_data = self.get_location_data(position);

        location_data.mark_visited(node);
    }

    pub fn insert_global_placement(&mut self, node: &'a dyn PlanGlobalPlacementNode) -> bool {
        self.global_nodes.insert(node)
    }

    pub fn insert_location_placement(&mut self, position: PlanLocation, node: &'a dyn PlanLocationPlacementNode) -> bool {
        let location_data = self.get_location_data(position);

        location_data.insert(node)
    }

    pub fn collect(self) -> Vec<PlanNodeChild<'a>> {
        let globals = self.global_nodes.inserted.iter().map(|node| PlanNodeChild::GlobalPlacement(*node));

        self
            .location_nodes
            .iter()
            .flat_map(|(location, location_data)| location_data.inserted.iter().map(move |node| PlanNodeChild::LocationPlacement(*location, *node)))
            .chain(globals)
            .collect()
    }
}

pub trait PlanBaseNode {
    fn name(&self) -> &str;

    fn desires_placement<'s>(&'s self, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) -> bool;

    fn gather_nodes<'b>(&'b self, data: &mut PlanGatherNodesData<'b>);
}

pub trait PlanGlobalNode: PlanBaseNode {
    fn as_base(&self) -> &dyn PlanBaseNode;

    fn get_children<'s>(&'s self, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>);
}

pub trait PlanGlobalPlacementNode: PlanGlobalNode {
    fn as_global(&self) -> &dyn PlanGlobalNode;

    fn id(&self) -> &uuid::Uuid;

    fn get_score(&self, terrain: &FastRoomTerrain, state: &PlannerState) -> Option<f32>;

    fn place(&self, terrain: &FastRoomTerrain, state: &mut PlannerState) -> Result<(), ()>;
}

pub trait PlanGlobalExpansionNode: PlanGlobalNode {
    fn as_global(&self) -> &dyn PlanGlobalNode;
}

pub trait PlanLocationNode: PlanBaseNode {
    fn as_base(&self) -> &dyn PlanBaseNode;

    fn desires_location<'s>(&'s self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) -> bool;

    fn get_children<'s>(&'s self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>);
}

pub trait PlanLocationPlacementNode: PlanLocationNode {
    fn as_location(&self) -> &dyn PlanLocationNode;

    fn id(&self) -> &uuid::Uuid;

    fn get_score(&self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState) -> Option<f32>;

    fn place(&self, position: PlanLocation, terrain: &FastRoomTerrain, state: &mut PlannerState) -> Result<(), ()>;
}

pub trait PlanPlacementExpansionNode: PlanLocationNode {
    fn as_location(&self) -> &dyn PlanLocationNode;
}

enum PlanNodeStorage<'a> {
    Empty,
    GlobalPlacement(&'a dyn PlanGlobalPlacementNode),
    GlobalExpansion(&'a dyn PlanGlobalExpansionNode),
    LocationPlacement(&'a dyn PlanLocationPlacementNode),
    LocationExpansion(&'a dyn PlanPlacementExpansionNode),
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanNodeStorage<'a> {
    fn gather_nodes(&self, data: &mut PlanGatherNodesData<'a>) {
        match self {
            PlanNodeStorage::Empty => {},
            PlanNodeStorage::GlobalPlacement(n) => n.gather_nodes(data),
            PlanNodeStorage::GlobalExpansion(n) => n.gather_nodes(data),
            PlanNodeStorage::LocationPlacement(n) => n.gather_nodes(data),
            PlanNodeStorage::LocationExpansion(n) => n.gather_nodes(data)
        }
    }

    fn desires_placement(&self, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'a>) -> bool {
        match self {
            PlanNodeStorage::Empty => false,
            PlanNodeStorage::GlobalPlacement(n) => gather_data.desires_placement(n.as_base(), terrain, state),
            PlanNodeStorage::GlobalExpansion(n) => gather_data.desires_placement(n.as_base(), terrain, state),
            PlanNodeStorage::LocationPlacement(n) => gather_data.desires_placement(n.as_base(), terrain, state),
            PlanNodeStorage::LocationExpansion(n) => gather_data.desires_placement(n.as_base(), terrain, state)
        }
    }

    fn desires_location(&self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'a>) -> bool {
        match self {
            PlanNodeStorage::Empty => false,
            PlanNodeStorage::GlobalPlacement(_) => true,
            PlanNodeStorage::GlobalExpansion(_) => true,
            PlanNodeStorage::LocationPlacement(n) => gather_data.desires_location(position, n.as_location(), terrain, state),
            PlanNodeStorage::LocationExpansion(n) => gather_data.desires_location(position, n.as_location(), terrain, state)
        }
    }

    fn insert_or_expand(&self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'a>) {
        match self {
            PlanNodeStorage::Empty => {},
            PlanNodeStorage::GlobalPlacement(n) => { gather_data.insert_global_placement(*n); },
            PlanNodeStorage::GlobalExpansion(n) => n.get_children(terrain, state, gather_data),
            PlanNodeStorage::LocationPlacement(n) => { gather_data.insert_location_placement(position, *n); },
            PlanNodeStorage::LocationExpansion(n) => n.get_children(position, terrain, state, gather_data)
        }
    }
}

struct PlanPlacement {
    structure_type: StructureType,
    offset: PlanLocation
}

fn flood_fill_distance(initial_seeds: HashSet<PlanLocation>, terrain: &FastRoomTerrain, data: &mut RoomDataArray<Option<u32>>) {
    let mut to_apply = initial_seeds;
    let mut current_distance: u32 = 0;

    while !to_apply.is_empty() {
        let eval_locations = std::mem::replace(&mut to_apply, HashSet::new());

        for pos in &eval_locations {
            let current = data.get_mut(pos.x() as usize, pos.y() as usize);

            if current.is_none() {
                *current = Some(current_distance);

                for x_delta in -1i8..=1i8 {
                    for y_delta in -1i8..=1i8 {
                        if x_delta != 0 && y_delta != 0 {
                            let next_location = *pos + (x_delta, y_delta);
                            if next_location.in_room_bounds() {
                                let terrain = terrain.get_xy(next_location.x() as u8, next_location.y() as u8); 
                                if !terrain.contains(TerrainFlags::WALL) {
                                    to_apply.insert(next_location);
                                }
                            }
                        }
                    }
                }
            }
        }

        current_distance += 1;
    }
}

struct PlaceAwayFromWallsNode<'a> {
    wall_distance: u32,
    child: PlanNodeStorage<'a>
}

impl<'a> PlaceAwayFromWallsNode<'a> {
    fn get_wall_min_distance_locations(terrain: &FastRoomTerrain, min_distance: u32) -> Vec<PlanLocation> {
        let mut data: RoomDataArray<Option<u32>> = RoomDataArray::new(None);

        let mut to_apply: HashSet<PlanLocation> = HashSet::new();

        for y in 0..ROOM_HEIGHT {
            for x in 0..ROOM_WIDTH {
                let terrain = terrain.get_xy(x, y);

                if terrain.contains(TerrainFlags::WALL) || !in_room_build_bounds(x, y) {
                    to_apply.insert(PlanLocation::new(x as i8, y as i8));
                }
            }
        }

        flood_fill_distance(to_apply, terrain, &mut data);

        data
            .iter()
            .filter(|(_, distance)| distance.map(|d| d >= min_distance).unwrap_or(false))
            .map(|((x, y), _)| PlanLocation::new(x as i8, y as i8))
            .collect()
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanBaseNode for PlaceAwayFromWallsNode<'a> {
    fn name(&self) -> &str {
        "Place Away From Walls"
    }

    fn gather_nodes<'b>(&'b self, data: &mut PlanGatherNodesData<'b>) {
        self.child.gather_nodes(data);
    }

    fn desires_placement<'s>(&'s self, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) -> bool {
        self.child.desires_placement(terrain, state, gather_data)
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanGlobalNode for PlaceAwayFromWallsNode<'a> {
    fn as_base(&self) -> &dyn PlanBaseNode {
        self
    }

    fn get_children<'s>(&'s self, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) {
        if !gather_data.has_visited_global(self) {
            gather_data.mark_visited_global(self);

            if self.child.desires_placement(terrain, state, gather_data) {
                let locations = Self::get_wall_min_distance_locations(terrain, self.wall_distance);

                for location in &locations {
                    if self.child.desires_location(*location, terrain, state, gather_data) {
                        self.child.insert_or_expand(*location, terrain, state, gather_data);
                    }
                }
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanGlobalExpansionNode for PlaceAwayFromWallsNode<'a> {
    fn as_global(&self) -> &dyn PlanGlobalNode {
        self
    }
}

struct FixedPlanNode<'a> {
    id: uuid::Uuid,
    placements: &'a [PlanPlacement],
    child: PlanNodeStorage<'a>,
    desires_placement: fn(terrain: &FastRoomTerrain, state: &PlannerState) -> bool,
    scorer: fn(position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState) -> Option<f32>
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanBaseNode for FixedPlanNode<'a> {
    fn name(&self) -> &str {
        "Fixed"
    }

    fn gather_nodes<'b>(&'b self, data: &mut PlanGatherNodesData<'b>) {
        if data.insert_location_placement(self.id, self) {
            self.child.gather_nodes(data);
        }
    }

    fn desires_placement<'s>(&'s self, terrain: &FastRoomTerrain, state: &PlannerState, _gather_data: &mut PlanGatherChildrenData<'s>) -> bool {
        (self.desires_placement)(terrain, state)
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanLocationNode for FixedPlanNode<'a> {
    fn as_base(&self) -> &dyn PlanBaseNode {
        self
    }

    fn desires_location<'s>(&'s self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, _gather_data: &mut PlanGatherChildrenData<'s>) -> bool {
        for placement in self.placements.iter() {
            let plan_location = position + placement.offset;

            if let Some(placement_location) = plan_location.as_build_location() {
                if terrain.get(&placement_location).contains(TerrainFlags::WALL) {
                    return false;
                }

                if let Some(existing) = state.get(&placement_location) {
                    if existing.structure_type != StructureType::Road || placement.structure_type != StructureType::Road {
                        return false;
                    }
                }
            } else {
                return false;
            }
        }

        true
    }

    fn get_children<'s>(&'s self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) {
        if !gather_data.has_visited_location(position, self) {
            gather_data.mark_visited_location(position, self);
                
            if self.child.desires_placement(terrain, state, gather_data) && self.child.desires_location(position, terrain, state, gather_data) {
                self.child.insert_or_expand(position, terrain, state, gather_data);
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanLocationPlacementNode for FixedPlanNode<'a> {
    fn as_location(&self) -> &dyn PlanLocationNode {
        self
    }

    fn id(&self) -> &uuid::Uuid {
        &self.id
    }

    fn get_score(&self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState) -> Option<f32> {
        (self.scorer)(position, terrain, state)
    }

    fn place(&self, position: PlanLocation, _terrain: &FastRoomTerrain, state: &mut PlannerState) -> Result<(), ()> {
        for placement in self.placements.iter() {
            let placement_location = (position + placement.offset).as_location().ok_or(())?;
            
            //TODO: Compute correct RCL + structure count.
            let rcl = 0;

            state.insert(placement_location, RoomItem { structure_type: placement.structure_type, required_rcl: rcl });
        }

        Ok(())
    }
}

pub struct OffsetPlanNode<'a> {
    offsets: &'a [(i8, i8)],
    child: PlanNodeStorage<'a>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanBaseNode for OffsetPlanNode<'a> {
    fn name(&self) -> &str {
        "Offset"
    }

    fn gather_nodes<'b>(&'b self, data: &mut PlanGatherNodesData<'b>) {
        self.child.gather_nodes(data);
    }

    fn desires_placement<'s>(&'s self, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) -> bool {
        self.child.desires_placement(terrain, state, gather_data)
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanLocationNode for OffsetPlanNode<'a> {
    fn as_base(&self) -> &dyn PlanBaseNode {
        self
    }

    fn desires_location<'s>(&'s self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) -> bool {
        self.offsets.iter().any(|offset| {
            let offset_position = position + offset;

            self.child.desires_location(offset_position, terrain, state, gather_data)
        })
    }

    fn get_children<'s>(&'s self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) {
        if !gather_data.has_visited_location(position, self) {
            gather_data.mark_visited_location(position, self);

            if self.child.desires_placement(terrain, state, gather_data) {
                for offset in self.offsets.iter() {
                    let offset_position = position + offset;

                    if self.child.desires_location(offset_position, terrain, state, gather_data) {
                        self.child.insert_or_expand(offset_position, terrain, state, gather_data);
                    }
                }
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanPlacementExpansionNode for OffsetPlanNode<'a> {
    fn as_location(&self) -> &dyn PlanLocationNode {
        self
    }
}

pub struct MultiPlanNode<'a> {
    children: &'a [PlanNodeStorage<'a>]
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanBaseNode for MultiPlanNode<'a> {
    fn name(&self) -> &str {
        "Multi"
    }

    fn gather_nodes<'b>(&'b self, data: &mut PlanGatherNodesData<'b>) {
        for child in self.children.iter() {
            child.gather_nodes(data);
        }
    }

    fn desires_placement<'s>(&'s self, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) -> bool {
        self.children.iter().any(|child| child.desires_placement(terrain, state, gather_data))
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanLocationNode for MultiPlanNode<'a> {
    fn as_base(&self) -> &dyn PlanBaseNode {
        self
    }

    fn desires_location<'s>(&'s self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) -> bool {
        self.children.iter().any(|child| child.desires_location(position, terrain, state, gather_data))
    }

    fn get_children<'s>(&'s self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) {
        if !gather_data.has_visited_location(position, self) {
            gather_data.mark_visited_location(position, self);

            for child in self.children.iter() {
                if child.desires_placement(terrain, state, gather_data) && child.desires_location(position, terrain, state, gather_data) {
                    child.insert_or_expand(position, terrain, state, gather_data);
                }
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanPlacementExpansionNode for MultiPlanNode<'a> {
    fn as_location(&self) -> &dyn PlanLocationNode {
        self
    }
}

pub struct LazyPlanNode<'a> {
    child: fn() -> PlanNodeStorage<'a>
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanBaseNode for LazyPlanNode<'a> {
    fn name(&self) -> &str {
        "Lazy"
    }

    fn gather_nodes<'b>(&'b self, data: &mut PlanGatherNodesData<'b>) {
        let node = (self.child)();

        node.gather_nodes(data);
    }

    fn desires_placement<'s>(&'s self, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) -> bool {
        let node = (self.child)();

        node.desires_placement(terrain, state, gather_data)
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanLocationNode for LazyPlanNode<'a> {
    fn as_base(&self) -> &dyn PlanBaseNode {
        self
    }

    fn desires_location<'s>(&'s self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) -> bool {
        let node = (self.child)();

        node.desires_location(position, terrain, state, gather_data)
    }

    fn get_children<'s>(&'s self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) {
        if !gather_data.has_visited_location(position, self) {
            gather_data.mark_visited_location(position, self);

            let node = (self.child)();

            if node.desires_placement(terrain, state, gather_data) && node.desires_location(position, terrain, state, gather_data) {
                node.insert_or_expand(position, terrain, state, gather_data);
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanPlacementExpansionNode for LazyPlanNode<'a> {
    fn as_location(&self) -> &dyn PlanLocationNode {
        self
    }
}

pub struct FloodFillPlanNodeLevel<'a> {
    offsets: &'a [(i8, i8)],
    node: &'a dyn PlanLocationPlacementNode,
    node_cost: u32
}

pub struct FloodFillPlanNode<'a> {
    id: uuid::Uuid,
    start_offsets: &'a [(i8, i8)],
    expansion_offsets: &'a [(i8, i8)],
    maximum_nodes: u32,
    maximum_expansion: u32,
    levels: &'a [FloodFillPlanNodeLevel<'a>],
    desires_placement: fn(terrain: &FastRoomTerrain, state: &PlannerState) -> bool,
    scorer: fn(position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState) -> Option<f32>

}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanBaseNode for FloodFillPlanNode<'a> {
    fn name(&self) -> &str {
        "Flood Fill"
    }

    fn gather_nodes<'b>(&'b self, data: &mut PlanGatherNodesData<'b>) {
        if data.insert_location_placement(*self.id(), self) {
            for lod in self.levels.iter() {
                lod.node.gather_nodes(data);
            }
        }
    }

    fn desires_placement<'s>(&'s self, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) -> bool {
        (self.desires_placement)(terrain, state) &&
        self.levels.iter().any(|l| l.node.desires_placement(terrain, state, gather_data))
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanLocationNode for FloodFillPlanNode<'a> {
    fn as_base(&self) -> &dyn PlanBaseNode {
        self
    }

    fn desires_location<'s>(&'s self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState, gather_data: &mut PlanGatherChildrenData<'s>) -> bool {
        let mut locations: HashSet<_> = self.start_offsets.into_iter().map(|o| position + o).collect();

        for lod in self.levels.iter() {
            let mut expanded_locations: HashSet<PlanLocation> = locations.iter().flat_map(|&location| lod.offsets.iter().map(move |offset| location + *offset)).collect();

            if expanded_locations.iter().any(|location| lod.node.desires_location(*location, terrain, state, gather_data)) {
                return true;
            }

            locations = std::mem::replace(&mut expanded_locations, HashSet::new());
        }

        false
    }

    fn get_children<'s>(&'s self, _position: PlanLocation, _terrain: &FastRoomTerrain, _state: &PlannerState, _gather_data: &mut PlanGatherChildrenData<'s>) {
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> PlanLocationPlacementNode for FloodFillPlanNode<'a> {
    fn as_location(&self) -> &dyn PlanLocationNode {
        self
    }

    fn id(&self) -> &uuid::Uuid {
        &self.id
    }

    fn get_score(&self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState) -> Option<f32> {
        (self.scorer)(position, terrain, state)
    }

    fn place(&self, position: PlanLocation, terrain: &FastRoomTerrain, state: &mut PlannerState) -> Result<(), ()> {
        let mut locations: HashSet<_> = self.start_offsets.into_iter().map(|o| position + o).collect();
        let mut next_locations: HashSet<_> = HashSet::new();
        let mut visited_locations: HashSet<_> = HashSet::new();
        
        let mut current_expansion = 0;
        let mut current_nodes = 0;

        if let Some(top_lod) = self.levels.first() {
            while current_nodes < self.maximum_nodes && current_expansion < self.maximum_expansion && !locations.is_empty() {
                let mut scored_locations: Vec<_> = locations.into_iter().filter_map(|location| {
                    top_lod.node.get_score(location, terrain, state).map(|s| (location, s))
                }).collect();

                scored_locations.sort_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap());

                for (root_location, _) in scored_locations.iter().rev() {
                    if !visited_locations.contains(root_location) {
                        visited_locations.insert(*root_location);

                        let mut node_locations = vec![*root_location];
                        let mut next_lod_locations = Vec::new();

                        for lod in self.levels.iter() {
                            let mut expanded_locations: Vec<_> = node_locations
                                .iter()
                                .flat_map(|&location| lod.offsets.iter().map(move |offset| location + *offset))
                                .filter_map(|location| lod.node.get_score(location, terrain, state).map(|s| (location, s)))
                                .collect();    
                            
                            expanded_locations.sort_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap());

                            for (node_location, _) in expanded_locations.iter().rev() {
                                let mut current_gather_data = PlanGatherChildrenData::<'a>::new();

                                if lod.node.desires_placement(terrain, state, &mut current_gather_data) && lod.node.desires_location(*node_location, terrain, state, &mut current_gather_data) && current_gather_data.insert_location_placement(*node_location, lod.node) {
                                    lod.node.place(*node_location, terrain, state)?;

                                    current_nodes += lod.node_cost;

                                    if current_nodes >= self.maximum_nodes {
                                        break;
                                    }

                                    for offset in self.expansion_offsets.into_iter() {
                                        let next_location = *root_location + *offset;
                                        
                                        next_locations.insert(next_location);
                                    }
                                } else {
                                    next_lod_locations.push(*node_location);
                                }
                            }

                            node_locations = std::mem::replace(&mut next_lod_locations, Vec::new());
                        }
                    }
                }

                locations = std::mem::replace(&mut next_locations, HashSet::new());

                current_expansion += 1;
            }
        }

        Ok(())
    }
}

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

const fn placement(structure_type: StructureType, x: i8, y: i8) -> PlanPlacement {
    PlanPlacement {
        structure_type,
        offset: PlanLocation {
            x,
            y
        }
    }
}

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

pub struct FastRoomTerrain {
    buffer: Vec<u8>
}

bitflags! {
    pub struct TerrainFlags: u8 {
        const NONE = 0;
        const WALL = TERRAIN_MASK_WALL;
        const SWAMP = TERRAIN_MASK_SWAMP;
        const LAVA = TERRAIN_MASK_LAVA;
    }
}

impl FastRoomTerrain {
    pub fn new(buffer: Vec<u8>) -> FastRoomTerrain {
        FastRoomTerrain {
            buffer
        }
    }

    pub fn get(&self, pos: &Location) -> TerrainFlags {
        self.get_xy(pos.x(), pos.y())
    }

    pub fn get_xy(&self, x: u8, y: u8) -> TerrainFlags {
        let index = (y as usize * ROOM_WIDTH as usize) + (x as usize);

        TerrainFlags::from_bits_truncate(self.buffer[index])
    }
}

struct EvaluationStackEntry<'b> {
    children: Vec<PlanNodeChild<'b>>,
    visited: Vec<PlanNodeChild<'b>>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'b> EvaluationStackEntry<'b> {
    pub fn to_serialized(&self, index_lookup: &HashMap<uuid::Uuid, usize>) -> SerializedEvaluationStackEntry {
        SerializedEvaluationStackEntry {
            children: self.children.iter().map(|c| c.to_serialized(index_lookup)).collect(),
            visited: self.visited.iter().map(|c| c.to_serialized(index_lookup)).collect(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct SerializedEvaluationStackEntry {
    #[serde(rename = "c")]
    children: Vec<SerializedPlanNodeChild>,
    #[serde(rename = "v")]
    visited: Vec<SerializedPlanNodeChild>,
}

impl SerializedEvaluationStackEntry {
    pub fn as_entry<'b>(&self, nodes: &PlanGatherNodesData<'b>, index_lookup: &Vec<uuid::Uuid>) -> Result<EvaluationStackEntry<'b>, ()> {
        let mut children = Vec::new();

        for serialized_child in &self.children {
            let child = serialized_child.as_entry(nodes, index_lookup)?;

            children.push(child);
        }

        let mut visited = Vec::new();

        for serialized_child in &self.visited {
            let child = serialized_child.as_entry(nodes, index_lookup)?;

            visited.push(child);
        }

        Ok(EvaluationStackEntry {
            children,
            visited
        })
    }
}

struct TreePlanner<'t, H> where H: FnMut(&PlannerState) {
    terrain: &'t FastRoomTerrain,
    handler: H
}

#[derive(Clone, Serialize, Deserialize)]
struct SerializedEvaluationStack {
    identifiers: Vec<uuid::Uuid>,
    entries: Vec<SerializedEvaluationStackEntry>
}

impl SerializedEvaluationStack {
    pub fn from_stack(gathered_nodes: &PlanGatherNodesData, entries: &Vec<EvaluationStackEntry>) -> SerializedEvaluationStack {
        let identifiers: Vec<_> = gathered_nodes.get_all_ids();
        let index_lookup: HashMap<_, _> = identifiers.iter().enumerate().map(|(index, id)| (*id, index)).collect();

        let serialized_entries = entries
            .iter()
            .map(|e| e.to_serialized(&index_lookup))
            .collect();

        SerializedEvaluationStack {
            identifiers,
            entries: serialized_entries
        }
    }

    pub fn to_stack<'b>(&self, gathered_nodes: &PlanGatherNodesData<'b>) -> Result<Vec<EvaluationStackEntry<'b>>, ()> {
        let mut stack = Vec::new();

        for serialized_entry in self.entries.iter() {
            let entry = serialized_entry.as_entry(&gathered_nodes, &self.identifiers)?;

            stack.push(entry);
        }

        Ok(stack)
    }
}

enum TreePlannerResult {
    Complete,
    Running(SerializedEvaluationStack)
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'t, H> TreePlanner<'t, H> where H: FnMut(&PlannerState) {
    pub fn seed<'r, 's>(&mut self, root_nodes: &[&'r dyn PlanGlobalExpansionNode], state: &'s mut PlannerState) -> Result<TreePlannerResult, ()> {
        let mut stack = Vec::new();

        let mut gathered_children = PlanGatherChildrenData::<'s>::new();

        for node in root_nodes.iter() {
            if gathered_children.desires_placement(node.as_base(), self.terrain, state) {
                node.get_children(self.terrain, state, &mut gathered_children);
            }
        }

        let children = gathered_children.collect();

        let mut ordered_children: Vec<_> = children
            .into_iter()
            .filter_map(|node| node.get_score(self.terrain, state).map(|score| (node, score)))
            .collect();

        ordered_children.sort_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap());

        stack.push(EvaluationStackEntry {
            children: ordered_children.into_iter().map(|(node, _)| node).collect(),
            visited: Vec::new()
        });

        let mut gathered_nodes = PlanGatherNodesData::new::<'r>();
        
        for node in root_nodes {
            node.gather_nodes(&mut gathered_nodes);
        }

        let serialized = SerializedEvaluationStack::from_stack(&gathered_nodes, &stack);

        Ok(TreePlannerResult::Running(serialized))
    }

    pub fn process<'r, 's, F>(&mut self, root_nodes: &[&'r dyn PlanGlobalExpansionNode], state: &'s mut PlannerState, serialized_stack: &SerializedEvaluationStack, should_continue: F) -> Result<TreePlannerResult, ()> where F: Fn() -> bool {
        let mut processed_entries = 0;
        
        let mut gathered_nodes = PlanGatherNodesData::new::<'r>();
        
        for node in root_nodes {
            node.gather_nodes(&mut gathered_nodes);
        }

        let mut stack = serialized_stack.to_stack(&gathered_nodes)?;

        while !stack.is_empty() && should_continue() {
            let mut placed_node = None;

            let finished_entry = {
                let entry = stack.last_mut().unwrap();

                while let Some(child) = entry.children.pop() {
                    processed_entries += 1;

                    state.push_layer();

                    match child.place(self.terrain, state) {
                        Ok(_) => {
                            (self.handler)(state);

                            placed_node = Some(child);

                            break;
                        },
                        Err(_) => {
                            state.pop_layer();

                            entry.visited.push(child);
                        }
                    }
                    
                    if !should_continue() {
                        break;
                    }
                }

                entry.children.is_empty()
            };

            if let Some(child) = placed_node {
                let mut gathered_children = PlanGatherChildrenData::<'s>::new();

                for entry in stack.iter() {
                    for visited in entry.visited.iter() {
                        visited.mark_visited(&mut gathered_children);
                    }
                }

                child.get_children(self.terrain, state, &mut gathered_children);

                for existing_child in stack.last().unwrap().children.iter() {
                    if existing_child.desires_placement(self.terrain, state, &mut gathered_children) && existing_child.desires_location(self.terrain, state, &mut gathered_children) {
                        existing_child.insert(&mut gathered_children);
                    }
                }

                let children = gathered_children.collect();

                let mut ordered_children: Vec<_> = children
                    .into_iter()
                    .filter_map(|node| node.get_score(self.terrain, state).map(|score| (node, score)))
                    .collect();

                ordered_children.sort_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap());

                stack.push(EvaluationStackEntry {
                    children: ordered_children.into_iter().map(|(node, _)| node).collect(),
                    visited: Vec::new()
                });
            } else if finished_entry {
                state.pop_layer();

                stack.pop();
            }
        }

        info!("Processed planning entries: {} - Known children: {}", processed_entries, stack.iter().map(|e| e.children.len()).sum::<usize>());

        if stack.is_empty() {
            Ok(TreePlannerResult::Complete)
        } else {
            let serialized = SerializedEvaluationStack::from_stack(&gathered_nodes, &stack);

            Ok(TreePlannerResult::Running(serialized))
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BestPlanData {
    score: f32,
    state: PlanState
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PlanRunningStateData {
    planner_state: PlannerState,
    stack: SerializedEvaluationStack,
    best_plan: Option<BestPlanData>
}

impl PlanRunningStateData {
    pub fn visualize(&self, visualizer: &mut RoomVisualizer) {
        self.planner_state.visualize(visualizer);
    }

    pub fn visualize_best(&self, visualizer: &mut RoomVisualizer) {
        if let Some(best_plan) = &self.best_plan {
            visualize_room_items(best_plan.state.iter(), visualizer);
        }
    }
}

pub enum PlanSeedResult {
    Complete(Option<Plan>),
    Running(PlanRunningStateData)
}

pub enum PlanEvaluationResult {
    Complete(Option<Plan>),
    Running()
}

pub struct Planner<'a> {
    room: &'a Room,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> Planner<'a> {
    pub fn new(room: &Room) -> Planner {
        Planner { room }
    }

    pub fn seed(&self, root_nodes: &[&dyn PlanGlobalExpansionNode]) -> Result<PlanSeedResult, ()> {
        let terrain = self.room.get_terrain();
        let fast_terrain = FastRoomTerrain::new(terrain.get_raw_buffer());

        let mut planner_state = PlannerState::new();

        let mut best_plan = None;

        let state_handler = |new_state: &PlannerState| {
            if let Some(score) = Self::score_state(new_state) {
                best_plan = Some(BestPlanData {
                    score,
                    state: new_state.snapshot()
                });
            }
        };

        let mut planner = TreePlanner {
            terrain: &fast_terrain,
            handler: state_handler
        };

        let seed_result = match planner.seed(root_nodes, &mut planner_state)? {
            TreePlannerResult::Complete => {
                let plan = best_plan.take().map(|p| Plan { state: p.state });

                PlanSeedResult::Complete(plan)
            },
            TreePlannerResult::Running(stack) => {
                let running_data = PlanRunningStateData {
                    planner_state,
                    stack,
                    best_plan,
                };

                PlanSeedResult::Running(running_data)
            }
        };

        Ok(seed_result)
    }

    pub fn evaluate(&self, root_nodes: &[&dyn PlanGlobalExpansionNode], evaluation_state: &mut PlanRunningStateData, allowed_cpu: f64) -> Result<PlanEvaluationResult, ()> {
        let terrain = self.room.get_terrain();
        let fast_terrain = FastRoomTerrain::new(terrain.get_raw_buffer());

        let mut current_best = evaluation_state.best_plan.as_ref().map(|p| p.score);
        let mut new_best_plan = None;

        let state_handler = |new_state: &PlannerState| {
            if let Some(score) = Self::score_state(new_state) {
                if current_best.map(|s| score > s).unwrap_or(true) {
                    new_best_plan = Some(BestPlanData {
                        score,
                        state: new_state.snapshot()
                    });

                    current_best = Some(score);
                }
            }
        };

        let mut planner = TreePlanner {
            terrain: &fast_terrain,
            handler: state_handler
        };

        let start_cpu = game::cpu::get_used();

        let should_continue = || game::cpu::get_used() - start_cpu < allowed_cpu;

        let evaluate_result = match planner.process(root_nodes, &mut evaluation_state.planner_state, &evaluation_state.stack, should_continue)? {
            TreePlannerResult::Complete => {
                if new_best_plan.is_some() {
                    evaluation_state.best_plan = new_best_plan;
                }

                let plan = evaluation_state.best_plan.take().map(|p| Plan { state: p.state });

                PlanEvaluationResult::Complete(plan)
            },
            TreePlannerResult::Running(stack) => {
                if new_best_plan.is_some() {
                    evaluation_state.best_plan = new_best_plan;
                }

                evaluation_state.stack = stack;

                PlanEvaluationResult::Running()
            }
        };

        Ok(evaluate_result)
    }

    
    fn score_state(eval_state: &PlannerState) -> Option<f32> {
        let is_complete = 
            eval_state.get_count(StructureType::Spawn) >= 1 &&
            eval_state.get_count(StructureType::Extension) >= 60 &&
            eval_state.get_count(StructureType::Storage) >= 1 &&
            eval_state.get_count(StructureType::Terminal) >= 1 &&
            eval_state.get_count(StructureType::Link) >= 1;

        if !is_complete {
            return None;
        }

        let storage_locations = eval_state.get_locations(StructureType::Storage);
        let extension_locations = eval_state.get_locations(StructureType::Extension);

        let total_extension_distance: f32 = extension_locations
            .iter()
            .map(|extension| storage_locations
                .iter()
                .map(|storage| storage.distance_to(*extension))
                .min()
                .unwrap() as f32
            )
            .sum();

        let average_distance = total_extension_distance / (extension_locations.len() as f32);
        let score = 1.0 - (average_distance / ROOM_WIDTH.max(ROOM_HEIGHT) as f32);

        Some(score)

    }

    /*
    fn get_nearest_empty_terrain(terrain: &FastRoomTerrain, start_pos: (u32, u32)) -> Option<(u32, u32)> {
        let expanded = &[(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
        let center = &[(0, 0)];
        let search_pattern = center.iter().chain(expanded.iter());

        for pos in search_pattern {
            let room_pos = ((start_pos.0 as i32 + pos.0), (start_pos.1 as i32 + pos.1));

            if room_pos.in_room_build_bounds() {
                let terrain_data = terrain.get(&Location::new(room_pos.0 as u32, room_pos.1 as u32));

                if terrain_data.contains(TerrainFlags::Wall) {
                    return Some((room_pos.0 as u32, room_pos.1 as u32));
                }
            }
        }

        None
    }
    */

    fn spawn_count_to_rcl(count: u32) -> Option<u32> {
        match count {
            0 => Some(0),
            1 => Some(1),
            2 => Some(7),
            3 => Some(8),
            _ => None
        }
    }

    //TODO: Need much better logic for spawn placement.
    //fn add_spawns(room: &Room, terrain: &FastRoomTerrain, state: &mut PlannerState) -> Result<Vec<Location>, String> {
        //let mut current_spawns = 0;

        /*
        let spawns = room.find(find::MY_SPAWNS);

        for spawns in spawns.iter() {
            let pos = spawns.pos();

            if let Some(rcl) = Self::spawn_count_to_rcl(current_spawns) {
                state.insert(
                    Location::from_coords(pos.x(), pos.y()),
                    RoomItem { structure_type: StructureType::Spawn, required_rcl: rcl }
                );
            }

            current_spawns += 1;
        }
        */

        /*
        let spawn_locations = Vec::new();
        
        let mut visualize = RoomVisualizer::new();
        
        let spawn_candidates = Self::get_wall_min_distance_locations(terrain, 5);

        for pos in &spawn_candidates {
            visualize.rect(pos.x() as f32 - 0.5, pos.y() as f32 - 0.5, 1.0, 1.0, Some(RectStyle::default().fill("green")));
        }
        
        let root_location = PlanLocation::new(0, 0);

        let root_node = DynamicPlanNode {
            locations: spawn_candidates.iter().map(|l| l.into()).collect(),
            child: ROOT_SPAWN_NODE,
        };

        let is_complete = |eval_state: &PlannerState| {
            eval_state.get_count(StructureType::Extension) >= 40
        };

        let planner = TreePlanner {
            terrain,
            completion: is_complete
        };

        info!("Planning!!!");

        match planner.evaluate(root_location, &root_node, state) {
            Ok(_) => {
                info!("Success!");
            },
            Err(_) => {
                info!("Failed!");
            }
        };

        visualize.apply(Some(room.name()));
        */

        //TODO: Extract spawns out...


        /*
        if spawns.is_empty() {
            let sources = room.find(find::SOURCES);

            if sources.len() == 2 {
                if let Some(empty_start_pos) = Self::get_nearest_empty_terrain(&terrain, sources[0].pos().into()) {
                    let find_options = FindOptions::new()
                        .max_rooms(1)
                        .ignore_creeps(true)
                        .ignore_destructible_structures(true);

                    let start_pos = RoomPosition::new(empty_start_pos.0, empty_start_pos.1, room.name());
                    let end_pos = sources[1].pos();

                    if let Path::Vectorized(path) = start_pos.find_path_to(&end_pos, find_options) {
                        if !path.is_empty() {
                            let mid_point = &path[path.len() / 2];

                            state.insert(
                                Location::from_coords(mid_point.x, mid_point.y),
                                RoomItem { structure_type: StructureType::Spawn, required_rcl: 0 }
                            );
                        }
                    }
                }
            }
        }
        */

        //Ok(spawn_locations)
    //}

    fn extension_count_to_rcl(count: u32) -> Option<u32> {
        match count {
            0 => Some(0),
            1..=5 => Some(2),
            6..=10 => Some(3),
            11..=20 => Some(4),
            21..=30 => Some(5),
            31..=40 => Some(6),
            41..=50 => Some(7),
            51..=60 => Some(8),
            _ => None,
        }
    }

    /*
    fn add_extensions(_room: &Room, terrain: &FastRoomTerrain, state: &mut PlannerState) -> Result<(), String> {
        let spawn_positions: Vec<Location> = state
            .iter()
            .filter_map(|(pos, entry)| match entry {
                RoomItem { structure_type: StructureType::Spawn, .. } => Some(pos),
                _ => None,
            })
            .cloned()
            .collect();

        let mut current_extensions = 0;
        let corner_points = [(-1, -1), (-1, 1), (1, 1), (1, -1)];
        let mut rcl = Self::extension_count_to_rcl(current_extensions + 1);

        for spawn_pos in spawn_positions {
            let mut expansion = 1;
            while rcl.is_some() {
                let expanded_corner_points: Vec<(i32, i32)> = corner_points.iter().map(|(x, y)| (x * expansion, y * expansion)).collect();
                for i in 0..expanded_corner_points.len() {
                    let mut current_pos = expanded_corner_points[i % expanded_corner_points.len()];
                    let end_pos = expanded_corner_points[(i + 1) % expanded_corner_points.len()];

                    let step_start = corner_points[i % corner_points.len()];
                    let step_end = corner_points[(i + 1) % corner_points.len()];

                    let delta_x = step_end.0 - step_start.0;
                    let delta_y = step_end.1 - step_start.1;

                    while current_pos != end_pos && rcl.is_some() {
                        let room_pos = ((spawn_pos.x() as i32 + current_pos.0), (spawn_pos.y() as i32 + current_pos.1));

                        let location = Location::from_coords(room_pos.0 as u32, room_pos.1 as u32);

                        if room_pos.in_room_build_bounds() && state.get(&location).is_none() {
                            match terrain.get(room_pos.0 as u32, room_pos.1 as u32) {
                                Terrain::Plain | Terrain::Swamp => {
                                    state.insert(
                                        Location::from_coords(room_pos.0 as u32, room_pos.1 as u32),
                                        RoomItem { structure_type: StructureType::Extension, required_rcl: rcl.unwrap() },
                                    );

                                    current_extensions += 1;
                                    rcl = Self::extension_count_to_rcl(current_extensions + 1);
                                }
                                _ => {}
                            }
                        }

                        current_pos.0 += delta_x;
                        current_pos.1 += delta_y;
                    }

                    if rcl.is_none() {
                        break;
                    }
                }

                expansion += 1;
            }
        }

        Ok(())
    }
    */

    fn add_containers(room: &Room, _terrain: &FastRoomTerrain, spawns: &Vec<Location>, state: &mut PlannerState) -> Result<(), String> {
        let spawn_positions: Vec<_> = spawns.iter().map(|l| RoomPosition::new(l.x() as u32, l.y() as u32, room.name())).collect();

        for source in room.find(find::SOURCES) {
            let nearest_spawn_path = spawn_positions
                .iter()
                .cloned()
                .find_nearest_path_to(source.pos(), PathFinderHelpers::same_room_ignore_creeps_and_structures_range_1);

            if let Some(Path::Vectorized(path)) = nearest_spawn_path {
                if let Some(last_step) = path.last() {
                    let pos_x = last_step.x as i32;
                    let pos_y = last_step.y as i32;

                    state.insert(
                        Location::from_coords(pos_x as u32, pos_y as u32),
                        RoomItem { structure_type: StructureType::Container, required_rcl: 2 }
                    );
                }
            }
        }

        if let Some(controller) = room.controller() {
            let nearest_spawn_path = spawn_positions
                .iter()
                .cloned()
                .find_nearest_path_to(controller.pos(), PathFinderHelpers::same_room_ignore_creeps_and_structures_range_1);

            if let Some(Path::Vectorized(path)) = nearest_spawn_path {
                if let Some(last_step) = path.last() {
                    let pos_x = last_step.x as i32;
                    let pos_y = last_step.y as i32;

                    state.insert(
                        Location::from_coords(pos_x as u32, pos_y as u32),
                        RoomItem { structure_type: StructureType::Container, required_rcl: 2 }
                    );
                }
            }
        }

        Ok(())
    }

    fn add_extractors(room: &Room, _terrain: &FastRoomTerrain, spawns: &Vec<Location>, state: &mut PlannerState) -> Result<(), String> {
        let spawn_positions: Vec<_> = spawns.iter().map(|l| RoomPosition::new(l.x() as u32, l.y() as u32, room.name())).collect();

        for mineral in room.find(find::MINERALS) {
            state.insert(
                Location::from_pos(mineral.pos()),
                RoomItem { structure_type: StructureType::Extractor, required_rcl: 6 }
            );

            let nearest_spawn_path = spawn_positions
                .iter()
                .cloned()
                .find_nearest_path_to(mineral.pos(), PathFinderHelpers::same_room_ignore_creeps_and_structures_range_1);

            if let Some(Path::Vectorized(path)) = nearest_spawn_path {
                if let Some(last_step) = path.last() {
                    let pos_x = last_step.x as i32;
                    let pos_y = last_step.y as i32;

                    state.insert(
                        Location::from_coords(pos_x as u32, pos_y as u32),
                        RoomItem { structure_type: StructureType::Container, required_rcl: 6 }
                    );
                }
            }
        }

        Ok(())
    }
}
