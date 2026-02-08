use std::cell::RefCell;

thread_local! {
    static USERNAME: RefCell<String> = const { RefCell::new(String::new()) };
}

pub mod user {
    pub fn name() -> String {
        super::USERNAME.with(|u| u.borrow().clone())
    }

    pub fn set_name(name: &str) {
        super::USERNAME.with(|u| *u.borrow_mut() = name.to_string());
    }
}
