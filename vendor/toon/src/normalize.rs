use crate::types::{JsonPrimitive, JsonValue};
use serde_json::Value;
use std::collections::HashMap;

/// Convert serde_json::Value to JsonValue
pub fn normalize_value(value: &Value) -> JsonValue {
    match value {
        Value::Null => JsonValue::Primitive(JsonPrimitive::Null),
        Value::Bool(b) => JsonValue::Primitive(JsonPrimitive::Boolean(*b)),
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                // Handle special numeric values
                if f.is_finite() {
                    // Canonicalize -0 to 0
                    if f == 0.0 {
                        JsonValue::Primitive(JsonPrimitive::Number(0.0))
                    } else {
                        JsonValue::Primitive(JsonPrimitive::Number(f))
                    }
                } else {
                    // NaN and Infinity become null
                    JsonValue::Primitive(JsonPrimitive::Null)
                }
            } else {
                JsonValue::Primitive(JsonPrimitive::Null)
            }
        }
        Value::String(s) => JsonValue::Primitive(JsonPrimitive::String(s.clone())),
        Value::Array(arr) => {
            let normalized: Vec<JsonValue> = arr.iter().map(normalize_value).collect();
            JsonValue::Array(normalized)
        }
        Value::Object(obj) => {
            let mut map = HashMap::new();
            for (k, v) in obj.iter() {
                map.insert(k.clone(), normalize_value(v));
            }
            JsonValue::Object(map)
        }
    }
}

/// Check if value is a primitive
pub fn is_primitive(value: &JsonValue) -> bool {
    matches!(value, JsonValue::Primitive(_))
}

/// Check if value is an array
pub fn is_array(value: &JsonValue) -> bool {
    matches!(value, JsonValue::Array(_))
}

/// Check if value is an object
pub fn is_object(value: &JsonValue) -> bool {
    matches!(value, JsonValue::Object(_))
}

/// Check if array contains only primitives
pub fn is_array_of_primitives(arr: &[JsonValue]) -> bool {
    arr.iter().all(is_primitive)
}

/// Check if array contains only arrays
pub fn is_array_of_arrays(arr: &[JsonValue]) -> bool {
    arr.iter().all(is_array)
}

/// Check if array contains only objects
pub fn is_array_of_objects(arr: &[JsonValue]) -> bool {
    arr.iter().all(is_object)
}
