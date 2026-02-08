use std::{fmt::Write, panic};

use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen]
extern "C" {
    type Error;

    #[wasm_bindgen(constructor)]
    fn new() -> Error;

    #[wasm_bindgen(structural, method, getter)]
    fn stack(error: &Error) -> String;

    #[wasm_bindgen(static_method_of = Error, setter, js_name = stackTraceLimit)]
    fn stack_trace_limit(size: f32);
}

pub fn setup_panic_hook() {
    panic::set_hook(Box::new(panic_hook));
}

fn panic_hook(info: &panic::PanicHookInfo) {
    // import JS Error API to get backtrace info (backtraces don't work in wasm)
    // Node 8 does support this API: https://nodejs.org/docs/latest-v8.x/api/errors.html#errors_error_stack

    let mut fmt_error = String::new();
    let _ = writeln!(fmt_error, "{}", info);

    // this could be controlled with an env var at compilation instead
    const SHOW_BACKTRACE: bool = true;

    if SHOW_BACKTRACE {
        Error::stack_trace_limit(10000_f32);
        let stack = Error::new().stack();
        // Skip all frames before the special symbol `__rust_end_short_backtrace`
        // and then skip that frame too.
        // Note: sometimes wasm-opt seems to delete that symbol.
        if stack.contains("__rust_end_short_backtrace") {
            for line in stack
                .lines()
                .skip_while(|line| !line.contains("__rust_end_short_backtrace"))
                .skip(1)
            {
                let _ = writeln!(fmt_error, "{}", line);
            }
        } else {
            // If there was no `__rust_end_short_backtrace` symbol, use the whole stack
            // but skip the first line, it just says Error.
            let stack = stack.split_once('\n').map(|(_, s)| s).unwrap_or(stack);
            let _ = writeln!(fmt_error, "{}", stack);
        }
    }

    log::error!("{}", fmt_error);
}
