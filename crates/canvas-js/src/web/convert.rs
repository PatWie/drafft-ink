//! Values across the boundary: errors as JS `Error`s, serde values as plain JS objects.

use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::prelude::*;

/// A thrown `Error` with this text.
pub(super) fn fail(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

/// A serde value as a plain JS value: objects, not `Map`s.
pub(super) fn to_js<T: Serialize + ?Sized>(value: &T) -> Result<JsValue, JsError> {
    value.serialize(&serde_wasm_bindgen::Serializer::json_compatible()).map_err(fail)
}

/// A JS value as a serde value.
pub(super) fn from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, JsError> {
    serde_wasm_bindgen::from_value(value).map_err(fail)
}

/// Options that may be left out: `undefined` and `null` give the default.
pub(super) fn options_from_js<T: DeserializeOwned + Default>(value: JsValue) -> Result<T, JsError> {
    if value.is_undefined() || value.is_null() { Ok(T::default()) } else { from_js(value) }
}

/// Writes a message to the browser console as an error. Used where no caller can be told: a listener that threw,
/// a deferred call that failed.
pub(super) fn console_error(message: &str) {
    web_sys::console::error_1(&JsValue::from_str(message));
}
