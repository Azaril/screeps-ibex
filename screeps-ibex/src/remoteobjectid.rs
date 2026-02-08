use screeps::*;
use serde::de::*;
use serde::ser::*;
use std::hash::*;
use wasm_bindgen::JsCast;

pub struct RemoteObjectId<T> {
    position: Position,
    id: ObjectId<T>,
}

impl<T> RemoteObjectId<T> {
    pub fn new_from_components(id: ObjectId<T>, position: Position) -> RemoteObjectId<T> {
        RemoteObjectId { id, position }
    }

    pub fn new(obj: &T) -> RemoteObjectId<T>
    where
        T: HasPosition + HasId + JsCast,
    {
        RemoteObjectId {
            id: obj.id(),
            position: obj.pos(),
        }
    }

    pub fn id(&self) -> ObjectId<T> {
        self.id
    }

    pub fn pos(&self) -> Position {
        self.position
    }

    pub fn resolve(self) -> Option<T>
    where
        T: MaybeHasId + JsCast,
    {
        self.id.resolve()
    }
}

impl<T> std::fmt::Debug for RemoteObjectId<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({:?}, {:?})", self.id, self.position)
    }
}

impl<T> Serialize for RemoteObjectId<T> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_tuple(2)?;
        s.serialize_element(&self.id)?;
        s.serialize_element(&self.position)?;
        s.end()
    }
}

impl<'de, T> Deserialize<'de> for RemoteObjectId<T> {
    fn deserialize<D>(deserializer: D) -> std::result::Result<RemoteObjectId<T>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Vis<T> {
            phantom: std::marker::PhantomData<T>,
        }

        impl<'de, T> Visitor<'de> for Vis<T> {
            type Value = RemoteObjectId<T>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("Room name and object id")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error> {
                if let Some(id) = seq.next_element()? {
                    if let Some(position) = seq.next_element()? {
                        return Ok(RemoteObjectId { id, position });
                    }
                }

                Err(serde::de::Error::invalid_length(2, &self))
            }
        }

        deserializer.deserialize_tuple(
            2,
            Vis::<T> {
                phantom: std::marker::PhantomData,
            },
        )
    }
}

impl<T> Copy for RemoteObjectId<T> {}

impl<T> Clone for RemoteObjectId<T> {
    fn clone(&self) -> RemoteObjectId<T> {
        RemoteObjectId {
            id: self.id,
            position: self.position,
        }
    }
}

impl<T> PartialEq for RemoteObjectId<T> {
    fn eq(&self, o: &RemoteObjectId<T>) -> bool {
        self.id.eq(&o.id) && self.position.eq(&o.position)
    }
}

impl<T> Eq for RemoteObjectId<T> {}

impl<T> Hash for RemoteObjectId<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.position.hash(state);
    }
}

pub trait HasRemoteObjectId<T>
where
    T: Sized + HasId + JsCast,
{
    fn remote_id(&self) -> RemoteObjectId<T>;
}

impl<T: Sized + HasId + JsCast + HasPosition> HasRemoteObjectId<T> for T {
    fn remote_id(&self) -> RemoteObjectId<Self> {
        RemoteObjectId {
            id: self.id(),
            position: self.pos(),
        }
    }
}
