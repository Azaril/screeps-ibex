//! Visualization data types, gather system, and render system.
//!
//! Typed summary data keyed by room (and global operations) for rendering.
//! Not serialized; recreated each tick when visualization is on.
//!
//! UI layout: Summary → combined text per panel → layout (positions/sizes) → render (rect + text).
//! Text is combined in the render phase so we minimize primitives when explicit size is unavailable.

use crate::creep::CreepOwner;
use crate::jobs::data::JobData;
use crate::missions::data::MissionData;
use crate::operations::data::OperationData;
use crate::room::data::RoomData;
use crate::spawnsystem::SpawnQueue;
use crate::visualize::Visualizer;
use screeps::game;
use screeps::traits::SharedCreepProperties;
use screeps::{LineDrawStyle, LineStyle, PolyStyle, RectStyle, ResourceType, RoomName, TextAlign, TextStyle};
use specs::prelude::*;
use specs::*;
use std::collections::HashMap;

// ─── Structured summary content ──────────────────────────────────────────────

/// Structured content for a summary component.
/// RenderSystem knows how to layout each variant.
#[derive(Debug, Clone)]
pub enum SummaryContent {
    /// Single-line text.
    Text(String),
    /// Multiple lines (header + items).
    Lines { header: String, items: Vec<String> },
    /// Tree: a label with optional nested children (one level of nesting).
    Tree { label: String, children: Vec<SummaryContent> },
}

impl SummaryContent {
    /// Flatten content into lines for panel rendering.
    pub fn to_lines(&self) -> Vec<String> {
        match self {
            SummaryContent::Text(s) => vec![s.clone()],
            SummaryContent::Lines { header, items } => {
                let mut lines = vec![header.clone()];
                lines.extend(items.iter().cloned());
                lines
            }
            SummaryContent::Tree { label, children } => {
                let mut lines = vec![label.clone()];
                for child in children {
                    for (i, line) in child.to_lines().iter().enumerate() {
                        if i == 0 {
                            lines.push(format!("  {}", line));
                        } else {
                            lines.push(format!("    {}", line));
                        }
                    }
                }
                lines
            }
        }
    }
}

// ─── Per-entity summary components (not serialized) ──────────────────────────
//
// Each entity type that appears in the overlay carries its own summary component.
// Summarization systems fill these after the domain Run* systems, and the
// AggregateSummarySystem reads *only* these components to build VisualizationData.

/// Summary of an operation entity for the global ops list.
#[derive(Component, Debug, Clone)]
#[storage(DenseVecStorage)]
pub struct OperationSummaryComponent {
    pub content: SummaryContent,
}

/// Summary of a mission entity for the per-room missions list.
#[derive(Component, Debug, Clone)]
#[storage(DenseVecStorage)]
pub struct MissionSummaryComponent {
    pub content: SummaryContent,
}

/// Summary of a creep's job for the per-room jobs list.
#[derive(Component, Debug, Clone)]
#[storage(DenseVecStorage)]
pub struct JobSummaryComponent {
    pub creep_name: String,
    pub content: SummaryContent,
}

/// Summary of room visibility/ownership for display.
#[derive(Component, Debug, Clone)]
#[storage(DenseVecStorage)]
pub struct RoomVisibilitySummaryComponent {
    pub visible: bool,
    pub age: u32,
    pub owner: String,
    pub reservation: String,
    pub source_keeper: String,
    pub hostile_creeps: bool,
    pub hostile_structures: bool,
}

// ─── Summarization systems ───────────────────────────────────────────────────

/// Reads OperationData and writes OperationSummaryComponent on each operation entity.
pub struct SummarizeOperationSystem;

#[derive(SystemData)]
pub struct SummarizeOperationSystemData<'a> {
    viz_gate: Option<Read<'a, VisualizationData>>,
    entities: Entities<'a>,
    operation_data: ReadStorage<'a, OperationData>,
    mission_data: ReadStorage<'a, MissionData>,
    room_data: ReadStorage<'a, RoomData>,
    op_summary: WriteStorage<'a, OperationSummaryComponent>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for SummarizeOperationSystem {
    type SystemData = SummarizeOperationSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if data.viz_gate.is_none() {
            return;
        }

        let ctx = crate::operations::operationsystem::OperationDescribeContext {
            mission_data: &data.mission_data,
            room_data: &data.room_data,
        };

        for (entity, op_data) in (&data.entities, &data.operation_data).join() {
            let content = op_data.describe_operation(&ctx);
            let _ = data.op_summary.insert(entity, OperationSummaryComponent { content });
        }
    }
}

/// Reads MissionData and writes MissionSummaryComponent on each mission entity.
pub struct SummarizeMissionSystem;

#[derive(SystemData)]
pub struct SummarizeMissionSystemData<'a> {
    viz_gate: Option<Read<'a, VisualizationData>>,
    entities: Entities<'a>,
    mission_data: ReadStorage<'a, MissionData>,
    mission_summary: WriteStorage<'a, MissionSummaryComponent>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for SummarizeMissionSystem {
    type SystemData = SummarizeMissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if data.viz_gate.is_none() {
            return;
        }

        for (entity, mission_data) in (&data.entities, &data.mission_data).join() {
            let content = mission_data.summarize();
            let _ = data.mission_summary.insert(entity, MissionSummaryComponent { content });
        }
    }
}

/// Reads CreepOwner + JobData and writes JobSummaryComponent on each creep entity.
pub struct SummarizeJobSystem;

#[derive(SystemData)]
pub struct SummarizeJobSystemData<'a> {
    viz_gate: Option<Read<'a, VisualizationData>>,
    entities: Entities<'a>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    job_data: ReadStorage<'a, JobData>,
    job_summary: WriteStorage<'a, JobSummaryComponent>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for SummarizeJobSystem {
    type SystemData = SummarizeJobSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if data.viz_gate.is_none() {
            return;
        }

        for (entity, creep_owner, job_data) in (&data.entities, &data.creep_owner, &data.job_data).join() {
            let creep_name = creep_owner.owner.resolve().map(|c| c.name()).unwrap_or_default();

            let content = job_data.summarize();
            let _ = data.job_summary.insert(entity, JobSummaryComponent { creep_name, content });
        }
    }
}

/// Reads RoomData dynamic visibility and writes RoomVisibilitySummaryComponent on each room entity.
pub struct SummarizeRoomVisibilitySystem;

#[derive(SystemData)]
pub struct SummarizeRoomVisibilitySystemData<'a> {
    viz_gate: Option<Read<'a, VisualizationData>>,
    entities: Entities<'a>,
    room_data: ReadStorage<'a, RoomData>,
    vis_summary: WriteStorage<'a, RoomVisibilitySummaryComponent>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for SummarizeRoomVisibilitySystem {
    type SystemData = SummarizeRoomVisibilitySystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if data.viz_gate.is_none() {
            return;
        }

        for (entity, room_data) in (&data.entities, &data.room_data).join() {
            if let Some(dyn_vis) = room_data.get_dynamic_visibility_data() {
                let _ = data.vis_summary.insert(
                    entity,
                    RoomVisibilitySummaryComponent {
                        visible: dyn_vis.visible(),
                        age: dyn_vis.age(),
                        owner: format!("{}", dyn_vis.owner()),
                        reservation: format!("{}", dyn_vis.reservation()),
                        source_keeper: format!("{}", dyn_vis.source_keeper()),
                        hostile_creeps: dyn_vis.hostile_creeps(),
                        hostile_structures: dyn_vis.hostile_structures(),
                    },
                );
            }
        }
    }
}

/// In-memory CPU usage history for histogram. Not serialized; lost on VM reset.
const CPU_HISTORY_LEN: usize = 48;

#[derive(Default)]
pub struct CpuHistory {
    pub samples: Vec<f32>,
    /// Game time when the last sample was pushed.
    pub tick: u32,
    /// CPU tick limit when the last sample was pushed.
    pub tick_limit: f32,
}

impl CpuHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, used: f32, tick: u32, tick_limit: f32) {
        self.samples.push(used);
        if self.samples.len() > CPU_HISTORY_LEN {
            self.samples.remove(0);
        }
        self.tick = tick;
        self.tick_limit = tick_limit;
    }
}

// ─── CPU tracking system ─────────────────────────────────────────────────────

/// Pushes a CPU sample (plus game tick and tick limit) into CpuHistory.
/// Runs once per tick when visualization is on, immediately before RenderSystem.
/// This is the **only** place the visualization path calls game CPU/time APIs.
pub struct CpuTrackingSystem;

#[derive(SystemData)]
pub struct CpuTrackingSystemData<'a> {
    cpu_history: Option<Write<'a, CpuHistory>>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for CpuTrackingSystem {
    type SystemData = CpuTrackingSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if let Some(ref mut hist) = data.cpu_history {
            let used = game::cpu::get_used() as f32;
            let tick = game::time();
            let tick_limit = game::cpu::tick_limit() as f32;
            hist.push(used, tick, tick_limit);
        }
    }
}

// ─── Summary types (data only, no rendering) ─────────────────────────────────

/// One operation entry for the global operations list.
#[derive(Debug, Clone)]
pub struct OperationSummary {
    pub content: SummaryContent,
}

/// One mission entry for a room.
#[derive(Debug, Clone)]
pub struct MissionSummary {
    pub content: SummaryContent,
}

/// One job entry for a room (creep name + content).
#[derive(Debug, Clone)]
pub struct JobSummary {
    pub creep_name: String,
    pub content: SummaryContent,
}

/// Room visibility / ownership summary for display.
#[derive(Debug, Clone)]
pub struct RoomVisibilitySummary {
    pub visible: bool,
    pub age: u32,
    pub owner: String,
    pub reservation: String,
    pub source_keeper: String,
    pub hostile_creeps: bool,
    pub hostile_structures: bool,
}

/// One spawn queue entry for a room.
#[derive(Debug, Clone)]
pub struct SpawnQueueEntry {
    pub priority: f32,
    pub cost: u32,
    pub description: String,
}

/// Per-room visualization data (missions, jobs, spawn queue, room info, stats history, transfer stats).
#[derive(Debug, Default, Clone)]
pub struct RoomVisualizationData {
    pub missions: Vec<MissionSummary>,
    pub jobs: Vec<JobSummary>,
    pub spawn_queue: Vec<SpawnQueueEntry>,
    pub room_visibility: Option<RoomVisibilitySummary>,
    /// Recent tier of room stats history for sparkline rendering.
    pub stats_history: Option<Vec<crate::stats_history::RoomStatsSnapshot>>,
    /// Current-tick transfer queue snapshot for this room.
    pub transfer_stats: Option<crate::transfer::transfersystem::TransferRoomSnapshot>,
}

impl RoomVisualizationData {
    pub fn new() -> Self {
        Self::default()
    }
}

/// All visualization summary data for one tick.
/// Filled by gather systems, read by RenderSystem. Not serialized.
#[derive(Debug, Default)]
pub struct VisualizationData {
    pub operations: Vec<OperationSummary>,
    pub rooms: HashMap<RoomName, RoomVisualizationData>,
}

impl VisualizationData {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear and reuse for next tick (when visualization is on).
    pub fn clear(&mut self) {
        self.operations.clear();
        self.rooms.clear();
    }

    pub fn get_or_create_room(&mut self, room: RoomName) -> &mut RoomVisualizationData {
        self.rooms.entry(room).or_default()
    }
}

// ─── Aggregate summary system (reads only summary components + SpawnQueue) ───

#[derive(SystemData)]
pub struct AggregateSummarySystemData<'a> {
    visualization_data: Option<Write<'a, VisualizationData>>,
    entities: Entities<'a>,
    room_data: ReadStorage<'a, RoomData>,
    op_summary: ReadStorage<'a, OperationSummaryComponent>,
    mission_summary: ReadStorage<'a, MissionSummaryComponent>,
    job_summary: ReadStorage<'a, JobSummaryComponent>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    vis_summary: ReadStorage<'a, RoomVisibilitySummaryComponent>,
    spawn_queue: Read<'a, SpawnQueue>,
    stats_history: Option<Read<'a, crate::stats_history::StatsHistoryData>>,
    transfer_stats: Option<Read<'a, crate::transfer::transfersystem::TransferStatsSnapshot>>,
}

pub struct AggregateSummarySystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for AggregateSummarySystem {
    type SystemData = AggregateSummarySystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let Some(ref mut viz) = data.visualization_data else {
            return;
        };
        viz.clear();

        // Ensure every known room has an entry (so we draw room-specific UI when that room is viewed).
        for (_entity, room_data) in (&data.entities, &data.room_data).join() {
            viz.get_or_create_room(room_data.name);
        }

        // Operations (global list) — from OperationSummaryComponent
        for (_entity, op_sum) in (&data.entities, &data.op_summary).join() {
            viz.operations.push(OperationSummary {
                content: op_sum.content.clone(),
            });
        }

        // Spawn queue (per room) — from SpawnQueue resource
        for (room_entity, requests) in data.spawn_queue.iter_requests() {
            if let Some(room) = data.room_data.get(*room_entity) {
                let room_viz = viz.get_or_create_room(room.name);
                for req in requests.iter() {
                    room_viz.spawn_queue.push(SpawnQueueEntry {
                        priority: req.priority(),
                        cost: req.cost(),
                        description: req.description().to_string(),
                    });
                }
            }
        }

        // Missions (per room, from room's mission list) — from MissionSummaryComponent
        for (_entity, room_data) in (&data.entities, &data.room_data).join() {
            let room_viz = viz.get_or_create_room(room_data.name);
            for mission_entity in room_data.get_missions().iter() {
                if let Some(ms) = data.mission_summary.get(*mission_entity) {
                    room_viz.missions.push(MissionSummary {
                        content: ms.content.clone(),
                    });
                }
            }
        }

        // Jobs (per room, from creeps with job summaries; room from creep.room())
        for (_creep_entity, creep_owner, job_sum) in (&data.entities, &data.creep_owner, &data.job_summary).join() {
            if let Some(creep) = creep_owner.owner.resolve() {
                if let Some(room) = creep.room() {
                    let room_viz = viz.get_or_create_room(room.name());
                    room_viz.jobs.push(JobSummary {
                        creep_name: job_sum.creep_name.clone(),
                        content: job_sum.content.clone(),
                    });
                }
            }
        }

        // Room visibility (per room) — from RoomVisibilitySummaryComponent
        for (_entity, room_data, rv) in (&data.entities, &data.room_data, &data.vis_summary).join() {
            let room_viz = viz.get_or_create_room(room_data.name);
            room_viz.room_visibility = Some(RoomVisibilitySummary {
                visible: rv.visible,
                age: rv.age,
                owner: rv.owner.clone(),
                reservation: rv.reservation.clone(),
                source_keeper: rv.source_keeper.clone(),
                hostile_creeps: rv.hostile_creeps,
                hostile_structures: rv.hostile_structures,
            });
        }

        // Stats history (per room) — from StatsHistoryData resource (recent tier)
        if let Some(ref stats) = data.stats_history {
            for (room_name, room_history) in &stats.rooms {
                let room_viz = viz.get_or_create_room(*room_name);
                if !room_history.recent.is_empty() {
                    room_viz.stats_history = Some(room_history.recent.iter().cloned().collect());
                }
            }
        }

        // Transfer stats (per room) — from TransferStatsSnapshot resource
        if let Some(ref transfer_stats) = data.transfer_stats {
            for (room_name, room_snapshot) in &transfer_stats.rooms {
                let room_viz = viz.get_or_create_room(*room_name);
                room_viz.transfer_stats = Some(room_snapshot.clone());
            }
        }
    }
}

// ─── UI layout and theme ─────────────────────────────────────────────────────
//
// Global is drawn the same way as room drawing but is drawn in every room. Render order: global first, then per-room around it.
//
// Proposed layout (edges/corners to avoid game objects):
// - Right side (global): misc global state, global operations.
// - Left side (room): room state, missions, jobs, spawn queue (stacked).
// - Bottom strip (fixed size): CPU histogram (global), then room resources / transfer / trade (per-room, placeholders for now).

/// Approximate character width in room units (no text measurement API).
const CHAR_WIDTH: f32 = 0.48;
const LINE_HEIGHT: f32 = 1.05;
const PAD: f32 = 0.45;
const FONT_SIZE: f32 = 0.55;

/// Max characters per line and max lines per panel to keep layout bounded and prevent overlap.
const MAX_LINE_CHARS: usize = 22;
const MAX_PANEL_LINES: usize = 12;

/// Truncate content to max lines and max chars per line (Screeps doesn't reliably wrap or honour \n in one text call).
fn truncate_content(s: &str, max_lines: usize, max_chars: usize) -> String {
    let lines: Vec<String> = s
        .lines()
        .take(max_lines)
        .map(|l| {
            let len = l.chars().count();
            if len <= max_chars {
                l.to_string()
            } else {
                format!("{}…", l.chars().take(max_chars.saturating_sub(1)).collect::<String>())
            }
        })
        .collect();
    if lines.is_empty() {
        "—".to_string()
    } else {
        lines.join("\n")
    }
}

/// Single panel: rect + multiple text lines (one text primitive per line so newlines display correctly).
struct Panel {
    /// Content split into lines (we draw one text per line).
    lines: Vec<String>,
    x: f32,
    y: f32,
}

impl Panel {
    fn line_count(&self) -> usize {
        self.lines.len().max(1)
    }

    fn max_line_len(&self) -> usize {
        self.lines.iter().map(|l| l.chars().count()).max().unwrap_or(0).max(1)
    }

    fn width(&self) -> f32 {
        (self.max_line_len() as f32) * CHAR_WIDTH + 2.0 * PAD
    }

    fn height(&self) -> f32 {
        (self.line_count() as f32) * LINE_HEIGHT + 2.0 * PAD
    }

    fn from_content(content: &str, max_lines: usize, max_chars: usize) -> Self {
        let truncated = truncate_content(content, max_lines, max_chars);
        let lines: Vec<String> = truncated.lines().map(String::from).collect();
        Panel {
            lines: if lines.is_empty() { vec!["—".to_string()] } else { lines },
            x: 0.0,
            y: 0.0,
        }
    }
}

// ─── Theme (dark, high contrast, accent) ────────────────────────────────────
const COLOR_BG: &str = "#0d1117";
const COLOR_PANEL: &str = "#161b22";
const COLOR_PANEL_STROKE: &str = "#30363d";
const COLOR_ACCENT: &str = "#58a6ff";
const COLOR_HEADER: &str = "#c9d1d9";
const COLOR_BODY: &str = "#8b949e";
const COLOR_SEP: &str = "#21262d";
const COLOR_GRAPH_LINE: &str = "#58a6ff";
const COLOR_GRAPH_FILL: &str = "#1f6feb";
const COLOR_GRID: &str = "#30363d";

fn panel_rect_style() -> RectStyle {
    RectStyle::default()
        .fill(COLOR_PANEL)
        .opacity(0.96)
        .stroke(COLOR_PANEL_STROKE)
        .stroke_width(0.06)
}

fn panel_header_text_style() -> TextStyle {
    TextStyle::default()
        .color(COLOR_HEADER)
        .font(FONT_SIZE + 0.2)
        .align(TextAlign::Left)
        .stroke(COLOR_BG)
        .stroke_width(0.08)
}

fn panel_text_style() -> TextStyle {
    TextStyle::default()
        .color(COLOR_BODY)
        .font(FONT_SIZE)
        .align(TextAlign::Left)
        .stroke(COLOR_BG)
        .stroke_width(0.05)
}

fn separator_line_style() -> LineStyle {
    LineStyle::default().color(COLOR_SEP).width(0.06).opacity(0.9)
}

fn accent_line_style() -> LineStyle {
    LineStyle::default().color(COLOR_ACCENT).width(0.12).opacity(0.85)
}

fn grid_line_style() -> LineStyle {
    LineStyle::default()
        .color(COLOR_GRID)
        .width(0.04)
        .opacity(0.6)
        .line_style(LineDrawStyle::Dashed)
}

const GAP: f32 = 0.85;

// Right side (global): fixed position, same in every room.
const RIGHT_EDGE: f32 = 50.0;
const RIGHT_MARGIN: f32 = 2.0;
const TOP_Y: f32 = 2.0;

// Left side (room): stack Room, Missions, Jobs, Spawn. Cap width so panels hug the edge and leave center clear.
const LEFT_X: f32 = 2.0;
const LEFT_COLUMN_MAX_WIDTH: f32 = 11.0;

// Right side (global Ops): cap width so panel hugs the right edge.
const OPS_PANEL_MAX_WIDTH: f32 = 10.0;

// CPU histogram: top center so it doesn't overlap per-room panels (left/right columns).
const ROOM_CENTER_X: f32 = 25.0;
const GRAPH_W: f32 = 12.0;
const GRAPH_H: f32 = 4.0;
const CPU_GRAPH_LEFT: f32 = ROOM_CENTER_X - GRAPH_W / 2.0;
const CPU_GRAPH_TOP_Y: f32 = 2.0;

// Stats sparkline: center, directly below CPU histogram.
const STATS_SPARKLINE_TOP_Y: f32 = CPU_GRAPH_TOP_Y + GRAPH_H + GAP;

/// Global right column: one panel (misc + operations). Right-aligned, max width capped to hug the edge.
fn layout_global_right_panel(ops_content: &str) -> Panel {
    let ops_max_chars = (OPS_PANEL_MAX_WIDTH / CHAR_WIDTH - 2.0 * PAD / CHAR_WIDTH).floor().max(4.0) as usize;
    let mut p = Panel::from_content(ops_content, MAX_PANEL_LINES, ops_max_chars.min(MAX_LINE_CHARS));
    p.x = RIGHT_EDGE - RIGHT_MARGIN - p.width();
    p.y = TOP_Y;
    p
}

/// Fixed width for the entire left column so all panels align. Capped to hug the left edge and maximize center space.
fn left_column_width(right_column_left_x: f32) -> f32 {
    let available = (right_column_left_x - GAP - LEFT_X).max(6.0);
    available.min(LEFT_COLUMN_MAX_WIDTH)
}

fn left_column_max_chars(width: f32) -> usize {
    (width / CHAR_WIDTH - 2.0 * PAD / CHAR_WIDTH).floor().max(4.0) as usize
}

/// Left column (room): Room state, Missions, Jobs, Spawn stacked vertically. All panels use the same fixed width for alignment.
fn layout_room_left_panels(right_column_left_x: f32, room_content: Option<&str>, missions: &str, jobs: &str, spawn: &str) -> Vec<Panel> {
    let w = left_column_width(right_column_left_x);
    let max_chars = left_column_max_chars(w).min(MAX_LINE_CHARS);
    let mut panels = Vec::with_capacity(4);
    let mut y = TOP_Y;

    if let Some(rc) = room_content {
        let mut room = Panel::from_content(rc, MAX_PANEL_LINES, max_chars);
        room.x = LEFT_X;
        room.y = y;
        y += room.height() + GAP;
        panels.push(room);
    }

    let mut missions_panel = Panel::from_content(missions, MAX_PANEL_LINES, max_chars);
    missions_panel.x = LEFT_X;
    missions_panel.y = y;
    y += missions_panel.height() + GAP;
    panels.push(missions_panel);

    let mut jobs_panel = Panel::from_content(jobs, MAX_PANEL_LINES, max_chars);
    jobs_panel.x = LEFT_X;
    jobs_panel.y = y;
    y += jobs_panel.height() + GAP;
    panels.push(jobs_panel);

    let mut spawn_panel = Panel::from_content(spawn, MAX_PANEL_LINES, max_chars);
    spawn_panel.x = LEFT_X;
    spawn_panel.y = y;
    panels.push(spawn_panel);

    panels
}

/// Right column left edge so room layout knows where to stop.
fn right_column_left_x(ops_panel: &Panel) -> f32 {
    RIGHT_EDGE - RIGHT_MARGIN - ops_panel.width()
}

/// Draw global layer: right column (Ops) + top-center CPU histogram. Same in every room; draw to global() and to each room_vis.
fn draw_global_layer(
    vis: &mut crate::visualize::RoomVisualizer,
    ops_panel: &Panel,
    rect_style: &RectStyle,
    header_style: &TextStyle,
    text_style: &TextStyle,
    accent_style: &LineStyle,
    sep_style: &LineStyle,
    grid_style: &LineStyle,
    cpu_samples: Option<&[f32]>,
    cpu_limit: f32,
    tick: u32,
) {
    // Right column: Ops (accent on right edge, header + separator)
    let ow = ops_panel.width();
    let oh = ops_panel.height();
    vis.rect(ops_panel.x, ops_panel.y, ow, oh, Some(rect_style.clone()));
    vis.line(
        (ops_panel.x + ow, ops_panel.y),
        (ops_panel.x + ow, ops_panel.y + oh),
        Some(accent_style.clone()),
    );
    let ops_header_y = ops_panel.y + PAD + LINE_HEIGHT;
    vis.line(
        (ops_panel.x + PAD, ops_header_y),
        (ops_panel.x + ow - PAD, ops_header_y),
        Some(sep_style.clone()),
    );
    for (i, line) in ops_panel.lines.iter().enumerate() {
        let style = if i == 0 { header_style.clone() } else { text_style.clone() };
        vis.text(
            ops_panel.x + PAD,
            ops_panel.y + PAD + (i as f32) * LINE_HEIGHT,
            line.clone(),
            Some(style),
        );
    }

    // Top center: CPU histogram. Rect, tick, then axis/grid/fill/line/labels when we have samples.
    let graph_left = CPU_GRAPH_LEFT;
    let graph_top = CPU_GRAPH_TOP_Y;
    let inner_left = graph_left + PAD;
    let inner_right = graph_left + GRAPH_W - PAD;
    let inner_top = graph_top + PAD;
    let inner_bottom = graph_top + GRAPH_H - PAD;
    let x_range = (GRAPH_W - 2.0 * PAD).max(0.1);
    let y_range = (GRAPH_H - 2.0 * PAD).max(0.1);

    vis.rect(graph_left, graph_top, GRAPH_W, GRAPH_H, Some(rect_style.clone()));
    let center_align = text_style.clone().align(TextAlign::Center);
    vis.text(
        graph_left + GRAPH_W / 2.0,
        graph_top + PAD,
        format!("Tick: {}", tick),
        Some(center_align),
    );

    if let Some(samples) = cpu_samples {
        if !samples.is_empty() {
            let max_sample = samples.iter().cloned().fold(0.0_f32, f32::max);
            let y_max = if cpu_limit > 0.0 {
                cpu_limit.max(max_sample)
            } else {
                max_sample.max(1.0)
            };
            let n = samples.len();
            let y_scale = |v: f32| (v / y_max).min(1.0);

            let points: Vec<(f32, f32)> = if n >= 2 {
                samples
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| {
                        let x = inner_left + (i as f32 / (n - 1) as f32) * x_range;
                        let y = inner_bottom - y_scale(v) * y_range;
                        (x, y)
                    })
                    .collect()
            } else {
                let v = samples[0];
                let y = inner_bottom - y_scale(v) * y_range;
                vec![(inner_left, y), (inner_right, y)]
            };

            // Axis: left edge of graph
            vis.line((inner_left, inner_top), (inner_left, inner_bottom), Some(grid_style.clone()));
            // Grid: baseline (0) and top
            vis.line((inner_left, inner_bottom), (inner_right, inner_bottom), Some(grid_style.clone()));
            vis.line((inner_left, inner_top), (inner_right, inner_top), Some(grid_style.clone()));

            // Fill under the line (closed poly: points + bottom-right + bottom-left)
            let mut fill_points = points.clone();
            fill_points.push((inner_right, inner_bottom));
            fill_points.push((inner_left, inner_bottom));
            vis.poly(fill_points, Some(PolyStyle::default().fill(COLOR_GRAPH_FILL).opacity(0.4)));
            vis.poly(
                points,
                Some(PolyStyle::default().stroke(COLOR_GRAPH_LINE).stroke_width(0.22).opacity(0.95)),
            );

            let used = samples.last().copied().unwrap_or(0.0);
            let right_align = text_style.clone().align(TextAlign::Right);
            vis.text(inner_right, graph_top + PAD, format!("{:.1} CPU", used), Some(right_align));
            vis.text(graph_left, inner_top, format!("{:.0}", y_max), Some(text_style.clone()));
            vis.text(graph_left, inner_bottom, "0".to_string(), Some(text_style.clone()));
        }
    }
}

// ─── Stats sparkline ─────────────────────────────────────────────────────────

const STATS_SPARKLINE_H: f32 = 3.5;
const COLOR_ENERGY_FILL: &str = "#f0c040";
const COLOR_MINERALS_LINE: &str = "#58a6ff";

#[allow(clippy::too_many_arguments)]
fn draw_stats_sparkline(
    vis: &mut crate::visualize::RoomVisualizer,
    snapshots: &[crate::stats_history::RoomStatsSnapshot],
    x: f32,
    y: f32,
    width: f32,
    rect_style: &RectStyle,
    header_style: &TextStyle,
    text_style: &TextStyle,
    grid_style: &LineStyle,
) {
    let h = STATS_SPARKLINE_H;
    vis.rect(x, y, width, h, Some(rect_style.clone()));

    // Header
    vis.text(x + PAD, y + PAD, "Storage".to_string(), Some(header_style.clone()));

    let inner_left = x + PAD;
    let inner_right = x + width - PAD;
    let inner_top = y + PAD + LINE_HEIGHT;
    let inner_bottom = y + h - PAD;
    let x_range = (inner_right - inner_left).max(0.1);
    let y_range = (inner_bottom - inner_top).max(0.1);
    let n = snapshots.len();

    // Find max for scale
    let max_energy = snapshots.iter().map(|s| s.energy).max().unwrap_or(1).max(1);
    let max_minerals = snapshots.iter().map(|s| s.minerals_total).max().unwrap_or(0);
    let y_max = (max_energy.max(max_minerals)).max(1) as f32;

    // Grid lines
    vis.line((inner_left, inner_bottom), (inner_right, inner_bottom), Some(grid_style.clone()));
    vis.line((inner_left, inner_top), (inner_right, inner_top), Some(grid_style.clone()));

    let to_point = |i: usize, value: u32| -> (f32, f32) {
        let px = inner_left + (i as f32 / (n - 1) as f32) * x_range;
        let py = inner_bottom - (value as f32 / y_max).min(1.0) * y_range;
        (px, py)
    };

    // Energy fill (closed polygon)
    let energy_points: Vec<(f32, f32)> = snapshots.iter().enumerate().map(|(i, s)| to_point(i, s.energy)).collect();
    let mut energy_fill = energy_points.clone();
    energy_fill.push((inner_right, inner_bottom));
    energy_fill.push((inner_left, inner_bottom));
    vis.poly(energy_fill, Some(PolyStyle::default().fill(COLOR_ENERGY_FILL).opacity(0.35)));
    vis.poly(
        energy_points,
        Some(PolyStyle::default().stroke(COLOR_ENERGY_FILL).stroke_width(0.15).opacity(0.85)),
    );

    // Minerals line (if any non-zero)
    if max_minerals > 0 {
        let mineral_points: Vec<(f32, f32)> = snapshots.iter().enumerate().map(|(i, s)| to_point(i, s.minerals_total)).collect();
        vis.poly(
            mineral_points,
            Some(PolyStyle::default().stroke(COLOR_MINERALS_LINE).stroke_width(0.15).opacity(0.85)),
        );
    }

    // Labels
    let last = snapshots.last().unwrap();
    let right_align = text_style.clone().align(TextAlign::Right);
    vis.text(
        inner_right,
        y + PAD,
        format!("E:{} M:{}", last.energy, last.minerals_total),
        Some(right_align),
    );
}

// ─── Transfer stats panel ────────────────────────────────────────────────────

/// Row height sized for one text line (baseline y model: text drawn at baseline, extends upward by ~FONT_SIZE).
const TRANSFER_ROW_H: f32 = FONT_SIZE + 0.15;
/// Max chars for resource label — only used to truncate; actual label column is sized to fit the longest label in the row set.
const TRANSFER_LABEL_MAX_CHARS: usize = 6;
const TRANSFER_NUM_WIDTH: f32 = 4.0 * CHAR_WIDTH;
const ROOM_BOTTOM: f32 = 50.0;
/// Same width as left column panels for consistent layout.
const TRANSFER_PANEL_WIDTH: f32 = LEFT_COLUMN_MAX_WIDTH;
const TRANSFER_PANEL_LEFT: f32 = RIGHT_EDGE - RIGHT_MARGIN - TRANSFER_PANEL_WIDTH;
const TRANSFER_BOTTOM_MARGIN: f32 = 1.2;
const COLOR_SUPPLY: &str = "#3fb950";
const COLOR_SUPPLY_IN_PROGRESS: &str = "#238636";
const COLOR_DEMAND: &str = "#f85149";
const COLOR_DEMAND_IN_PROGRESS: &str = "#da3633";

/// Format a number compactly to save space: 0, 1k, 5.5k, 200k, 1.2M, etc.
fn compact_number(n: u32) -> String {
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        if m >= 100.0 {
            format!("{}M", m as u32)
        } else if m >= 10.0 {
            format!("{:.0}M", m)
        } else {
            format!("{:.1}M", m)
        }
    } else if n >= 1_000 {
        let k = n as f64 / 1_000.0;
        if k >= 100.0 {
            format!("{}k", k as u32)
        } else if k >= 10.0 {
            format!("{:.0}k", k)
        } else {
            format!("{:.1}k", k)
        }
    } else {
        format!("{}", n)
    }
}

/// Use the game API's canonical short name for the resource (e.g. "energy", "H", "UO", "UH2O").
fn resource_short_label(r: ResourceType) -> String {
    r.to_string()
}

fn truncate_transfer_label(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= TRANSFER_LABEL_MAX_CHARS {
        s.to_string()
    } else {
        chars.into_iter().take(TRANSFER_LABEL_MAX_CHARS).collect::<String>()
    }
}

/// Draw transfer panel: supply/demand bars back-to-back (supply left, demand right from center), with in-progress vs unfulfilled segments.
#[allow(clippy::too_many_arguments)]
fn draw_transfer_panel(
    vis: &mut crate::visualize::RoomVisualizer,
    snapshot: &crate::transfer::transfersystem::TransferRoomSnapshot,
    x: f32,
    width: f32,
    room_bottom: f32,
    rect_style: &RectStyle,
    header_style: &TextStyle,
    text_style: &TextStyle,
) -> f32 {
    // Build rows with pending: (label, supply, supply_pending, demand, demand_pending).
    let mut rows: Vec<(String, u32, u32, u32, u32)> = Vec::new();
    for (resource, stats) in &snapshot.resources {
        if stats.supply > 0 || stats.demand > 0 {
            let label = truncate_transfer_label(&resource_short_label(*resource));
            rows.push((label, stats.supply, stats.supply_pending, stats.demand, stats.demand_pending));
        }
    }
    if snapshot.generic_demand > 0 {
        rows.push((
            "Any".to_string(),
            0,
            0,
            snapshot.generic_demand,
            snapshot.generic_demand_pending,
        ));
    }

    // Stable sort by label so resource order does not jump each tick. "Any" last.
    rows.sort_by(|a, b| {
        let a_any = a.0 == "Any";
        let b_any = b.0 == "Any";
        match (a_any, b_any) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) => a.0.cmp(&b.0),
        }
    });

    if rows.is_empty() {
        return 0.0;
    }

    let max_val = rows
        .iter()
        .map(|(_, s, _, d, _)| (*s).max(*d))
        .max()
        .unwrap_or(1)
        .max(1) as f32;

    // Dynamic label width: fit the longest label in the current set (no wasted space for short labels).
    let max_label_chars = rows.iter().map(|(l, ..)| l.chars().count()).max().unwrap_or(1) as f32;
    let label_width = max_label_chars * CHAR_WIDTH + PAD * 0.5;

    let inner_width = (width - 2.0 * PAD).max(1.0);
    // Layout: [label][supply num][bar region: supply←|center|demand→][demand num].
    let bar_region_width = (inner_width - label_width - 2.0 * TRANSFER_NUM_WIDTH - PAD).max(0.5);
    let half_bar = bar_region_width * 0.5;
    let supply_num_x = x + PAD + label_width;
    let bar_region_left = supply_num_x + TRANSFER_NUM_WIDTH + PAD * 0.25;
    let center_x = bar_region_left + half_bar;
    let demand_num_x = bar_region_left + bar_region_width;

    let header_h = PAD + FONT_SIZE + PAD * 0.5;
    let total_h = header_h + (rows.len() as f32) * TRANSFER_ROW_H;
    let y = room_bottom - TRANSFER_BOTTOM_MARGIN - total_h;

    vis.rect(x, y, width, total_h, Some(rect_style.clone()));

    // Header baseline: positioned so text sits inside header area.
    vis.text(x + PAD, y + PAD + FONT_SIZE * 0.8, "Transfer".to_string(), Some(header_style.clone()));

    let mut row_y = y + header_h;
    let label_x = x + PAD;
    let supply_style = text_style.clone().color(COLOR_SUPPLY);
    let demand_style = text_style.clone().color(COLOR_DEMAND);
    let bar_h = (TRANSFER_ROW_H * 0.55).max(0.15);

    for (label, supply, supply_pending, demand, demand_pending) in rows {
        // Screeps text y = baseline. Text extends upward by ~FONT_SIZE. Place baseline near row bottom so text fills the row.
        let text_y = row_y + TRANSFER_ROW_H - 0.05;
        // Bar center = visual center of text ≈ baseline - FONT_SIZE * 0.35.
        let bar_center_y = text_y - FONT_SIZE * 0.35;
        vis.text(label_x, text_y, label.clone(), Some(text_style.clone()));

        // Supply grows left from center. supply_available = supply - supply_pending (unfulfilled), supply_pending = in progress.
        let supply_len = (supply as f32 / max_val).min(1.0) * half_bar;
        let supply_pending_len = (supply_pending as f32 / max_val).min(1.0) * half_bar;
        let supply_available_len = (supply_len - supply_pending_len).max(0.0);

        if supply_len > 0.02 {
            // Segment toward left: available (unfulfilled supply)
            if supply_available_len > 0.02 {
                vis.rect(
                    center_x - supply_len,
                    bar_center_y - bar_h * 0.5,
                    supply_available_len,
                    bar_h,
                    Some(RectStyle::default().fill(COLOR_SUPPLY).opacity(0.9)),
                );
            }
            // Segment next to center: in progress (pickups reserved)
            if supply_pending_len > 0.02 {
                vis.rect(
                    center_x - supply_len + supply_available_len,
                    bar_center_y - bar_h * 0.5,
                    supply_pending_len,
                    bar_h,
                    Some(RectStyle::default().fill(COLOR_SUPPLY_IN_PROGRESS).opacity(0.9)),
                );
            }
        }
        vis.text(
            supply_num_x + TRANSFER_NUM_WIDTH,
            text_y,
            compact_number(supply),
            Some(supply_style.clone().align(TextAlign::Right)),
        );

        // Demand grows right from center. demand_pending = in progress, demand_available = demand - demand_pending (unfulfilled).
        let demand_len = (demand as f32 / max_val).min(1.0) * half_bar;
        let demand_pending_len = (demand_pending as f32 / max_val).min(1.0) * half_bar;
        let demand_available_len = (demand_len - demand_pending_len).max(0.0);

        if demand_len > 0.02 {
            // Segment next to center: in progress (deliveries reserved)
            if demand_pending_len > 0.02 {
                vis.rect(
                    center_x,
                    bar_center_y - bar_h * 0.5,
                    demand_pending_len,
                    bar_h,
                    Some(RectStyle::default().fill(COLOR_DEMAND_IN_PROGRESS).opacity(0.9)),
                );
            }
            // Segment toward right: unfulfilled demand
            if demand_available_len > 0.02 {
                vis.rect(
                    center_x + demand_pending_len,
                    bar_center_y - bar_h * 0.5,
                    demand_available_len,
                    bar_h,
                    Some(RectStyle::default().fill(COLOR_DEMAND).opacity(0.9)),
                );
            }
        }
        vis.text(
            demand_num_x,
            text_y,
            compact_number(demand),
            Some(demand_style.clone()),
        );

        row_y += TRANSFER_ROW_H;
    }

    total_h
}

// ─── Render system ───────────────────────────────────────────────────────────

#[derive(SystemData)]
pub struct RenderSystemData<'a> {
    visualization_data: Option<Read<'a, VisualizationData>>,
    visualizer: Option<Write<'a, Visualizer>>,
    cpu_history: Option<Read<'a, CpuHistory>>,
}

pub struct RenderSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RenderSystem {
    type SystemData = RenderSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let (Some(viz), Some(visualizer)) = (data.visualization_data.as_deref(), data.visualizer.as_deref_mut()) else {
            return;
        };

        let rect_style = panel_rect_style();
        let header_text_style = panel_header_text_style();
        let text_style = panel_text_style();
        let sep_style = separator_line_style();
        let accent_style = accent_line_style();

        // Global: operations (no CPU line; CPU shown as histogram below)
        let ops_lines: Vec<String> = viz.operations.iter().flat_map(|op| op.content.to_lines()).collect();
        // Tick and CPU limit come from CpuHistory (set by CpuTrackingSystem), not game API.
        let tick = data.cpu_history.as_deref().map_or(0, |h| h.tick);
        let cpu_limit_f32 = data.cpu_history.as_deref().map_or(0.0, |h| h.tick_limit);
        let ops_content = if ops_lines.is_empty() {
            "Operations".to_string()
        } else {
            format!("Operations\n{}", ops_lines.join("\n"))
        };
        let global_ops_panel = layout_global_right_panel(&ops_content);
        let right_column_left_x = right_column_left_x(&global_ops_panel);
        let cpu_samples = data.cpu_history.as_deref().map(|h| h.samples.as_slice());

        let grid_style = grid_line_style();
        {
            let global = visualizer.global();
            draw_global_layer(
                global,
                &global_ops_panel,
                &rect_style,
                &header_text_style,
                &text_style,
                &accent_style,
                &sep_style,
                &grid_style,
                cpu_samples,
                cpu_limit_f32,
                tick,
            );
        }

        // Per-room: draw room layer first (left stack), then global layer (right Ops + bottom CPU) so the histogram is on top and visible.
        for (room_name, room_viz) in &viz.rooms {
            let room_vis = visualizer.get_room(*room_name);

            let room_content = room_viz.room_visibility.as_ref().map(|rv| {
                format!(
                    "Room\nVisible: {}\nAge: {}\nOwner: {}\nReservation: {}\nSource Keeper: {}\nHostile creeps: {}\nHostile structs: {}",
                    rv.visible, rv.age, rv.owner, rv.reservation, rv.source_keeper, rv.hostile_creeps, rv.hostile_structures
                )
            });

            let missions_content = if room_viz.missions.is_empty() {
                "Missions".to_string()
            } else {
                let lines: Vec<String> = room_viz.missions.iter().flat_map(|m| m.content.to_lines()).collect();
                format!("Missions\n{}", lines.join("\n"))
            };

            let jobs_content = if room_viz.jobs.is_empty() {
                "Jobs".to_string()
            } else {
                let lines: Vec<String> = room_viz
                    .jobs
                    .iter()
                    .map(|j| {
                        let desc = match &j.content {
                            SummaryContent::Text(s) => s.clone(),
                            other => other.to_lines().join(" / "),
                        };
                        format!("{} — {}", j.creep_name, desc)
                    })
                    .collect();
                format!("Jobs\n{}", lines.join("\n"))
            };

            let spawn_content = if room_viz.spawn_queue.is_empty() {
                "Spawn".to_string()
            } else {
                let lines: Vec<String> = room_viz
                    .spawn_queue
                    .iter()
                    .map(|s| format!("{:.0} · {} · {}", s.priority, s.cost, s.description))
                    .collect();
                format!("Spawn\n{}", lines.join("\n"))
            };

            let panels = layout_room_left_panels(
                right_column_left_x,
                room_content.as_deref(),
                &missions_content,
                &jobs_content,
                &spawn_content,
            );
            let left_w = left_column_width(right_column_left_x);

            for panel in &panels {
                let h = panel.height();
                room_vis.rect(panel.x, panel.y, left_w, h, Some(rect_style.clone()));
                room_vis.line((panel.x, panel.y), (panel.x, panel.y + h), Some(accent_style.clone()));
                let header_y = panel.y + PAD + LINE_HEIGHT;
                room_vis.line(
                    (panel.x + PAD, header_y),
                    (panel.x + left_w - PAD, header_y),
                    Some(sep_style.clone()),
                );
                for (i, line) in panel.lines.iter().enumerate() {
                    let style = if i == 0 { header_text_style.clone() } else { text_style.clone() };
                    room_vis.text(panel.x + PAD, panel.y + PAD + (i as f32) * LINE_HEIGHT, line.clone(), Some(style));
                }
            }

            // Stats history sparkline (center, below CPU histogram — fixed position per room)
            if let Some(ref snapshots) = room_viz.stats_history {
                if snapshots.len() >= 2 {
                    draw_stats_sparkline(
                        room_vis,
                        snapshots,
                        CPU_GRAPH_LEFT,
                        STATS_SPARKLINE_TOP_Y,
                        GRAPH_W,
                        &rect_style,
                        &header_text_style,
                        &text_style,
                        &grid_style,
                    );
                }
            }

            // Transfer stats panel (bottom of room, thin horizontal layout)
            if let Some(ref transfer_snapshot) = room_viz.transfer_stats {
                let has_data =
                    transfer_snapshot.resources.values().any(|s| s.supply > 0 || s.demand > 0) || transfer_snapshot.generic_demand > 0;
                if has_data {
                    let _h = draw_transfer_panel(
                        room_vis,
                        transfer_snapshot,
                        TRANSFER_PANEL_LEFT,
                        TRANSFER_PANEL_WIDTH,
                        ROOM_BOTTOM,
                        &rect_style,
                        &header_text_style,
                        &text_style,
                    );
                }
            }

            draw_global_layer(
                room_vis,
                &global_ops_panel,
                &rect_style,
                &header_text_style,
                &text_style,
                &accent_style,
                &sep_style,
                &grid_style,
                cpu_samples,
                cpu_limit_f32,
                tick,
            );
        }
    }
}
