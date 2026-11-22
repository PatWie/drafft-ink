//! The wasm-bindgen glue: JavaScript hosts for the stores, the `RoomCore` class and the edge functions.

mod edge_api;
mod hosts;
mod room_core;

use wasm_bindgen::JsError;

/// Turns a typed error into the exception that the shell sees.
fn js_error(error: &impl std::error::Error) -> JsError {
    JsError::new(&error.to_string())
}

/// Milliseconds from JavaScript (`Date.now()`), as an integer. A value that is not a time counts as 0.
fn ms(value: f64) -> u64 {
    if value.is_finite() && value >= 0.0 { value as u64 } else { 0 }
}

/// A number for JavaScript. Values above 2^53 are not exact, and no sequence number or time gets there.
fn num(value: u64) -> f64 {
    value as f64
}
