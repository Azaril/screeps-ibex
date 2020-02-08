use specs::saveload::*;

pub struct SerializeMarkerTag;

pub type SerializeMarker = SimpleMarker<SerializeMarkerTag>;

pub type SerializeMarkerAllocator = SimpleMarkerAllocator<SerializeMarkerTag>;