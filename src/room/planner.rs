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

    fn commit_layer(&mut self) {
        let layer = self.layers.pop().unwrap();
        let base = self.layers.last_mut().unwrap();

        for (pos, entry) in layer.data.into_iter() {
            base.insert(pos, entry);
        }
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

    pub fn insert(&mut self, location: Location, item: RoomItem) {
        let layer = self.layers.last_mut().unwrap();

        layer.insert(location, item);
    }

    pub fn complete(&self) -> PlanState {
        self.layers.last().unwrap().clone().complete()
    }

    pub fn visualize(&self, visualizer: &mut RoomVisualizer) {
        for layer in &self.layers {
            layer.visualize(visualizer);
        }
    }
}

#[derive(Clone, Copy)]
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

pub struct Planner<'a> {
    room: &'a Room,
}

#[derive(Clone)]
pub struct PlanNodeChild<'a> {
    location: PlanLocation,
    node: &'a dyn PlanNode
}

impl<'a> PlanNodeChild<'a> {
    fn to_serialized(&self) -> SerializedPlanNodeChild {
        SerializedPlanNodeChild {
            location: self.location,
            node: self.node.id()
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct SerializedPlanNodeChild {
    #[serde(rename = "l")]
    location: PlanLocation,
    #[serde(rename = "n")]
    node: uuid::Uuid
}

impl SerializedPlanNodeChild {
    pub fn as_entry<'b>(&self, nodes: &HashMap<uuid::Uuid, &'b dyn PlanNode>) -> Result<PlanNodeChild<'b>, ()> {
        let node = nodes.get(&self.node).ok_or(())?;

        Ok(PlanNodeChild{
            location: self.location,
            node: *node
        })
    }
}

trait PlanNode {
    fn id(&self) -> uuid::Uuid;

    fn gather_nodes<'b>(&'b self, nodes: &mut HashMap<uuid::Uuid, &'b dyn PlanNode>);

    fn name(&self) -> &str;

    fn immediate(&self) -> bool;
    
    fn desires_placement(&self, terrain: &FastRoomTerrain, state: &PlannerState) -> bool;

    fn get_score(&self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState) -> f32;

    fn place(&self, position: PlanLocation, terrain: &FastRoomTerrain, state: &mut PlannerState) -> Result<(), ()>;

    fn get_children<'s>(&'s self, position: PlanLocation) -> Vec<PlanNodeChild<'s>>;
}

struct PlanPlacement {
    structure_type: StructureType,
    offset: PlanLocation
}

struct DynamicPlanNode<'a> {
    id: uuid::Uuid,
    locations: Vec<PlanLocation>,
    child: &'a dyn PlanNode
}

impl<'a> PlanNode for DynamicPlanNode<'a> {
    fn id(&self) -> uuid::Uuid {
        self.id
    }

    fn gather_nodes<'b>(&'b self, nodes: &mut HashMap<uuid::Uuid, &'b dyn PlanNode>) {
        match nodes.entry(self.id()) {
            Entry::Occupied(_) => return,
            Entry::Vacant(e) => {
                e.insert(self);
            }
        }

        self.child.gather_nodes(nodes);
    }

    fn name(&self) -> &str {
        "Dynamic"
    }

    fn immediate(&self) -> bool {
        true
    }

    fn desires_placement(&self, _terrain: &FastRoomTerrain, _state: &PlannerState) -> bool {
        true
    }

    fn get_score(&self, _position: PlanLocation, _terrain: &FastRoomTerrain, _state: &PlannerState) -> f32 {
        0.0
    }

    fn place(&self, _position: PlanLocation, _terrain: &FastRoomTerrain, _state: &mut PlannerState) -> Result<(), ()> {
        Ok(())
    }

    fn get_children<'n>(&'n self, _position: PlanLocation) -> Vec<PlanNodeChild<'n>> {
        self.locations.iter().map(|l| PlanNodeChild { location: *l, node: self.child }).collect()
    }
}

struct FixedPlanNode<'a> {
    id: uuid::Uuid,
    placements: &'a [PlanPlacement],
    children: &'a [&'a dyn PlanNode],
    validator: fn(terrain: &FastRoomTerrain, state: &PlannerState) -> bool,
    scorer: fn(position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState) -> f32
}

impl<'a> PlanNode for FixedPlanNode<'a> {
    fn id(&self) -> uuid::Uuid {
        self.id
    }

    fn gather_nodes<'b>(&'b self, nodes: &mut HashMap<uuid::Uuid, &'b dyn PlanNode>) {
        match nodes.entry(self.id()) {
            Entry::Occupied(_) => return,
            Entry::Vacant(e) => {
                e.insert(self);
            }
        }

        for child in self.children.iter() {
            child.gather_nodes(nodes);
        }
    }

    fn name(&self) -> &str {
        "Fixed"
    }

    fn immediate(&self) -> bool {
        false
    }

    fn desires_placement(&self, terrain: &FastRoomTerrain, state: &PlannerState) -> bool {
        (self.validator)(terrain, state)
    }

    fn get_score(&self, position: PlanLocation, terrain: &FastRoomTerrain, state: &PlannerState) -> f32 {
        (self.scorer)(position, terrain, state)
    }

    fn place(&self, position: PlanLocation, terrain: &FastRoomTerrain, state: &mut PlannerState) -> Result<(), ()> {
        for placement in self.placements.iter() {
            let placement_location = (position + placement.offset).as_location().ok_or(())?;

            if terrain.get(&placement_location).contains(TerrainFlags::WALL) {
                return Err(());
            }

            if let Some(existing) = state.get(&placement_location) {
                //TODO: This allows overwriting roads - make more generic.
                if existing.structure_type != placement.structure_type || existing.structure_type != StructureType::Road || placement.structure_type != StructureType::Road {
                    return Err(());
                }
            }
            
            //TODO: Compute correct RCL + structure count.
            let rcl = 0;

            state.insert(placement_location, RoomItem { structure_type: placement.structure_type, required_rcl: rcl });
        }

        Ok(())
    }

    fn get_children<'n>(&'n self, position: PlanLocation) -> Vec<PlanNodeChild<'n>> {
        self.children.iter().map(|c| PlanNodeChild { location: position, node: *c }).collect()
    }
}

pub struct OffsetPlanNode<'a> {
    id: uuid::Uuid,
    offsets: &'a [(i8, i8)],
    child: &'a dyn PlanNode
}

impl<'a> PlanNode for OffsetPlanNode<'a> {
    fn id(&self) -> uuid::Uuid {
        self.id
    }

    fn gather_nodes<'b>(&'b self, nodes: &mut HashMap<uuid::Uuid, &'b dyn PlanNode>) {
        match nodes.entry(self.id()) {
            Entry::Occupied(_) => return,
            Entry::Vacant(e) => {
                e.insert(self);
            }
        }

        self.child.gather_nodes(nodes);
    }

    fn name(&self) -> &str {
        "Offset"
    }

    fn immediate(&self) -> bool {
        true
    }

    fn desires_placement(&self, terrain: &FastRoomTerrain, state: &PlannerState) -> bool {
        self.child.desires_placement(terrain, state)
    }
    
    fn get_score(&self, _position: PlanLocation, _terrain: &FastRoomTerrain, _state: &PlannerState) -> f32 {
        0.0
    }

    fn place(&self, _position: PlanLocation, _terrain: &FastRoomTerrain, _state: &mut PlannerState) -> Result<(), ()> {
        Ok(())
    }

    fn get_children<'n>(&'n self, position: PlanLocation) -> Vec<PlanNodeChild<'n>> {
        self.offsets.iter().map(|offset| PlanNodeChild { location: position + offset, node: self.child }).collect()
    }
}

pub struct MultiPlanNode<'a> {
    id: uuid::Uuid,
    children: &'a [&'a dyn PlanNode]
}

impl<'a> PlanNode for MultiPlanNode<'a> {
    fn id(&self) -> uuid::Uuid {
        self.id
    }

    fn gather_nodes<'b>(&'b self, nodes: &mut HashMap<uuid::Uuid, &'b dyn PlanNode>) {
        match nodes.entry(self.id()) {
            Entry::Occupied(_) => return,
            Entry::Vacant(e) => {
                e.insert(self);
            }
        }

        for child in self.children.iter() {
            child.gather_nodes(nodes);
        }
    }

    fn name(&self) -> &str {
        "Multi"
    }

    fn immediate(&self) -> bool {
        true
    }

    fn desires_placement(&self, terrain: &FastRoomTerrain, state: &PlannerState) -> bool {
        self.children.iter().any(|child| child.desires_placement(terrain, state))
    }

    fn get_score(&self, _position: PlanLocation, _terrain: &FastRoomTerrain, _state: &PlannerState) -> f32 {
        0.0
    }

    fn place(&self, _position: PlanLocation, _terrain: &FastRoomTerrain, _state: &mut PlannerState) -> Result<(), ()> {
        Ok(())
    }

    fn get_children<'n>(&'n self, position: PlanLocation) -> Vec<PlanNodeChild<'n>> {
        self.children.iter().map(|child| PlanNodeChild { location: position, node: *child }).collect()
    }
}

pub struct LazyPlanNode<'a> {
    id: uuid::Uuid,
    child: fn() -> &'a dyn PlanNode
}

impl<'a> PlanNode for LazyPlanNode<'a> {
    fn id(&self) -> uuid::Uuid {
        self.id
    }

    fn gather_nodes<'b>(&'b self, nodes: &mut HashMap<uuid::Uuid, &'b dyn PlanNode>) {
        match nodes.entry(self.id()) {
            Entry::Occupied(_) => return,
            Entry::Vacant(e) => {
                e.insert(self);
            }
        }

        let node = (self.child)();

        node.gather_nodes(nodes);
    }

    fn name(&self) -> &str {
        "Lazy"
    }

    fn immediate(&self) -> bool {
        true
    }

    fn desires_placement(&self, terrain: &FastRoomTerrain, state: &PlannerState) -> bool {
        let node = (self.child)();

        node.desires_placement(terrain, state)
    }

    fn get_score(&self, _position: PlanLocation, _terrain: &FastRoomTerrain, _state: &PlannerState) -> f32 {
        0.0
    }

    fn place(&self, _position: PlanLocation, _terrain: &FastRoomTerrain, _state: &mut PlannerState) -> Result<(), ()> {
        Ok(())
    }

    fn get_children<'n>(&'n self, position: PlanLocation) -> Vec<PlanNodeChild<'n>> {
        let node = (self.child)();

        vec![PlanNodeChild { location: position, node }]
    }
}

const fn multi_node<'a>(id: u128, children: &'a [&'a dyn PlanNode]) -> MultiPlanNode<'a> {
    MultiPlanNode {
        id: uuid::Uuid::from_u128(id),
        children
    }
}

const fn lazy_node<'a>(id: u128, child: fn() -> &'a dyn PlanNode) -> LazyPlanNode<'a> {
    LazyPlanNode {
        id: uuid::Uuid::from_u128(id),
        child
    }
}

const fn offset_node<'a>(id: u128, offsets: &'a [(i8, i8)], child: &'a dyn PlanNode) -> OffsetPlanNode<'a> {
    OffsetPlanNode {
        id: uuid::Uuid::from_u128(id),
        offsets,
        child
    }
}

//
// Patterns
//

const ONE_OFFSET_SQUARE: &[(i8, i8)] = &[(-1, -1), (-1, 0), (-1, 1), (0, 1), (1, 1), (1, 0), (1, -1), (0, -1)];
const TWO_OFFSET_SQUARE: &[(i8, i8)] = &[(-2, -2), (-2, -1), (-2, 0), (-2, 1), (-2, 2), (-1, 2), (0, 2), (1, 2), (2, 2), (2, 1), (2, 0), (2, -1), (2, -2), (1, -2), (0, -2), (-1, -2)];

//TODO: Make this centered.
const ONE_OFFSET_DIAMOND: &[(i8, i8)] = &[(0, 0), (-1, -1), (1, -1), (0, -2)];
const TWO_OFFSET_DIAMOND: &[(i8, i8)] = &[(0, -2), (-1, -1), (-2, 0), (-1, 1), (0, 2), (1, 1), (2, 0), (1, -1)];

//
// Nodes
//

const ALL_NODES: &MultiPlanNode = &MultiPlanNode {
    id: uuid::Uuid::from_u128(0x48b4_9ec0_2ba6_45a7_8482_e9c2_3b9c_d867u128),
    children: &[EXTENSION_CROSS]
};

const ALL_NODES_LAZY: &LazyPlanNode = &LazyPlanNode {
    id: uuid::Uuid::from_u128(0x6cf5_b443_7953_4f55_80f6_7325_ecaf_0398u128),
    child: || ALL_NODES
};

const ALL_NODES_ONE_OFFSET_SQUARE: &OffsetPlanNode = &offset_node(0x5de9_75f0_76cf_4346_8aae_09ef_3919_d70fu128, 
    ONE_OFFSET_SQUARE,
    ALL_NODES_LAZY
);

const ALL_NODES_TWO_OFFSET_SQUARE: &OffsetPlanNode = &offset_node(0x130c_7903_0744_4360_ba11_fd55_7d1f_32cc_u128, 
    TWO_OFFSET_SQUARE,
    ALL_NODES_LAZY
);

const ALL_NODES_ONE_OFFSET_DIAMOND: &OffsetPlanNode = &offset_node(0xcc0e_e7ea_cb57_4021_bd22_d5d7_22aa_6e25u128, 
    ONE_OFFSET_DIAMOND,
    ALL_NODES_LAZY
);

const ALL_NODES_TWO_OFFSET_DIAMOND: &OffsetPlanNode = &offset_node(0xa2e9_e6e5_d307_4c21_8a6d_91ba_fc0c_6382u128, 
    TWO_OFFSET_DIAMOND,
    ALL_NODES_LAZY
);

const fn placement(structure_type: StructureType, x: i8, y: i8) -> PlanPlacement {
    PlanPlacement {
        structure_type,
        offset: PlanLocation {
            x,
            y
        }
    }
}

const EXTENSION_CROSS: &OffsetPlanNode = &OffsetPlanNode {
    id: uuid::Uuid::from_u128(0x4348_d001_e496_45a5_adf8_61e6_ebff_4fbfu128), 
    offsets: TWO_OFFSET_DIAMOND, 
    child: &FixedPlanNode {
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
        children: &[ALL_NODES_TWO_OFFSET_DIAMOND],
        validator: |_, state| state.get_count(StructureType::Extension) <= 55,
        scorer: |_, _, _| 0.5,
    }
};

const STORAGE: &OffsetPlanNode = &OffsetPlanNode {
    id: uuid::Uuid::from_u128(0xe76d_2c8b_9ba2_44d0_bd16_d83d_1e50_2cf2u128),
    offsets: ONE_OFFSET_DIAMOND,
    child: &FixedPlanNode {
        id: uuid::Uuid::from_u128(0x7f7e_e145_d350_4aa1_9493_0c7c_ecb3_26cdu128),
        placements: &[
            placement(StructureType::Storage, 0, 0),

            placement(StructureType::Road, -1, 0),
            placement(StructureType::Road, 0, 1),
            placement(StructureType::Road, 1, 0),
            placement(StructureType::Road, 0, 0),
        ],
        children: &[&offset_node(0x3e48_7a1f_4f30_4ed9_a2fb_69df_6df1_ad3du128, ONE_OFFSET_DIAMOND,
            &multi_node(0x5828_b144_de3e_4564_9656_2ff2_a187_b376u128, &[TERMINAL, LINK])
        )],
        validator: |_, state| state.get_count(StructureType::Storage) == 0,
        scorer: |_, _, _| 0.75,
    }
};

const TERMINAL: &OffsetPlanNode = &OffsetPlanNode {
    id: uuid::Uuid::from_u128(0xbafb_d828_22e3_41a1_98cd_52fc_8b81_2c73u128), 
    offsets: ONE_OFFSET_DIAMOND,
    child: &FixedPlanNode {
        id: uuid::Uuid::from_u128(0x8213_221e_29f3_4325_b333_79fa_a5e2_b8e8),
        placements: &[
            placement(StructureType::Terminal, 0, 0),

            placement(StructureType::Road, -1, 0),
            placement(StructureType::Road, 0, 1),
            placement(StructureType::Road, 1, 0),
            placement(StructureType::Road, 0, 0),
        ],
        children: &[ALL_NODES_ONE_OFFSET_DIAMOND],
        validator: |_, state| state.get_count(StructureType::Terminal) == 0,
        scorer: |_, _, _| 0.5,
    }
};

const LINK: &OffsetPlanNode = &OffsetPlanNode {
    id: uuid::Uuid::from_u128(0x7e20_36db_2ee0_4d33_aec0_190c_258f_34b6u128),
    offsets: ONE_OFFSET_DIAMOND,
    child: &FixedPlanNode {
        id: uuid::Uuid::from_u128(0xacd2_b536_5666_48d7_b9de_97eb_b687_5d74u128),
        placements: &[
            placement(StructureType::Link, 0, 0),

            placement(StructureType::Road, -1, 0),
            placement(StructureType::Road, 0, 1),
            placement(StructureType::Road, 1, 0),
            placement(StructureType::Road, 0, 0),
        ],
        children: &[ALL_NODES_ONE_OFFSET_DIAMOND],
        validator: |_, state| state.get_count(StructureType::Link) == 0,
        scorer: |_, _, _| 0.5,
    }
};

const ROOT_SPAWN: &FixedPlanNode = &FixedPlanNode {
    id: uuid::Uuid::from_u128(0x1533_4930_d790_4a49_b1e0_1e30_acc4_eb46u128),
    placements: &[
        placement(StructureType::Spawn, 0, 0),

        placement(StructureType::Road, -1, 0),
        placement(StructureType::Road, 0, 1),
        placement(StructureType::Road, 1, 0),
        placement(StructureType::Road, 0, -1),
    ],
    children: &[STORAGE, ALL_NODES_ONE_OFFSET_SQUARE],
    validator: |_, state| state.get_count(StructureType::Spawn) == 0,
    scorer: |_, _, _| 1.0,
};

struct FastRoomTerrain {
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

    pub fn get(&self, pos:&Location) -> TerrainFlags {
        let index = (pos.y() as usize * ROOM_WIDTH as usize) + (pos.x() as usize);

        TerrainFlags::from_bits_truncate(self.buffer[index])
    }
}

struct EvaluationStackEntry<'a, 'b> {
    node: &'a dyn PlanNode,
    children: Vec<PlanNodeChild<'b>>,
}

impl<'a, 'b> EvaluationStackEntry<'a, 'b> {
    pub fn to_serialized(&self) -> SerializedEvaluationStackEntry {
        SerializedEvaluationStackEntry {
            node: self.node.id(),
            children: self.children.iter().map(|c| c.to_serialized()).collect(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct SerializedEvaluationStackEntry {
    #[serde(rename = "n")]
    node: uuid::Uuid,
    #[serde(rename = "c")]
    children: Vec<SerializedPlanNodeChild>,
}

impl SerializedEvaluationStackEntry {
    pub fn as_entry<'a>(&self, nodes: &'a HashMap<uuid::Uuid, &dyn PlanNode>) -> Result<EvaluationStackEntry<'a, 'a>, ()> {
        let node = nodes.get(&self.node).ok_or(())?;
        
        let mut children = Vec::new();

        for serialized_child in &self.children {
            let child = serialized_child.as_entry(nodes)?;

            children.push(child);
        }

        Ok(EvaluationStackEntry {
            node: *node,
            children,
        })
    }
}

struct TreePlanner<'t> {
    terrain: &'t FastRoomTerrain,
    completion: fn(&PlannerState) -> bool
}

enum EvaluationResult {
    Complete,
    Placed
}

enum TreePlannerResult {
    Complete,
    Running(Vec<SerializedEvaluationStackEntry>)
}

impl<'t> TreePlanner<'t> {
    pub fn seed<'r, 's>(&self, location: PlanLocation, node: &'r dyn PlanNode, state: &'s mut PlannerState) -> Result<TreePlannerResult, ()> {
        let mut stack = Vec::new();
        
        state.push_layer();
        
        match self.evaluate_node(location, node, state) {
            Ok(EvaluationResult::Complete) => {
                state.commit_layer();

                return Ok(TreePlannerResult::Complete);
            },
            Ok(EvaluationResult::Placed) => {
                let children = self.get_children(location, node, state);

                let mut ordered_children: Vec<_> = children
                    .into_iter()
                    .filter(|e| e.node.desires_placement(self.terrain, state))
                    .map(|e| {
                        let score = e.node.get_score(e.location, self.terrain, state);

                        (e, score)
                    }).collect();

                ordered_children.sort_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap());

                stack.push(EvaluationStackEntry {
                    node,
                    children: ordered_children.into_iter().map(|(e, _)| e).collect(),
                });
            }
            Err(_) => {
                state.pop_layer();

                return Err(());
            }
        }

        let serialized = stack.iter().map(|e| e.to_serialized()).collect();

        Ok(TreePlannerResult::Running(serialized))
    }

    pub fn process<'r, 's, F>(&self, node: &'r dyn PlanNode, state: &'s mut PlannerState, serialized_stack: &Vec<SerializedEvaluationStackEntry>, should_continue: F) -> Result<TreePlannerResult, ()> where F: Fn() -> bool {
        let mut all_nodes = HashMap::new();
        
        node.gather_nodes(&mut all_nodes);

        let mut stack = Vec::new();

        for serialized_entry in serialized_stack.iter() {
            let entry = serialized_entry.as_entry(&all_nodes)?;

            stack.push(entry);
        }

        let mut complete = false;

        while !stack.is_empty() && !complete && should_continue() {
            let mut placed_node = None;

            {
                let entry = stack.last_mut().unwrap();

                while let Some(child) = entry.children.pop() {
                    state.push_layer();

                    match self.evaluate_node(child.location, child.node, state) {
                        Ok(EvaluationResult::Complete) => {
                            state.commit_layer();

                            complete = true;

                            break;
                        },
                        Ok(EvaluationResult::Placed) => {
                            let new_children = self.get_children(child.location, child.node, state);

                            let mut available_children: Vec<_> = entry.children
                                .iter()
                                .chain(new_children.iter())
                                .filter(|e| e.node.desires_placement(self.terrain, state))
                                .map(|e| {
                                    let score = e.node.get_score(e.location, self.terrain, state);
            
                                    (e, score)
                                }).collect();
            
                            available_children.sort_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap());

                            let frame_children: Vec<_> = available_children.into_iter().map(|(e, _)| e).cloned().collect();

                            info!("New children: {}", frame_children.len());

                            placed_node = Some(EvaluationStackEntry {
                                node: child.node,
                                children: frame_children
                            });
                            
                            break;
                        },
                        Err(_) => {
                            state.pop_layer();
                        }
                    }

                    if !should_continue() {
                        break;
                    }
                }
            }

            if let Some(entry) = placed_node {
                stack.push(entry);
            } else if !complete {
                state.pop_layer();

                stack.pop();
            }
        }

        if complete {
            for _ in stack.iter().rev() {
                state.commit_layer();
            }

            Ok(TreePlannerResult::Complete)
        } else if !stack.is_empty() {
            let serialized = stack.iter().map(|e| e.to_serialized()).collect();

            Ok(TreePlannerResult::Running(serialized))
        } else {
            for _ in stack.iter().rev() {
                state.pop_layer();
            }

            Err(())
        }
    }

    pub fn evaluate_node(&self, location: PlanLocation, node: &dyn PlanNode, state: &mut PlannerState) -> Result<EvaluationResult, ()> {
        if node.desires_placement(self.terrain, state) {
            node.place(location, self.terrain, state)?;

            if (self.completion)(state) {
                return Ok(EvaluationResult::Complete);
            }

            return Ok(EvaluationResult::Placed);
        }

        Err(())
    }

    fn get_children<'n>(&self, location: PlanLocation, node: &'n dyn PlanNode, state: &mut PlannerState) -> Vec<PlanNodeChild<'n>> {
        let mut result = Vec::new();
        let mut to_process = node.get_children(location);

        while let Some(child) = to_process.pop() {
            if child.node.immediate() {
                if child.node.desires_placement(self.terrain, state) {
                    state.push_layer();

                    match child.node.place(child.location, self.terrain, state) {
                        Ok(_) => { 
                            state.commit_layer();

                            to_process.append(&mut child.node.get_children(child.location));
                        }
                        Err(_) => {
                            state.pop_layer();
                        }
                    }
                }
            } else {
                result.push(child);
            }
        }

        result
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PlanRunningStateData {
    spawn_candidates: Vec<PlanLocation>,
    planner_state: PlannerState,
    stack: Vec<SerializedEvaluationStackEntry>,
}

impl PlanRunningStateData {
    pub fn visualize(&self, visualizer: &mut RoomVisualizer) {
        self.planner_state.visualize(visualizer);
    }
}

pub enum PlanSeedResult {
    Complete(Plan),
    Running(PlanRunningStateData)
}

pub enum PlanEvaluationResult {
    Complete(Plan),
    Running()
}

static ROOT_DYNAMIC_NODE_ID: uuid::Uuid = uuid::Uuid::from_u128(0x1aab_45ef_d4e2_48d1_b9e3_4b7b_4988_fe8eu128);

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> Planner<'a> {
    pub fn new(room: &Room) -> Planner {
        Planner { room }
    }

    pub fn seed(&self) -> Result<PlanSeedResult, ()> {
        let terrain = self.room.get_terrain();
        let fast_terrain = FastRoomTerrain::new(terrain.get_raw_buffer());

        let spawn_candidates: Vec<PlanLocation> = Self::get_wall_min_distance_locations(&fast_terrain, 5).iter().map(|l| l.into()).collect();

        let root_location = PlanLocation::new(0, 0);

        let root_node = DynamicPlanNode {
            id: ROOT_DYNAMIC_NODE_ID,
            locations: spawn_candidates.clone(),
            child: ROOT_SPAWN,
        };

        let mut planner_state = PlannerState::new();

        let planner = TreePlanner {
            terrain: &fast_terrain,
            completion: Self::is_evaluation_complete
        };

        let seed_result = match planner.seed(root_location, &root_node, &mut planner_state)? {
            TreePlannerResult::Complete => {
                PlanSeedResult::Complete(Plan { state: planner_state.complete() })
            },
            TreePlannerResult::Running(stack) => {
                let running_data = PlanRunningStateData {
                    spawn_candidates: spawn_candidates.clone(),
                    planner_state,
                    stack
                };

                PlanSeedResult::Running(running_data)
            }
        };

        Ok(seed_result)
    }

    pub fn evaluate(&self, evaluation_state: &mut PlanRunningStateData, allowed_cpu: f64) -> Result<PlanEvaluationResult, ()> {
        let terrain = self.room.get_terrain();
        let fast_terrain = FastRoomTerrain::new(terrain.get_raw_buffer());

        let root_node = DynamicPlanNode {
            id: ROOT_DYNAMIC_NODE_ID,
            locations: evaluation_state.spawn_candidates.clone(),
            child: ROOT_SPAWN,
        };

        let planner = TreePlanner {
            terrain: &fast_terrain,
            completion: Self::is_evaluation_complete
        };

        let start_cpu = game::cpu::get_used();

        let should_continue = || game::cpu::get_used() - start_cpu < allowed_cpu;

        let evaluate_result = match planner.process(&root_node, &mut evaluation_state.planner_state, &evaluation_state.stack, should_continue)? {
            TreePlannerResult::Complete => {
                PlanEvaluationResult::Complete(Plan{ state: evaluation_state.planner_state.complete() })
            },
            TreePlannerResult::Running(stack) => {
                evaluation_state.stack = stack;

                PlanEvaluationResult::Running()
            }
        };

        Ok(evaluate_result)
    }

    
    fn is_evaluation_complete(eval_state: &PlannerState) -> bool {
        eval_state.get_count(StructureType::Spawn) >= 1 &&
        eval_state.get_count(StructureType::Extension) >= 60 &&
        eval_state.get_count(StructureType::Storage) >= 1 &&
        eval_state.get_count(StructureType::Terminal) >= 1 &&
        eval_state.get_count(StructureType::Link) >= 1
    }

    fn get_wall_min_distance_locations(terrain: &FastRoomTerrain, min_distance: u32) -> Vec<Location> {
        let mut data: RoomDataArray<Option<u32>> = RoomDataArray::new(None);

        let mut to_apply: HashSet<(usize, usize)> = HashSet::new();

        for y in 0..ROOM_HEIGHT {
            for x in 0..ROOM_WIDTH {
                let terrain = terrain.get(&Location::from_coords(x as u32, y as u32));

                if terrain.contains(TerrainFlags::WALL) || !in_room_build_bounds(x, y) {
                    to_apply.insert((x as usize, y as usize));
                }
            }
        }

        let mut current_distance: u32 = 0;

        while !to_apply.is_empty() {
            let eval_locations = std::mem::replace(&mut to_apply, HashSet::new());

            for pos in &eval_locations {
                let current = data.get_mut(pos.0, pos.1);

                if current.is_none() {
                    *current = Some(current_distance);

                    for x_delta in -1i32..=1i32 {
                        for y_delta in -1i32..=1i32 {
                            if x_delta != 0 && y_delta != 0 {
                                let next_x = pos.0 as i32 + x_delta;
                                let next_y = pos.1 as i32 + y_delta;
                                if in_room_bounds(next_x, next_y) {
                                    to_apply.insert((next_x as usize, next_y as usize));
                                }
                            }
                        }
                    }
                }
            }

            current_distance += 1;
        }

        let mut locations: Vec<((usize, usize), u32)> = data
            .iter()
            .filter_map(|(pos, e)| {
                e.map(|distance| (pos, distance))
            })
            .filter(|(_, distance)| {
                *distance >= min_distance
            })
            .collect();

        locations.sort_by_key(|(_, distance)| *distance);

        locations.iter().map(|(pos, _)| Location::from_coords(pos.0 as u32, pos.1 as u32)).collect()
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
