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
                        visualize: {
                            on: true,
                            plan: true,
                        },
                        execute: true
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
        ::memory::root().path_bool("_features.visualize.on")
    }
}

pub mod construction {
    pub fn plan() -> bool {
        ::memory::root().path_bool("_features.construction.plan")
    }

    pub fn visualize() -> bool {
        ::memory::root().path_bool("_features.construction.visualize.on")
    }

    pub fn execute() -> bool {
        ::memory::root().path_bool("_features.construction.execute")
    }
}

pub mod transfer {
    pub fn visualize() -> bool {
        ::memory::root().path_bool("_features.transfer.visualize.on")
    }

    pub fn visualize_haul() -> bool {
        ::memory::root().path_bool("_features.transfer.visualize.haul") && visualize()
    }    

    pub fn visualize_demand() -> bool {
        ::memory::root().path_bool("_features.transfer.visualize.demand") && visualize()
    }
}
