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
