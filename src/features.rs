pub mod js {
    use stdweb::*;

    pub fn prepare() {
        js! {
            _.defaultsDeep(Memory, {
                _features: {
                    visualize: {
                        on: true
                    },
                    construction: {
                        plan: true,
                        force_plan: false,
                        execute: true,
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
                    raid: true
                }
            });
        }
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

    pub fn execute() -> bool {
        ::screeps::memory::root().path_bool("_features.construction.execute")
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