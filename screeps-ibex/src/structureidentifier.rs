use screeps::*;
use serde::*;

use crate::remoteobjectid::*;

#[derive(Clone, Copy, Deserialize, Serialize)]
pub enum StructureIdentifier {
    Container(ObjectId<StructureContainer>),
    Controller(ObjectId<StructureController>),
    Extension(ObjectId<StructureExtension>),
    Extractor(ObjectId<StructureExtractor>),
    Factory(ObjectId<StructureFactory>),
    InvaderCore(ObjectId<StructureInvaderCore>),
    KeeperLair(ObjectId<StructureKeeperLair>),
    Lab(ObjectId<StructureLab>),
    Link(ObjectId<StructureLink>),
    Nuker(ObjectId<StructureNuker>),
    Observer(ObjectId<StructureObserver>),
    PowerBank(ObjectId<StructurePowerBank>),
    PowerSpawn(ObjectId<StructurePowerSpawn>),
    Portal(ObjectId<StructurePortal>),
    Rampart(ObjectId<StructureRampart>),
    Road(ObjectId<StructureRoad>),
    Spawn(ObjectId<StructureSpawn>),
    Storage(ObjectId<StructureStorage>),
    Terminal(ObjectId<StructureTerminal>),
    Tower(ObjectId<StructureTower>),
    Wall(ObjectId<StructureWall>),
}

impl StructureIdentifier {
    pub fn new(structure: &StructureObject) -> StructureIdentifier {
        match structure {
            StructureObject::StructureContainer(v) => StructureIdentifier::Container(v.id()),
            StructureObject::StructureController(v) => StructureIdentifier::Controller(v.id()),
            StructureObject::StructureExtension(v) => StructureIdentifier::Extension(v.id()),
            StructureObject::StructureExtractor(v) => StructureIdentifier::Extractor(v.id()),
            StructureObject::StructureFactory(v) => StructureIdentifier::Factory(v.id()),
            StructureObject::StructureInvaderCore(v) => StructureIdentifier::InvaderCore(v.id()),
            StructureObject::StructureKeeperLair(v) => StructureIdentifier::KeeperLair(v.id()),
            StructureObject::StructureLab(v) => StructureIdentifier::Lab(v.id()),
            StructureObject::StructureLink(v) => StructureIdentifier::Link(v.id()),
            StructureObject::StructureNuker(v) => StructureIdentifier::Nuker(v.id()),
            StructureObject::StructureObserver(v) => StructureIdentifier::Observer(v.id()),
            StructureObject::StructurePowerBank(v) => StructureIdentifier::PowerBank(v.id()),
            StructureObject::StructurePowerSpawn(v) => StructureIdentifier::PowerSpawn(v.id()),
            StructureObject::StructurePortal(v) => StructureIdentifier::Portal(v.id()),
            StructureObject::StructureRampart(v) => StructureIdentifier::Rampart(v.id()),
            StructureObject::StructureRoad(v) => StructureIdentifier::Road(v.id()),
            StructureObject::StructureSpawn(v) => StructureIdentifier::Spawn(v.id()),
            StructureObject::StructureStorage(v) => StructureIdentifier::Storage(v.id()),
            StructureObject::StructureTerminal(v) => StructureIdentifier::Terminal(v.id()),
            StructureObject::StructureTower(v) => StructureIdentifier::Tower(v.id()),
            StructureObject::StructureWall(v) => StructureIdentifier::Wall(v.id()),
        }
    }

    pub fn resolve(&self) -> Option<StructureObject> {
        match self {
            StructureIdentifier::Container(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Controller(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Extension(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Extractor(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Factory(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::InvaderCore(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::KeeperLair(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Lab(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Link(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Nuker(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Observer(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::PowerBank(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::PowerSpawn(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Portal(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Rampart(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Road(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Spawn(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Storage(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Terminal(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Tower(id) => id.resolve().map(|s| s.into()),
            StructureIdentifier::Wall(id) => id.resolve().map(|s| s.into()),
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
pub enum RemoteStructureIdentifier {
    Container(RemoteObjectId<StructureContainer>),
    Controller(RemoteObjectId<StructureController>),
    Extension(RemoteObjectId<StructureExtension>),
    Extractor(RemoteObjectId<StructureExtractor>),
    Factory(RemoteObjectId<StructureFactory>),
    InvaderCore(RemoteObjectId<StructureInvaderCore>),
    KeeperLair(RemoteObjectId<StructureKeeperLair>),
    Lab(RemoteObjectId<StructureLab>),
    Link(RemoteObjectId<StructureLink>),
    Nuker(RemoteObjectId<StructureNuker>),
    Observer(RemoteObjectId<StructureObserver>),
    PowerBank(RemoteObjectId<StructurePowerBank>),
    PowerSpawn(RemoteObjectId<StructurePowerSpawn>),
    Portal(RemoteObjectId<StructurePortal>),
    Rampart(RemoteObjectId<StructureRampart>),
    Road(RemoteObjectId<StructureRoad>),
    Spawn(RemoteObjectId<StructureSpawn>),
    Storage(RemoteObjectId<StructureStorage>),
    Terminal(RemoteObjectId<StructureTerminal>),
    Tower(RemoteObjectId<StructureTower>),
    Wall(RemoteObjectId<StructureWall>),
}

impl RemoteStructureIdentifier {
    pub fn new(structure: &StructureObject) -> RemoteStructureIdentifier {
        match structure {
            StructureObject::StructureContainer(v) => RemoteStructureIdentifier::Container(v.remote_id()),
            StructureObject::StructureController(v) => RemoteStructureIdentifier::Controller(v.remote_id()),
            StructureObject::StructureExtension(v) => RemoteStructureIdentifier::Extension(v.remote_id()),
            StructureObject::StructureExtractor(v) => RemoteStructureIdentifier::Extractor(v.remote_id()),
            StructureObject::StructureFactory(v) => RemoteStructureIdentifier::Factory(v.remote_id()),
            StructureObject::StructureInvaderCore(v) => RemoteStructureIdentifier::InvaderCore(v.remote_id()),
            StructureObject::StructureKeeperLair(v) => RemoteStructureIdentifier::KeeperLair(v.remote_id()),
            StructureObject::StructureLab(v) => RemoteStructureIdentifier::Lab(v.remote_id()),
            StructureObject::StructureLink(v) => RemoteStructureIdentifier::Link(v.remote_id()),
            StructureObject::StructureNuker(v) => RemoteStructureIdentifier::Nuker(v.remote_id()),
            StructureObject::StructureObserver(v) => RemoteStructureIdentifier::Observer(v.remote_id()),
            StructureObject::StructurePowerBank(v) => RemoteStructureIdentifier::PowerBank(v.remote_id()),
            StructureObject::StructurePowerSpawn(v) => RemoteStructureIdentifier::PowerSpawn(v.remote_id()),
            StructureObject::StructurePortal(v) => RemoteStructureIdentifier::Portal(v.remote_id()),
            StructureObject::StructureRampart(v) => RemoteStructureIdentifier::Rampart(v.remote_id()),
            StructureObject::StructureRoad(v) => RemoteStructureIdentifier::Road(v.remote_id()),
            StructureObject::StructureSpawn(v) => RemoteStructureIdentifier::Spawn(v.remote_id()),
            StructureObject::StructureStorage(v) => RemoteStructureIdentifier::Storage(v.remote_id()),
            StructureObject::StructureTerminal(v) => RemoteStructureIdentifier::Terminal(v.remote_id()),
            StructureObject::StructureTower(v) => RemoteStructureIdentifier::Tower(v.remote_id()),
            StructureObject::StructureWall(v) => RemoteStructureIdentifier::Wall(v.remote_id()),
        }
    }

    pub fn pos(&self) -> Position {
        match self {
            RemoteStructureIdentifier::Container(id) => id.pos(),
            RemoteStructureIdentifier::Controller(id) => id.pos(),
            RemoteStructureIdentifier::Extension(id) => id.pos(),
            RemoteStructureIdentifier::Extractor(id) => id.pos(),
            RemoteStructureIdentifier::Factory(id) => id.pos(),
            RemoteStructureIdentifier::InvaderCore(id) => id.pos(),
            RemoteStructureIdentifier::KeeperLair(id) => id.pos(),
            RemoteStructureIdentifier::Lab(id) => id.pos(),
            RemoteStructureIdentifier::Link(id) => id.pos(),
            RemoteStructureIdentifier::Nuker(id) => id.pos(),
            RemoteStructureIdentifier::Observer(id) => id.pos(),
            RemoteStructureIdentifier::PowerBank(id) => id.pos(),
            RemoteStructureIdentifier::PowerSpawn(id) => id.pos(),
            RemoteStructureIdentifier::Portal(id) => id.pos(),
            RemoteStructureIdentifier::Rampart(id) => id.pos(),
            RemoteStructureIdentifier::Road(id) => id.pos(),
            RemoteStructureIdentifier::Spawn(id) => id.pos(),
            RemoteStructureIdentifier::Storage(id) => id.pos(),
            RemoteStructureIdentifier::Terminal(id) => id.pos(),
            RemoteStructureIdentifier::Tower(id) => id.pos(),
            RemoteStructureIdentifier::Wall(id) => id.pos(),
        }
    }

    pub fn resolve(&self) -> Option<StructureObject> {
        match self {
            RemoteStructureIdentifier::Container(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Controller(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Extension(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Extractor(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Factory(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::InvaderCore(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::KeeperLair(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Lab(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Link(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Nuker(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Observer(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::PowerBank(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::PowerSpawn(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Portal(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Rampart(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Road(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Spawn(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Storage(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Terminal(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Tower(id) => id.resolve().map(|s| s.into()),
            RemoteStructureIdentifier::Wall(id) => id.resolve().map(|s| s.into()),
        }
    }
}
