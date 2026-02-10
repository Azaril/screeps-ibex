use super::data::*;
use super::roomplansystem::*;
use crate::visualize::*;
use specs::prelude::*;

// ---------------------------------------------------------------------------
// RoomPlanVisualizeSystem — renders completed room plans
// ---------------------------------------------------------------------------

#[derive(SystemData)]
pub struct RoomPlanVisualizeSystemData<'a> {
    room_data: ReadStorage<'a, RoomData>,
    room_plan_data: ReadStorage<'a, RoomPlanData>,
    visualizer: Option<Write<'a, Visualizer>>,
}

/// Renders completed room plans using the screeps-visual structure visuals.
///
/// Only runs when the `construction.visualize.plan` feature flag is enabled.
/// Inserted into the dispatcher after `RoomPlanSystem` and before
/// `RenderSystem` / `ApplyVisualsSystem`.
pub struct RoomPlanVisualizeSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RoomPlanVisualizeSystem {
    type SystemData = RoomPlanVisualizeSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let features = crate::features::features();

        if !features.construction.visualize.plan() {
            return;
        }

        let Some(visualizer) = data.visualizer.as_deref_mut() else {
            return;
        };

        for (room_data, room_plan_data) in (&data.room_data, &data.room_plan_data).join() {
            if let Some(plan) = room_plan_data.plan() {
                let room_vis = visualizer.get_room(room_data.name);
                plan.visualize(room_vis);
            }
        }
    }
}
