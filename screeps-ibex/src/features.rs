use std::cell::RefCell;

use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;

// ─── Reset flags (separate from feature flags) ────────────────────────────────
//
// Reset flags are one-shot triggers read *before* the feature cache is
// populated, so they have their own lightweight path that reads directly from
// `Memory._features.reset` without caching.

/// Read the one-shot reset flags directly from `Memory._features.reset`.
///
/// This is intentionally cheap (two JS reads) and independent of the feature
/// flag cache, because resets may destroy/recreate the environment before the
/// main features are loaded.
pub fn load_reset() -> ResetFlags {
    let root = crate::memory_helper::root();
    let features = js_get(&root, "_features");
    let reset = js_get(&features, "reset");
    ResetFlags {
        environment: js_bool(&reset, "environment"),
        memory: js_bool(&reset, "memory"),
    }
}

/// Clear reset flags in Memory so they don't fire again.
pub fn clear_reset() {
    crate::memory_helper::path_set("_features.reset.environment", false);
    crate::memory_helper::path_set("_features.reset.memory", false);
}

#[derive(Debug, Clone, Copy)]
pub struct ResetFlags {
    pub environment: bool,
    pub memory: bool,
}

// ─── Feature flag structs ──────────────────────────────────────────────────────
//
// Every struct derives `Serialize + Deserialize` with `#[serde(default)]` so
// that `serde_wasm_bindgen` can round-trip them to/from the JS Memory tree.
// Missing keys in Memory automatically fall back to the struct's `Default` impl
// — this is the primary mechanism for providing safe defaults when the Memory
// schema changes across ticks.

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualizeFeatures {
    pub on: bool,
}

impl Default for VisualizeFeatures {
    fn default() -> Self {
        Self { on: true }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct ConstructionVisualizeFeatures {
    pub on: bool,
    pub planner: bool,
    pub planner_best: bool,
    pub plan: bool,
}


impl ConstructionVisualizeFeatures {
    /// Returns `planner && on`.
    pub fn planner(&self) -> bool {
        self.planner && self.on
    }

    /// Returns `planner_best && on`.
    pub fn planner_best(&self) -> bool {
        self.planner_best && self.on
    }

    /// Returns `plan && on`.
    pub fn plan(&self) -> bool {
        self.plan && self.on
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct ConstructionFeatures {
    pub plan: bool,
    pub force_plan: bool,
    pub allow_replan: bool,
    pub execute: bool,
    pub cleanup: bool,
    pub visualize: ConstructionVisualizeFeatures,
}

impl Default for ConstructionFeatures {
    fn default() -> Self {
        Self {
            plan: true,
            force_plan: false,
            allow_replan: false,
            execute: true,
            cleanup: true,
            visualize: ConstructionVisualizeFeatures::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct MarketFeatures {
    pub buy: bool,
    pub sell: bool,
    pub credit_reserve: f64,
    pub buy_energy: bool,
    pub buy_minerals: bool,
}

impl Default for MarketFeatures {
    fn default() -> Self {
        Self {
            buy: false,
            sell: false,
            credit_reserve: 10_000_000.0,
            buy_energy: false,
            buy_minerals: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct TransferVisualizeFeatures {
    pub on: bool,
    pub haul: bool,
    pub demand: bool,
    pub orders: bool,
}


impl TransferVisualizeFeatures {
    pub fn haul(&self) -> bool {
        self.haul && self.on
    }

    pub fn demand(&self) -> bool {
        self.demand && self.on
    }

    pub fn orders(&self) -> bool {
        self.orders && self.on
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct TransferFeatures {
    pub visualize: TransferVisualizeFeatures,
}


#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteMineFeatures {
    pub harvest: bool,
    pub reserve: bool,
}

impl Default for RemoteMineFeatures {
    fn default() -> Self {
        Self {
            harvest: true,
            reserve: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct PathingFeatures {
    pub visualize_on: bool,
    pub custom: bool,
}

impl Default for PathingFeatures {
    fn default() -> Self {
        Self {
            visualize_on: false,
            custom: true,
        }
    }
}

impl PathingFeatures {
    /// Returns `visualize_on && global visualize.on`.
    pub fn visualize(&self, global_visualize: bool) -> bool {
        self.visualize_on && global_visualize
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct RoomFeatures {
    pub visualize_on: bool,
}

impl Default for RoomFeatures {
    fn default() -> Self {
        Self {
            visualize_on: true,
        }
    }
}

impl RoomFeatures {
    /// Returns `visualize_on && global visualize.on`.
    pub fn visualize(&self, global_visualize: bool) -> bool {
        self.visualize_on && global_visualize
    }
}

// ─── Top-level features ────────────────────────────────────────────────────────

/// All feature flags, loaded once per tick from `Memory._features`.
///
/// Reset flags are intentionally excluded — they live in a separate path
/// ([`load_reset`] / [`clear_reset`]) because they are consumed before the
/// feature cache is populated.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Features {
    pub visualize: VisualizeFeatures,
    pub construction: ConstructionFeatures,
    pub market: MarketFeatures,
    pub transfer: TransferFeatures,
    pub remote_mine: RemoteMineFeatures,
    pub pathing: PathingFeatures,
    pub room: RoomFeatures,
    pub raid: bool,
    pub claim: bool,
    pub dismantle: bool,
}


// ─── Thread-local cache ────────────────────────────────────────────────────────

thread_local! {
    static CACHED: RefCell<Features> = RefCell::new(Features::default());
}

/// Return a copy of the cached feature flags for the current tick.
pub fn features() -> Features {
    CACHED.with(|c| *c.borrow())
}

// ─── JS helpers (private) ──────────────────────────────────────────────────────

#[inline]
fn js_get(parent: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(parent, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

#[inline]
fn js_bool(parent: &JsValue, key: &str) -> bool {
    js_get(parent, key).as_bool().unwrap_or(false)
}

/// Deserialize the `_features` object from Memory, falling back to defaults for
/// any missing or malformed values.
fn features_from_memory() -> Features {
    let root = crate::memory_helper::root();
    let js_features = js_get(&root, "_features");

    if js_features.is_undefined() || js_features.is_null() {
        Features::default()
    } else {
        serde_wasm_bindgen::from_value(js_features).unwrap_or_default()
    }
}

// ─── Prepare / Load ────────────────────────────────────────────────────────────

/// Ensure `Memory._features` exists with sensible defaults.
///
/// Reads the existing `_features` tree (deserializing with defaults for any
/// missing keys), then serializes the complete struct back to Memory. This
/// means any keys that were absent are now visible in the console with their
/// default values, while existing operator overrides are preserved.
///
/// Called once during environment creation.  The same read-merge-write logic
/// also runs every tick inside [`load`], so user edits to Memory between ticks
/// are always picked up.
pub fn prepare() {
    load();
}

/// Load all feature flags from `Memory._features` into the per-tick cache.
///
/// Must be called once at the start of every game tick, after resets have been
/// handled.  The resolved flags (with defaults filled in for any missing keys)
/// are written back to Memory so the user can always see and modify the
/// complete set of feature flags in the console between ticks.
pub fn load() {
    let root = crate::memory_helper::root();
    let flags = features_from_memory();

    // Write the fully-resolved struct back so new/missing keys are visible in
    // Memory for the user to inspect and modify between ticks.
    if let Ok(js_val) = serde_wasm_bindgen::to_value(&flags) {
        let _ = js_sys::Reflect::set(&root, &JsValue::from_str("_features"), &js_val);
    }

    // Ensure reset sub-object also exists with defaults.
    let js_features = js_get(&root, "_features");
    let reset = js_get(&js_features, "reset");
    if reset.is_undefined() || reset.is_null() {
        let obj = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("environment"), &JsValue::from_bool(false));
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("memory"), &JsValue::from_bool(false));
        let _ = js_sys::Reflect::set(js_features.as_ref(), &JsValue::from_str("reset"), &obj);
    }

    CACHED.with(|c| *c.borrow_mut() = flags);
}
