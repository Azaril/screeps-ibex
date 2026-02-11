#!/usr/bin/env node
/**
 * Generate the Grafana dashboard JSON for screeps-ibex.
 *
 * Layout:
 *   Row 1: Global Overview (GCL, GPL, Credits, Global Energy, Global Resources)
 *   Row 2: Per-Shard (repeated by $shard) -- CPU, Bucket, Ticks
 *   Row 3: Per-Room (repeated by $room)  -- RCL, Upgrade, Energy, Resources
 */

const fs = require("fs");

const DS = { type: "graphite", uid: "afcvzx524n75sb" };
const P = "data.screeps_com.$user.shard.$shard"; // base path

let nextId = 1;
function id() { return nextId++; }

// ─── Panel builders ──────────────────────────────────────────────────────────

function statPanel({ title, gridPos, targets, maxDataPoints }) {
  return {
    datasource: { ...DS },
    fieldConfig: { defaults: {}, overrides: [] },
    gridPos,
    id: id(),
    maxDataPoints: maxDataPoints || 100,
    options: {},
    targets: targets.map(t => ({ ...t, datasource: { ...DS } })),
    title,
    type: "stat",
  };
}

function timeseriesPanel({ title, gridPos, targets }) {
  return {
    datasource: { ...DS },
    fieldConfig: { defaults: {}, overrides: [] },
    gridPos,
    id: id(),
    options: { legend: { calcs: [], displayMode: "list", placement: "bottom", showLegend: true }, tooltip: { mode: "single", sort: "none" } },
    targets: targets.map(t => ({ ...t, datasource: { ...DS } })),
    title,
    type: "timeseries",
  };
}

function row({ title, y, repeat, collapsed }) {
  const r = {
    collapsed: collapsed || false,
    gridPos: { h: 1, w: 24, x: 0, y },
    id: id(),
    panels: [],
    title,
    type: "row",
  };
  if (repeat) r.repeat = repeat;
  return r;
}

// ─── Helpers for Graphite targets ────────────────────────────────────────────

function t(refId, target, opts) {
  return { refId, target, ...opts };
}

// ─── Build panels ────────────────────────────────────────────────────────────

const panels = [];
let y = 0;

// ═══════════════════════════════════════════════════════════════════════════════
// ROW: Global Overview
// ═══════════════════════════════════════════════════════════════════════════════
panels.push(row({ title: "Global Overview", y: y++ }));

// --- GCL row: Level | Progress | Time to Next | Upgrade Rate ---
panels.push(statPanel({
  title: "GCL Level",
  gridPos: { h: 3, w: 3, x: 0, y },
  targets: [t("A", `${P}.gcl.level`)],
}));

panels.push(statPanel({
  title: "GCL Progress",
  gridPos: { h: 3, w: 3, x: 3, y },
  targets: [
    t("A", `divideSeries(${P}.gcl.progress, #B)`),
    t("B", `${P}.gcl.progress_total`, { hide: true }),
  ],
}));

panels.push(statPanel({
  title: "Time to Next GCL",
  gridPos: { h: 3, w: 3, x: 6, y },
  targets: [
    t("A", `divideSeries(diffSeries(keepLastValue(${P}.gcl.progress_total, 100), #B), #C)`,
      { targetFull: `divideSeries(diffSeries(keepLastValue(${P}.gcl.progress_total, 100), keepLastValue(${P}.gcl.progress, 100)), scaleToSeconds(movingAverage(nonNegativeDerivative(keepLastValue(${P}.gcl.progress, 100)), '1hour'), 1))` }),
    t("B", `keepLastValue(${P}.gcl.progress, 100)`, { hide: true }),
    t("C", `scaleToSeconds(movingAverage(nonNegativeDerivative(keepLastValue(${P}.gcl.progress, 100)), '1hour'), 1)`, { hide: true }),
  ],
}));

panels.push(timeseriesPanel({
  title: "GCL Upgrade Rate",
  gridPos: { h: 7, w: 6, x: 9, y },
  targets: [
    t("A", `alias(divideSeries(nonNegativeDerivative(keepLastValue(${P}.gcl.progress, 100)), #B), 'GCL per tick')`,
      { targetFull: `alias(divideSeries(nonNegativeDerivative(keepLastValue(${P}.gcl.progress, 100)), derivative(${P}.time)), 'GCL per tick')` }),
    t("B", `derivative(${P}.time)`, { hide: true }),
    t("C", `alias(movingAverage(divideSeries(nonNegativeDerivative(keepLastValue(${P}.gcl.progress, 100)), #B), '1hour'), '1hr Moving Average')`,
      { targetFull: `alias(movingAverage(divideSeries(nonNegativeDerivative(keepLastValue(${P}.gcl.progress, 100)), derivative(${P}.time)), '1hour'), '1hr Moving Average')` }),
  ],
}));

// --- GPL: Level | Progress ---
panels.push(statPanel({
  title: "GPL Level",
  gridPos: { h: 3, w: 3, x: 15, y },
  targets: [t("A", `${P}.gpl.level`)],
}));

panels.push(statPanel({
  title: "GPL Progress",
  gridPos: { h: 3, w: 3, x: 18, y },
  targets: [
    t("A", `divideSeries(${P}.gpl.progress, #B)`),
    t("B", `${P}.gpl.progress_total`, { hide: true }),
  ],
}));

// --- Credits: Current | Rate ---
panels.push(statPanel({
  title: "Credits",
  gridPos: { h: 3, w: 3, x: 21, y },
  targets: [t("A", `alias(keepLastValue(${P}.market.credits, 100), 'Credits')`)],
}));

y += 3; // after the 3h stat row

// GCL progress gauge row is done (timeseries at x:9 extends to y+7)

panels.push(timeseriesPanel({
  title: "Credits Rate",
  gridPos: { h: 4, w: 9, x: 15, y },
  targets: [
    t("A", `alias(divideSeries(derivative(keepLastValue(${P}.market.credits, 100)), #B), 'Credits per tick')`,
      { targetFull: `alias(divideSeries(derivative(keepLastValue(${P}.market.credits, 100)), derivative(${P}.time)), 'Credits per tick')` }),
    t("B", `derivative(${P}.time)`, { hide: true }),
    t("C", `alias(movingAverage(divideSeries(derivative(keepLastValue(${P}.market.credits, 100)), #B), '1hour'), '1hr Moving Average')`,
      { targetFull: `alias(movingAverage(divideSeries(derivative(keepLastValue(${P}.market.credits, 100)), derivative(${P}.time)), '1hour'), '1hr Moving Average')` }),
  ],
}));

y += 4;

// --- Global Energy & Resources ---
panels.push(timeseriesPanel({
  title: "Global Stored Energy",
  gridPos: { h: 7, w: 8, x: 0, y },
  targets: [
    t("B", `groupByNode(${P}.room.*.storage.*.energy, 7, "sumSeries")`, { hide: false }),
    t("A", `alias(sumSeries(#B), 'Total')`,
      { targetFull: `alias(sumSeries(groupByNode(${P}.room.*.storage.*.energy, 7, "sumSeries")), 'Total')` }),
  ],
}));

panels.push(timeseriesPanel({
  title: "Global Resources",
  gridPos: { h: 7, w: 8, x: 8, y },
  targets: [
    t("B", `groupByNode(${P}.room.*.storage.*.*, 8, "sumSeries")`, { hide: false }),
  ],
}));

panels.push(timeseriesPanel({
  title: "Ticks Per Stats Update",
  gridPos: { h: 7, w: 8, x: 16, y },
  targets: [
    t("A", `alias(derivative(${P}.time), 'Ticks Per Update')`),
  ],
}));

y += 7;

// ═══════════════════════════════════════════════════════════════════════════════
// ROW: Per-Shard (repeated by $shard)
// ═══════════════════════════════════════════════════════════════════════════════
panels.push(row({ title: "Shard: $shard", y: y++, repeat: "shard" }));

panels.push(timeseriesPanel({
  title: "CPU",
  gridPos: { h: 7, w: 12, x: 0, y },
  targets: [
    t("A", `alias(${P}.cpu.used, 'Used')`, { hide: false }),
    t("D", `alias(movingAverage(${P}.cpu.used, '10min'), 'Average Used')`),
    t("B", `alias(${P}.cpu.bucket, 'Bucket')`),
    t("C", `alias(${P}.cpu.limit, 'Limit')`, { hide: false }),
  ],
}));

panels.push(statPanel({
  title: "CPU Bucket",
  gridPos: { h: 7, w: 4, x: 12, y },
  targets: [t("A", `${P}.cpu.bucket`)],
}));

panels.push(statPanel({
  title: "Total Creeps",
  gridPos: { h: 7, w: 4, x: 16, y },
  targets: [t("A", `alias(countSeries(${P}.room.*), 'Rooms')`)],
}));

panels.push(statPanel({
  title: "Game Time",
  gridPos: { h: 7, w: 4, x: 20, y },
  targets: [t("A", `${P}.time`)],
}));

y += 7;

// ═══════════════════════════════════════════════════════════════════════════════
// ROW: Per-Room (repeated by $room)
// ═══════════════════════════════════════════════════════════════════════════════
panels.push(row({ title: "$room", y: y++, repeat: "room" }));

const RP = `${P}.room.$room`; // room path

panels.push(statPanel({
  title: "RCL",
  gridPos: { h: 3, w: 2, x: 0, y },
  targets: [t("A", `${RP}.controller_level`)],
}));

panels.push(statPanel({
  title: "RCL Progress",
  gridPos: { h: 3, w: 3, x: 2, y },
  targets: [
    t("A", `divideSeries(${RP}.controller_progress, #B)`),
    t("B", `${RP}.controller_progress_total`, { hide: true }),
  ],
}));

panels.push(statPanel({
  title: "Time to Next RCL",
  gridPos: { h: 3, w: 4, x: 5, y },
  targets: [
    t("A", `divideSeries(diffSeries(${RP}.controller_progress_total, #B), #C)`,
      { targetFull: `divideSeries(diffSeries(${RP}.controller_progress_total, keepLastValue(${RP}.controller_progress, 100)), scaleToSeconds(movingAverage(nonNegativeDerivative(keepLastValue(${RP}.controller_progress, 100)), '1hour'), 1))` }),
    t("B", `keepLastValue(${RP}.controller_progress, 100)`, { hide: true }),
    t("C", `scaleToSeconds(movingAverage(nonNegativeDerivative(keepLastValue(${RP}.controller_progress, 100)), '1hour'), 1)`, { hide: true }),
  ],
}));

// RCL Upgrade Rate (timeseries, spans both stat rows)
panels.push(timeseriesPanel({
  title: "RCL Upgrade Rate",
  gridPos: { h: 7, w: 6, x: 9, y },
  targets: [
    t("A", `alias(divideSeries(nonNegativeDerivative(keepLastValue(${RP}.controller_progress, 100)), #C), 'RCL per tick')`,
      { targetFull: `alias(divideSeries(nonNegativeDerivative(keepLastValue(${RP}.controller_progress, 100)), derivative(${P}.time)), 'RCL per tick')` }),
    t("C", `derivative(${P}.time)`, { hide: true }),
    t("B", `alias(movingAverage(divideSeries(nonNegativeDerivative(keepLastValue(${RP}.controller_progress, 100)), #C), '1hour'), '1hr Moving Average')`,
      { targetFull: `alias(movingAverage(divideSeries(nonNegativeDerivative(keepLastValue(${RP}.controller_progress, 100)), derivative(${P}.time)), '1hour'), '1hr Moving Average')` }),
  ],
}));

// Room Stored Energy
panels.push(timeseriesPanel({
  title: "Room Stored Energy",
  gridPos: { h: 7, w: 5, x: 15, y },
  targets: [
    t("A", `groupByNode(${RP}.storage.*.energy, 7, "sumSeries")`),
    t("D", `alias(sumSeries(#A), "Total")`,
      { targetFull: `alias(sumSeries(groupByNode(${RP}.storage.*.energy, 7, "sumSeries")), "Total")` }),
  ],
}));

// Room Resources
panels.push(timeseriesPanel({
  title: "Room Resources",
  gridPos: { h: 7, w: 4, x: 20, y },
  targets: [
    t("A", `groupByNode(${RP}.storage.*.*, 8, "sumSeries")`),
  ],
}));

y += 3; // after the 3h stat row, room energy/spawn stats

panels.push(statPanel({
  title: "Energy Available",
  gridPos: { h: 4, w: 5, x: 0, y },
  targets: [
    t("A", `alias(${RP}.energy_available, 'Available')`),
    t("B", `alias(${RP}.energy_capacity_available, 'Capacity')`),
  ],
}));

// Placeholder for future creep count stat
panels.push(statPanel({
  title: "Energy Fill",
  gridPos: { h: 4, w: 4, x: 5, y },
  targets: [
    t("A", `divideSeries(${RP}.energy_available, #B)`),
    t("B", `${RP}.energy_capacity_available`, { hide: true }),
  ],
}));

y += 4;

// ─── Assemble dashboard ──────────────────────────────────────────────────────

const dashboard = {
  annotations: {
    list: [{
      builtIn: 1,
      datasource: { type: "grafana", uid: "-- Grafana --" },
      enable: true,
      hide: true,
      iconColor: "rgba(0, 211, 255, 1)",
      name: "Annotations & Alerts",
      type: "dashboard",
    }],
  },
  editable: true,
  fiscalYearStartMonth: 0,
  graphTooltip: 0,
  id: null,
  links: [],
  panels,
  preload: false,
  refresh: "30s",
  schemaVersion: 41,
  tags: ["screeps"],
  templating: {
    list: [
      {
        current: { text: "All", value: "$__all" },
        datasource: "graphite",
        definition: "data.screeps_com.*",
        includeAll: true,
        name: "user",
        options: [],
        query: "data.screeps_com.*",
        refresh: 1,
        regex: "",
        type: "query",
      },
      {
        current: { text: "All", value: "$__all" },
        datasource: "graphite",
        definition: "data.screeps_com.$user.shard.*",
        includeAll: true,
        name: "shard",
        options: [],
        query: "data.screeps_com.$user.shard.*",
        refresh: 1,
        regex: "",
        type: "query",
      },
      {
        current: { text: "All", value: "$__all" },
        datasource: "graphite",
        definition: "data.screeps_com.$user.shard.$shard.room.*",
        includeAll: true,
        name: "room",
        options: [],
        query: "data.screeps_com.$user.shard.$shard.room.*",
        refresh: 1,
        regex: "",
        type: "query",
      },
    ],
  },
  time: { from: "now-6h", to: "now" },
  timepicker: {},
  timezone: "browser",
  title: "Screeps Overview",
  uid: "screeps-overview-v2",
  version: 1,
};

const outFile = process.argv[2] || "migrated-dashboard.json";
fs.writeFileSync(outFile, JSON.stringify(dashboard, null, 2) + "\n");

console.log(`Generated ${outFile}`);
console.log(`  Panels: ${panels.length}`);
const types = {};
for (const p of panels) types[p.type] = (types[p.type] || 0) + 1;
console.log(`  Types:`, types);
