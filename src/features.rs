pub mod js {
    pub fn prepare() {
        js! {
            _.defaultsDeep(Memory, {
                _features: {
                    visualize: {
                        on: true
                    },
                    construction: {
                        plan: true,
                        execute: true,
                        visualize: {
                            on: true,
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

    pub fn execute() -> bool {
        ::screeps::memory::root().path_bool("_features.construction.execute")
    }

    pub fn visualize() -> bool {
        ::screeps::memory::root().path_bool("_features.construction.visualize.on")
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
