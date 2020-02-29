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
                        visualize: false,
                        execute: true
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
        ::memory::root().path_bool("_features.construction.visualize")
    }

    pub fn execute() -> bool {
        ::memory::root().path_bool("_features.construction.execute")
    }
}
