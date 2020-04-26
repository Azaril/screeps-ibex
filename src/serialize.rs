use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use std::collections::HashMap;
use std::hash::Hash;
use std::iter::Iterator;

pub struct SerializeMarkerTag;

pub type SerializeMarker = SimpleMarker<SerializeMarkerTag>;

pub type SerializeMarkerAllocator = SimpleMarkerAllocator<SerializeMarkerTag>;

//
// NOTE: EntityVec is a wrapper type due to the built in ConvertSaveLoad is overly aggresive
//       at trying to use Serde derived types and ignores that the contents of the vector
//       are ConvertSaveLoad types.
//

#[derive(Clone, Debug)]
pub struct EntityVec<T>(Vec<T>);

impl<T> EntityVec<T> {
    pub fn new() -> EntityVec<T> {
        EntityVec(Vec::new())
    }

    pub fn with_capacity(capacity: usize) -> EntityVec<T> {
        EntityVec(Vec::with_capacity(capacity))
    }
}

impl<T> std::ops::Deref for EntityVec<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T> std::ops::DerefMut for EntityVec<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.0
    }
}

impl<T> From<&[T]> for EntityVec<T>
where
    T: Clone,
{
    fn from(other: &[T]) -> EntityVec<T> {
        EntityVec(other.to_vec())
    }
}

impl<C, M: Serialize + Marker> ConvertSaveload<M> for EntityVec<C>
where
    for<'de> M: Deserialize<'de>,
    C: ConvertSaveload<M>,
{
    type Data = Vec<<C as ConvertSaveload<M>>::Data>;
    type Error = <C as ConvertSaveload<M>>::Error;

    fn convert_into<F>(&self, mut ids: F) -> Result<Self::Data, Self::Error>
    where
        F: FnMut(Entity) -> Option<M>,
    {
        let mut output = Vec::with_capacity(self.len());

        for item in self.iter() {
            let converted_item = item.convert_into(|entity| ids(entity))?;

            output.push(converted_item);
        }

        Ok(output)
    }

    fn convert_from<F>(data: Self::Data, mut ids: F) -> Result<Self, Self::Error>
    where
        F: FnMut(M) -> Option<Entity>,
    {
        let mut output: EntityVec<C> = EntityVec::with_capacity(data.len());

        for item in data.into_iter() {
            let converted_item = ConvertSaveload::convert_from(item, |marker| ids(marker))?;

            output.push(converted_item);
        }

        Ok(output)
    }
}

#[derive(Clone, Debug)]
pub struct EntityOption<T>(Option<T>);

impl<T> std::ops::Deref for EntityOption<T> {
    type Target = Option<T>;

    fn deref(&self) -> &Option<T> {
        &self.0
    }
}

impl<T> std::ops::DerefMut for EntityOption<T> {
    fn deref_mut(&mut self) -> &mut Option<T> {
        &mut self.0
    }
}

impl<T> From<Option<T>> for EntityOption<T> {
    fn from(value: Option<T>) -> EntityOption<T> {
        EntityOption(value)
    }
}

impl<C, M: Serialize + Marker> ConvertSaveload<M> for EntityOption<C>
where
    for<'de> M: Deserialize<'de>,
    C: ConvertSaveload<M>,
{
    type Data = Option<<C as ConvertSaveload<M>>::Data>;
    type Error = <C as ConvertSaveload<M>>::Error;

    fn convert_into<F>(&self, mut ids: F) -> Result<Self::Data, Self::Error>
    where
        F: FnMut(Entity) -> Option<M>,
    {
        if let Some(item) = &self.0 {
            let converted_item = item.convert_into(|entity| ids(entity))?;

            Ok(Some(converted_item))
        } else {
            Ok(None)
        }
    }

    fn convert_from<F>(data: Self::Data, mut ids: F) -> Result<Self, Self::Error>
    where
        F: FnMut(M) -> Option<Entity>,
    {
        if let Some(item) = data {
            let converted_item = ConvertSaveload::convert_from(item, |marker| ids(marker))?;

            Ok(EntityOption(Some(converted_item)))
        } else {
            Ok(EntityOption(None))
        }
    }
}

#[derive(Clone, Debug)]
pub struct EntityHashMap<K, V>(HashMap<K, V>);

impl<K, V> EntityHashMap<K, V> {
    pub fn new() -> EntityHashMap<K, V> {
        EntityHashMap(HashMap::new())
    }
}

impl<K, V> std::ops::Deref for EntityHashMap<K, V> {
    type Target = HashMap<K, V>;

    fn deref(&self) -> &HashMap<K, V> {
        &self.0
    }
}

impl<K, V> std::ops::DerefMut for EntityHashMap<K, V> {
    fn deref_mut(&mut self) -> &mut HashMap<K, V> {
        &mut self.0
    }
}

impl<K, V, M: Serialize + Marker> ConvertSaveload<M> for EntityHashMap<K, V>
where
    for<'de> M: Deserialize<'de>,
    for<'de> K: Deserialize<'de>,
    K: Serialize + std::hash::Hash + Eq + Clone,
    V: ConvertSaveload<M>,
{
    type Data = HashMap<K, <V as ConvertSaveload<M>>::Data>;
    type Error = <V as ConvertSaveload<M>>::Error;

    fn convert_into<F>(&self, mut ids: F) -> Result<Self::Data, Self::Error>
    where
        F: FnMut(Entity) -> Option<M>,
    {
        let mut output: HashMap<K, <V as ConvertSaveload<M>>::Data> = HashMap::new();

        for (key, item) in self.iter() {
            let converted_item = item.convert_into(|entity| ids(entity))?;

            output.insert(key.clone(), converted_item);
        }

        Ok(output)
    }

    fn convert_from<F>(data: Self::Data, mut ids: F) -> Result<Self, Self::Error>
    where
        F: FnMut(M) -> Option<Entity>,
    {
        let mut output: EntityHashMap<K, V> = EntityHashMap::new();

        for (key, item) in data.into_iter() {
            let converted_item = ConvertSaveload::convert_from(item, |marker| ids(marker))?;

            output.insert(key, converted_item);
        }

        Ok(output)
    }
}

pub trait EntityItertools : Iterator {
    fn into_entity_group_map<K, V>(self) -> EntityHashMap<K, EntityVec<V>>
        where Self: Iterator<Item=(K, V)> + Sized,
            K: Hash + Eq,
    {
        let mut lookup = EntityHashMap::new();

        for (key, val) in self {
            lookup.entry(key).or_insert(EntityVec::new()).push(val);
        }

        lookup
    }
}

impl<T: ?Sized> EntityItertools for T where T: Iterator { }

pub fn encode_to_string<T>(data: T) -> Result<String, String> where T: Serialize {
    let serialized_data = bincode::serialize(&data).map_err(|e| e.to_string())?;

    let encoded_data = base64::encode(&serialized_data);

    Ok(encoded_data)
}

pub fn decode_from_string<T>(data: &str) -> Result<T, String> where for<'de> T: Deserialize<'de> {
    let decoded_data = base64::decode(data).map_err(|e| e.to_string())?;

    let data = bincode::deserialize_from(decoded_data.as_slice()).map_err(|e| e.to_string())?;

    Ok(data)
}