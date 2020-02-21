pub mod experimental {
    #[cfg(feature = "experimental")]
    pub fn on() -> bool {
        true
    }

    #[cfg(not(feature = "experimental"))]
    pub fn on() -> bool {
        false
    }
}

pub mod construction {
    pub fn plan() -> bool {
        ::memory::root().path_bool("_features.construction.plan") || super::experimental::on()
    }

    pub fn visualize() -> bool {
        ::memory::root().path_bool("_features.construction.visualize") || super::experimental::on()
    }

    pub fn execute() -> bool {
        ::memory::root().path_bool("_features.construction.execute") || super::experimental::on()
    }
}
