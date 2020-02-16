use screeps::*;
use serde::*;

use crate::remoteobjectid::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
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
    pub fn new(structure: &Structure) -> StructureIdentifier {
        match structure {
            Structure::Container(v) => StructureIdentifier::Container(v.id()),
            Structure::Controller(v) => StructureIdentifier::Controller(v.id()),
            Structure::Extension(v) => StructureIdentifier::Extension(v.id()),
            Structure::Extractor(v) => StructureIdentifier::Extractor(v.id()),
            Structure::Factory(v) => StructureIdentifier::Factory(v.id()),
            Structure::InvaderCore(v) => StructureIdentifier::InvaderCore(v.id()),
            Structure::KeeperLair(v) => StructureIdentifier::KeeperLair(v.id()),
            Structure::Lab(v) => StructureIdentifier::Lab(v.id()),
            Structure::Link(v) => StructureIdentifier::Link(v.id()),
            Structure::Nuker(v) => StructureIdentifier::Nuker(v.id()),
            Structure::Observer(v) => StructureIdentifier::Observer(v.id()),
            Structure::PowerBank(v) => StructureIdentifier::PowerBank(v.id()),
            Structure::PowerSpawn(v) => StructureIdentifier::PowerSpawn(v.id()),
            Structure::Portal(v) => StructureIdentifier::Portal(v.id()),
            Structure::Rampart(v) => StructureIdentifier::Rampart(v.id()),
            Structure::Road(v) => StructureIdentifier::Road(v.id()),
            Structure::Spawn(v) => StructureIdentifier::Spawn(v.id()),
            Structure::Storage(v) => StructureIdentifier::Storage(v.id()),
            Structure::Terminal(v) => StructureIdentifier::Terminal(v.id()),
            Structure::Tower(v) => StructureIdentifier::Tower(v.id()),
            Structure::Wall(v) => StructureIdentifier::Wall(v.id()),
        }
    }

    pub fn resolve(&self) -> Option<Structure> {
        match self {
            StructureIdentifier::Container(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Controller(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Extension(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Extractor(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Factory(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::InvaderCore(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::KeeperLair(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Lab(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Link(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Nuker(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Observer(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::PowerBank(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::PowerSpawn(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Portal(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Rampart(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Road(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Spawn(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Storage(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Terminal(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Tower(id) => id.resolve().map(|s| s.as_structure()),
            StructureIdentifier::Wall(id) => id.resolve().map(|s| s.as_structure()),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
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
    pub fn new(structure: &Structure) -> RemoteStructureIdentifier {
        match structure {
            Structure::Container(v) => RemoteStructureIdentifier::Container(v.remote_id()),
            Structure::Controller(v) => RemoteStructureIdentifier::Controller(v.remote_id()),
            Structure::Extension(v) => RemoteStructureIdentifier::Extension(v.remote_id()),
            Structure::Extractor(v) => RemoteStructureIdentifier::Extractor(v.remote_id()),
            Structure::Factory(v) => RemoteStructureIdentifier::Factory(v.remote_id()),
            Structure::InvaderCore(v) => RemoteStructureIdentifier::InvaderCore(v.remote_id()),
            Structure::KeeperLair(v) => RemoteStructureIdentifier::KeeperLair(v.remote_id()),
            Structure::Lab(v) => RemoteStructureIdentifier::Lab(v.remote_id()),
            Structure::Link(v) => RemoteStructureIdentifier::Link(v.remote_id()),
            Structure::Nuker(v) => RemoteStructureIdentifier::Nuker(v.remote_id()),
            Structure::Observer(v) => RemoteStructureIdentifier::Observer(v.remote_id()),
            Structure::PowerBank(v) => RemoteStructureIdentifier::PowerBank(v.remote_id()),
            Structure::PowerSpawn(v) => RemoteStructureIdentifier::PowerSpawn(v.remote_id()),
            Structure::Portal(v) => RemoteStructureIdentifier::Portal(v.remote_id()),
            Structure::Rampart(v) => RemoteStructureIdentifier::Rampart(v.remote_id()),
            Structure::Road(v) => RemoteStructureIdentifier::Road(v.remote_id()),
            Structure::Spawn(v) => RemoteStructureIdentifier::Spawn(v.remote_id()),
            Structure::Storage(v) => RemoteStructureIdentifier::Storage(v.remote_id()),
            Structure::Terminal(v) => RemoteStructureIdentifier::Terminal(v.remote_id()),
            Structure::Tower(v) => RemoteStructureIdentifier::Tower(v.remote_id()),
            Structure::Wall(v) => RemoteStructureIdentifier::Wall(v.remote_id()),
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

    pub fn resolve(&self) -> Option<Structure> {
        match self {
            RemoteStructureIdentifier::Container(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Controller(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Extension(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Extractor(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Factory(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::InvaderCore(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::KeeperLair(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Lab(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Link(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Nuker(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Observer(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::PowerBank(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::PowerSpawn(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Portal(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Rampart(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Road(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Spawn(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Storage(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Terminal(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Tower(id) => id.resolve().map(|s| s.as_structure()),
            RemoteStructureIdentifier::Wall(id) => id.resolve().map(|s| s.as_structure()),
        }
    }
}
