//! Schema migrations. v0 (no `schema` key in the manifest) called a
//! transmission an `emission` (tagged `"entity": "emission"`); everything
//! else is unchanged. A v0 catalog
//! is read through `value_v0` and re-saved at v1 on open.

use serde_json::Value;

use crate::Error;
use crate::state::State;

/// Rewrite v0 JSON in place: every `"entity": "emission"` becomes
/// `"transmission"`.
pub fn value_v0(v: &mut Value) {
    match v {
        Value::Object(map) => {
            if map.get("entity").and_then(Value::as_str) == Some("emission") {
                map.insert("entity".into(), Value::String("transmission".into()));
            }
            map.values_mut().for_each(value_v0);
        }
        Value::Array(items) => items.iter_mut().for_each(value_v0),
        _ => {}
    }
}

/// A v0 snapshot as current state.
pub fn snapshot_v0(bytes: &[u8]) -> Result<State, Error> {
    let mut v: Value = serde_json::from_slice(bytes)?;
    value_v0(&mut v);
    Ok(serde_json::from_value(v)?)
}
