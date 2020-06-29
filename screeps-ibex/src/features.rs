pub mod js {
    use stdweb::*;

    pub fn prepare() {
        js! {
            _.defaultsDeep(Memory, {
                _features: {
                    reset: {
                        environment: false,
                        memory: false,
                    },
                    visualize: {
                        on: true
                    },
                    construction: {
                        plan: true,
                        force_plan: false,
                        allow_replan: false,
                        execute: true,
                        cleanup: false,
                        visualize: {
                            on: true,
                            planner: true,
                            planner_best: false,
                            plan: true,
                        },
                    },
                    market: {
                        buy: true,
                        sell: true,
                        credit_reserve: 10000000
                    },
                    transfer: {
                        visualize: {
                            on: true,
                            haul: true,
                            demand: false
                        }
                    },
                    remote_mine: {
                        harvest: true,
                        reserve: true
                    },
                    raid: true,
                    dismantle: true,
                    pathing: {
                        custom: true,
                        visualize: {
                            on: false
                        }
                    },
                    room: {
                        visualize: {
                            on: false
                        }
                    }
                }
            });
        }
    }
}

pub mod reset {
    pub fn reset_environment() -> bool {
        ::screeps::memory::root().path_bool("_features.reset.environment")
    }

    pub fn reset_memory() -> bool {
        ::screeps::memory::root().path_bool("_features.reset.memory")
    }

    pub fn clear() {
        ::screeps::memory::root().path_set("_features.reset.environment", false);
        ::screeps::memory::root().path_set("_features.reset.memory", false);
    }
}

pub mod visualize {
    pub fn on() -> bool {
        ::screeps::memory::root().path_bool("_features.visualize.on")
    }
}

pub mod construction {
    pub fn plan() -> bool {
        ::screeps::memory::root().path_bool("_features.construction.plan")
    }

    pub fn force_plan() -> bool {
        ::screeps::memory::root().path_bool("_features.construction.force_plan")
    }

    pub fn allow_replan() -> bool {
        ::screeps::memory::root().path_bool("_features.construction.allow_replan")
    }

    pub fn execute() -> bool {
        ::screeps::memory::root().path_bool("_features.construction.execute")
    }
    
    pub fn cleanup() -> bool {
        ::screeps::memory::root().path_bool("_features.construction.cleanup")
    }

    pub fn visualize() -> bool {
        ::screeps::memory::root().path_bool("_features.construction.visualize.on")
    }

    pub fn visualize_planner() -> bool {
        ::screeps::memory::root().path_bool("_features.construction.visualize.planner") && visualize()
    }

    pub fn visualize_planner_best() -> bool {
        ::screeps::memory::root().path_bool("_features.construction.visualize.planner_best") && visualize()
    }

    pub fn visualize_plan() -> bool {
        ::screeps::memory::root().path_bool("_features.construction.visualize.plan") && visualize()
    }
}

pub mod market {
    pub fn buy() -> bool {
        ::screeps::memory::root().path_bool("_features.market.buy")
    }

    pub fn sell() -> bool {
        ::screeps::memory::root().path_bool("_features.market.sell")
    }

    pub fn credit_reserve() -> f64 {
        ::screeps::memory::root()
            .path_f64("_features.market.credit_reserve")
            .unwrap_or(None)
            .unwrap_or(10_000_000.0)
    }
}

pub mod transfer {
    pub fn visualize() -> bool {
        ::screeps::memory::root().path_bool("_features.transfer.visualize.on")
    }

    pub fn visualize_haul() -> bool {
        ::screeps::memory::root().path_bool("_features.transfer.visualize.haul") && visualize()
    }

    pub fn visualize_demand() -> bool {
        ::screeps::memory::root().path_bool("_features.transfer.visualize.demand") && visualize()
    }

    pub fn visualize_orders() -> bool {
        ::screeps::memory::root().path_bool("_features.transfer.visualize.orders") && visualize()
    }
}

pub mod remote_mine {
    pub fn harvest() -> bool {
        ::screeps::memory::root().path_bool("_features.remote_mine.harvest")
    }

    pub fn reserve() -> bool {
        ::screeps::memory::root().path_bool("_features.remote_mine.reserve")
    }
}

pub fn raid() -> bool {
    ::screeps::memory::root().path_bool("_features.raid")
}

pub fn dismantle() -> bool {
    ::screeps::memory::root().path_bool("_features.dismantle")
}

pub mod pathing {
    pub fn visualize() -> bool {
        ::screeps::memory::root().path_bool("_features.pathing.visualize.on") && crate::features::visualize::on()
    }

    pub fn custom() -> bool {
        ::screeps::memory::root().path_bool("_features.pathing.custom")
    }
}

pub mod room {
    pub fn visualize() -> bool {
        ::screeps::memory::root().path_bool("_features.room.visualize.on") && crate::features::visualize::on()
    }
}