pub mod js {
    pub fn prepare() {

        /*
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
                        credit_reserve: 10000000,
                        buy_energy: true,
                        buy_minerals: true,
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
                    claim: true,
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
        */
    }
}

//TODO: wiarchbe: Fix once memory is implemented.
pub mod reset {
    pub fn reset_environment() -> bool {
        //::screeps::memory::root().path_bool("_features.reset.environment")
        false
    }

    pub fn reset_memory() -> bool {
        //::screeps::memory::root().path_bool("_features.reset.memory")
        false
    }

    pub fn clear() {
        //::screeps::memory::root().path_set("_features.reset.environment", false);
        //::screeps::memory::root().path_set("_features.reset.memory", false);
    }
}

pub mod visualize {
    pub fn on() -> bool {
        //::screeps::memory::root().path_bool("_features.visualize.on")
        true
    }
}

pub mod construction {
    pub fn plan() -> bool {
        //::screeps::memory::root().path_bool("_features.construction.plan")
        true
    }

    pub fn force_plan() -> bool {
        //::screeps::memory::root().path_bool("_features.construction.force_plan")
        false
    }

    pub fn allow_replan() -> bool {
        //::screeps::memory::root().path_bool("_features.construction.allow_replan")
        false
    }

    pub fn execute() -> bool {
        //::screeps::memory::root().path_bool("_features.construction.execute")
        true
    }

    pub fn cleanup() -> bool {
        //::screeps::memory::root().path_bool("_features.construction.cleanup")
        false
    }

    pub fn visualize() -> bool {
        //::screeps::memory::root().path_bool("_features.construction.visualize.on")
        true
    }

    pub fn visualize_planner() -> bool {
        //::screeps::memory::root().path_bool("_features.construction.visualize.planner") && visualize()
        true
    }

    pub fn visualize_planner_best() -> bool {
        //::screeps::memory::root().path_bool("_features.construction.visualize.planner_best") && visualize()
        false
    }

    pub fn visualize_plan() -> bool {
        //::screeps::memory::root().path_bool("_features.construction.visualize.plan") && visualize()
        true
    }
}

pub mod market {
    pub fn buy() -> bool {
        //::screeps::memory::root().path_bool("_features.market.buy")
        false
    }

    pub fn sell() -> bool {
        //::screeps::memory::root().path_bool("_features.market.sell")
        true
    }

    pub fn credit_reserve() -> f64 {
        /*
        ::screeps::memory::root()
            .path_f64("_features.market.credit_reserve")
            .unwrap_or(None)
            .unwrap_or(10_000_000.0)
        */

        10_000_000.0
    }

    pub fn buy_energy() -> bool {
        //::screeps::memory::root().path_bool("_features.market.buy_energy")
        false
    }

    pub fn buy_minerals() -> bool {
        //::screeps::memory::root().path_bool("_features.market.buy_minerals")
        false
    }
}

pub mod transfer {
    pub fn visualize() -> bool {
        //::screeps::memory::root().path_bool("_features.transfer.visualize.on")
        false
    }

    pub fn visualize_haul() -> bool {
        //::screeps::memory::root().path_bool("_features.transfer.visualize.haul") && visualize()
        false
    }

    pub fn visualize_demand() -> bool {
        //::screeps::memory::root().path_bool("_features.transfer.visualize.demand") && visualize()
        false
    }

    pub fn visualize_orders() -> bool {
        //::screeps::memory::root().path_bool("_features.transfer.visualize.orders") && visualize()
        false
    }
}

pub mod remote_mine {
    pub fn harvest() -> bool {
        //::screeps::memory::root().path_bool("_features.remote_mine.harvest")
        true
    }

    pub fn reserve() -> bool {
        //::screeps::memory::root().path_bool("_features.remote_mine.reserve")
        true
    }
}

pub fn raid() -> bool {
    //::screeps::memory::root().path_bool("_features.raid")
    true
}

pub fn claim() -> bool {
    //::screeps::memory::root().path_bool("_features.claim")
    true
}

pub fn dismantle() -> bool {
    //::screeps::memory::root().path_bool("_features.dismantle")
    true
}

pub mod pathing {
    pub fn visualize() -> bool {
        //::screeps::memory::root().path_bool("_features.pathing.visualize.on") && crate::features::visualize::on()
        false
    }

    pub fn custom() -> bool {
        //::screeps::memory::root().path_bool("_features.pathing.custom")
        true
    }
}

pub mod room {
    pub fn visualize() -> bool {
        //::screeps::memory::root().path_bool("_features.room.visualize.on") && crate::features::visualize::on()
        false
    }
}
