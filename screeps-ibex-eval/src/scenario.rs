//! Versioned scenario configs (P1.A3; component-test-plans §15.2).
//!
//! A scenario = run length + a FAULT SCHEDULE, expressed as data so a
//! run is reproducible from its config alone. v1 carries exactly what
//! Phase 1 needs (ticks + cpu-burn / global-reset faults); the §15.2
//! fields not yet represented — map/terrain seeds, opponents, RNG
//! seeds, gate expressions — are added WITH the scenarios that first
//! need them (don't speculate schema).
//!
//! Faults compile to the kit's generic console injections
//! ([`ConsoleInjection`]): JS snippets that set `Memory._features.*`
//! flags the bot reads —
//! - `eval.cpu_burn_ms` → the bot's tick-top synthetic burner (P1.A5),
//! - `reset.environment` → the bot's one-shot STATE-PRESERVING global
//!   reset (heap/env rebuild from segments — NOT a world wipe; the
//!   phase doc's "read this twice" note).

use anyhow::{bail, Context, Result};
use screeps_server_kit::capture::ConsoleInjection;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Bump on any non-additive change; [`Scenario::load`] rejects others.
pub const SCENARIO_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    /// Schema version ([`SCENARIO_SCHEMA_VERSION`]).
    pub v: u32,
    pub name: String,
    /// Observed ticks to capture.
    pub ticks: u64,
    #[serde(default)]
    pub faults: Vec<Fault>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fault {
    /// Synthetic CPU burn: `ms` per tick for `duration_ticks`, starting
    /// once `at_observed_tick` ticks have been observed.
    CpuBurn {
        at_observed_tick: u64,
        ms: u32,
        duration_ticks: u64,
    },
    /// One-shot state-preserving global reset (environment rebuild from
    /// segments) at the given observed tick.
    GlobalReset { at_observed_tick: u64 },
    /// Deliberate bot panic shortly after the given observed tick — the
    /// P1.C2 containment probe (loader catch → halt → fresh VM; the
    /// run's gates WILL report the panic line — that's the point; the
    /// verdict for containment scenarios is the counter inspection).
    PanicOnce { at_observed_tick: u64 },
}

/// JS guard chain creating `Memory._features.<group>` then assigning.
fn feature_set(group: &str, assignment: &str) -> String {
    format!(
        "if(!Memory._features)Memory._features={{}};\
         if(!Memory._features.{group})Memory._features.{group}={{}};\
         Memory._features.{group}.{assignment};"
    )
}

impl Scenario {
    /// The economy bring-up smoke — the Phase-0 loop, now config-shaped.
    pub fn builtin_smoke(ticks: u64) -> Scenario {
        Scenario {
            v: SCENARIO_SCHEMA_VERSION,
            name: "smoke".into(),
            ticks,
            faults: Vec::new(),
        }
    }

    /// Induced CPU pressure: burn hard through the middle third of the
    /// run, then release — the governor must shed and the colony must
    /// keep making progress (M1's first half).
    pub fn builtin_pressure(ticks: u64) -> Scenario {
        Scenario {
            v: SCENARIO_SCHEMA_VERSION,
            name: "pressure".into(),
            ticks,
            faults: vec![Fault::CpuBurn {
                at_observed_tick: ticks / 3,
                // Private-server limit is 100 cpu/tick with tick_limit
                // 500; 90 ms/tick burns the bucket at ~the refill rate's
                // ceiling and forces sustained negative trend.
                ms: 90,
                duration_ticks: ticks / 3,
            }],
        }
    }

    /// The M1 acceptance composite: pressure with a state-preserving
    /// global reset in the middle of the burn window.
    pub fn builtin_reset_under_pressure(ticks: u64) -> Scenario {
        let mut scenario = Self::builtin_pressure(ticks);
        scenario.name = "reset-under-pressure".into();
        scenario.faults.push(Fault::GlobalReset {
            at_observed_tick: ticks / 2,
        });
        scenario
    }

    /// P1.C2 containment acceptance: a deliberate panic mid-run. The
    /// panic gate WILL flag this run (expected); the verdict is the
    /// counter inspection: aborted_ticks=1, vm_starts=2, colony alive.
    pub fn builtin_panic_containment(ticks: u64) -> Scenario {
        Scenario {
            v: SCENARIO_SCHEMA_VERSION,
            name: "panic-containment".into(),
            ticks,
            faults: vec![Fault::PanicOnce {
                at_observed_tick: ticks / 2,
            }],
        }
    }

    pub fn builtin(name: &str, ticks: u64) -> Option<Scenario> {
        match name {
            "smoke" => Some(Self::builtin_smoke(ticks)),
            "pressure" => Some(Self::builtin_pressure(ticks)),
            "reset-under-pressure" => Some(Self::builtin_reset_under_pressure(ticks)),
            "panic-containment" => Some(Self::builtin_panic_containment(ticks)),
            _ => None,
        }
    }

    pub fn load(path: &Path) -> Result<Scenario> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading scenario {}", path.display()))?;
        let scenario: Scenario = serde_json::from_str(&raw)
            .with_context(|| format!("parsing scenario {}", path.display()))?;
        if scenario.v != SCENARIO_SCHEMA_VERSION {
            bail!(
                "scenario {} is schema v{} (this build reads v{})",
                path.display(),
                scenario.v,
                SCENARIO_SCHEMA_VERSION
            );
        }
        Ok(scenario)
    }

    /// Compile the fault schedule into kit console injections (pure).
    pub fn injections(&self) -> Vec<ConsoleInjection> {
        let mut out = Vec::new();
        for fault in &self.faults {
            match fault {
                Fault::CpuBurn {
                    at_observed_tick,
                    ms,
                    duration_ticks,
                } => {
                    out.push(ConsoleInjection {
                        at_observed_tick: *at_observed_tick,
                        expression: feature_set("eval", &format!("cpu_burn_ms={ms}")),
                        label: format!("cpu-burn on ({ms} ms/tick)"),
                    });
                    out.push(ConsoleInjection {
                        at_observed_tick: at_observed_tick + duration_ticks,
                        expression: feature_set("eval", "cpu_burn_ms=0"),
                        label: "cpu-burn off".into(),
                    });
                }
                Fault::GlobalReset { at_observed_tick } => {
                    out.push(ConsoleInjection {
                        at_observed_tick: *at_observed_tick,
                        expression: feature_set("reset", "environment=true"),
                        label: "global reset (environment)".into(),
                    });
                }
                Fault::PanicOnce { at_observed_tick } => {
                    out.push(ConsoleInjection {
                        at_observed_tick: *at_observed_tick,
                        // Game.time evaluates at injection time in the
                        // user VM; +2 clears the injection tick itself.
                        expression: feature_set("eval", "panic_at_tick=Game.time+2"),
                        label: "deliberate panic (containment probe)".into(),
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_names_resolve() {
        for name in ["smoke", "pressure", "reset-under-pressure"] {
            let s = Scenario::builtin(name, 600).unwrap();
            assert_eq!(s.name, name);
            assert_eq!(s.v, SCENARIO_SCHEMA_VERSION);
        }
        assert!(Scenario::builtin("nope", 600).is_none());
    }

    /// The fault→injection compilation is the load-bearing mapping:
    /// pin the flag paths and the on/off pairing.
    #[test]
    fn faults_compile_to_memory_flag_injections() {
        let s = Scenario::builtin_reset_under_pressure(600);
        let inj = s.injections();
        assert_eq!(inj.len(), 3);
        assert_eq!(inj[0].at_observed_tick, 200);
        assert!(inj[0].expression.contains("Memory._features.eval.cpu_burn_ms=90"));
        assert_eq!(inj[1].at_observed_tick, 400);
        assert!(inj[1].expression.contains("cpu_burn_ms=0"));
        assert_eq!(inj[2].at_observed_tick, 300);
        assert!(inj[2]
            .expression
            .contains("Memory._features.reset.environment=true"));
        // Guard chains so a fresh Memory tree can take the assignment.
        assert!(inj[0].expression.starts_with("if(!Memory._features)"));
    }

    #[test]
    fn scenario_round_trips_and_version_gates() {
        let s = Scenario::builtin_pressure(300);
        let json = serde_json::to_string_pretty(&s).unwrap();
        let dir = std::env::temp_dir().join("ibex-eval-scenario-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pressure.json");
        std::fs::write(&path, &json).unwrap();
        let loaded = Scenario::load(&path).unwrap();
        assert_eq!(loaded.name, s.name);
        assert_eq!(loaded.faults.len(), 1);
        // Wrong version is rejected loudly.
        let bad = json.replace("\"v\": 1", "\"v\": 99");
        std::fs::write(&path, bad).unwrap();
        assert!(Scenario::load(&path).is_err());
    }
}
