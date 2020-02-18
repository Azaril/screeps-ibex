static mut USERNAME: String = String::new();

pub mod user {
    pub fn name() -> &'static str {
        unsafe { &super::USERNAME }
    }

    pub fn set_name(name: &str) {
        unsafe {
            super::USERNAME = name.to_string();
        }
    }
}
