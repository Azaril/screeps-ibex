use crate::visualize::*;
use screeps::*;

/// Thin wrapper around Visualizer that provides per-room and global access
/// to RoomVisualizer. Transfer/order systems use this for their specialized
/// visualizations (demand/haul overlay, etc.).
pub struct UISystem;

impl Default for UISystem {
    fn default() -> UISystem {
        UISystem::new()
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl UISystem {
    pub fn new() -> UISystem {
        UISystem
    }

    pub fn with_global<T>(&mut self, visualizer: &mut Visualizer, callback: T)
    where
        T: Fn(&mut RoomVisualizer),
    {
        let global_visualizer = visualizer.global();
        callback(global_visualizer);
    }

    pub fn with_room<T>(&mut self, room: RoomName, visualizer: &mut Visualizer, callback: T)
    where
        T: FnOnce(&mut RoomVisualizer),
    {
        let room_visualizer = visualizer.get_room(room);
        callback(room_visualizer);
    }
}
