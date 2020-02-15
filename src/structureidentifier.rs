use screeps::*;
use serde::*;

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

    pub fn as_structure(&self) -> Option<Structure> {
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
