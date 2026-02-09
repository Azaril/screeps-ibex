use wasm_bindgen::JsValue;

/// Get the Memory root object from the Screeps global.
pub fn root() -> JsValue {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("Memory")).unwrap_or(JsValue::UNDEFINED)
}

/// Navigate a dotted path (e.g. "_features.reset.environment") in the Memory object.
pub fn path_get(path: &str) -> JsValue {
    let mut current = root();
    for key in path.split('.') {
        if current.is_undefined() || current.is_null() {
            return JsValue::UNDEFINED;
        }
        current = js_sys::Reflect::get(&current, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED);
    }
    current
}

/// Read a boolean value at a dotted path, defaulting to `false`.
pub fn path_bool(path: &str) -> bool {
    path_get(path).as_bool().unwrap_or(false)
}

/// Read an f64 value at a dotted path.
pub fn path_f64(path: &str) -> Option<f64> {
    let val = path_get(path);
    if val.is_undefined() || val.is_null() {
        None
    } else {
        val.as_f64()
    }
}

/// Set a value at a dotted path in the Memory object.
pub fn path_set(path: &str, value: impl Into<JsValue>) {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return;
    }
    let mut current = root();
    for key in &parts[..parts.len() - 1] {
        let next = js_sys::Reflect::get(&current, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED);
        if next.is_undefined() || next.is_null() {
            return;
        }
        current = next;
    }
    if let Some(last_key) = parts.last() {
        let _ = js_sys::Reflect::set(&current, &JsValue::from_str(last_key), &value.into());
    }
}

/// Get a sub-object of memory by key, returning None if missing.
pub fn dict(key: &str) -> Option<JsValue> {
    let val = js_sys::Reflect::get(&root(), &JsValue::from_str(key)).ok()?;
    if val.is_undefined() || val.is_null() {
        None
    } else {
        Some(val)
    }
}

/// Delete a key from a JsValue object.
pub fn del(obj: &JsValue, key: &str) {
    let _ = js_sys::Reflect::delete_property(&obj.clone().into(), &JsValue::from_str(key));
}

/// Get keys of a JsValue object.
pub fn keys(obj: &JsValue) -> Vec<String> {
    if let Ok(obj) = js_sys::Object::try_from(obj).ok_or(()) {
        js_sys::Object::keys(obj).iter().filter_map(|k| k.as_string()).collect()
    } else {
        Vec::new()
    }
}
