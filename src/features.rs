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
                    transfer: {
                        visualize: {
                            on: true,
                            haul: true,
                            demand: false
                        }
                    }
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
