use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

pub struct SerializeMarkerTag;

pub type SerializeMarker = SimpleMarker<SerializeMarkerTag>;

pub type SerializeMarkerAllocator = SimpleMarkerAllocator<SerializeMarkerTag>;

//
// NOTE: EntityVec is a wrapper type due to the built in ConverSaveLoad is overly aggresive
//       at trying to use Serde derived types and ignores that the contents of the vector
//       are ConvertSaveLoad types.
//

#[derive(Clone, Debug)]
pub struct EntityVec(pub Vec<Entity>);

impl EntityVec {
    pub fn new() -> EntityVec {
        EntityVec { 0: vec![] }
    }
}

impl<M: Marker + Serialize> ConvertSaveload<M> for EntityVec
where
    for<'de> M: Deserialize<'de>,
{
    type Data = Vec<M>;
    type Error = NoError;

    fn convert_into<F>(&self, mut ids: F) -> Result<Self::Data, Self::Error>
    where
        F: FnMut(Entity) -> Option<M>,
    {
        let markers = self.0.iter().filter_map(|entity| ids(*entity)).collect();

        Ok(markers)
    }

    fn convert_from<F>(data: Self::Data, mut ids: F) -> Result<Self, Self::Error>
    where
        F: FnMut(M) -> Option<Entity>,
    {
        let entities = data
            .iter()
            .filter_map(|marker| ids(marker.clone()))
            .collect();

        Ok(EntityVec { 0: entities })
    }
}
