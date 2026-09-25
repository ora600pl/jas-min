//! Token-Oriented Object Notation (TOON)
//!
//! A compact, human-readable format designed for passing structured data to Large Language Models
//! with significantly reduced token usage.
//!
//! # Example
//!
//! ```
//! use toon::{encode, EncodeOptions, Delimiter};
//! use serde_json::json;
//!
//! let data = json!({
//!     "items": [
//!         {"sku": "A1", "qty": 2, "price": 9.99},
//!         {"sku": "B2", "qty": 1, "price": 14.5}
//!     ]
//! });
//!
//! let toon_output = encode(&data, None);
//! println!("{}", toon_output);
//! // Output:
//! // items[2]{price,qty,sku}:
//! //   9.99,2,A1
//! //   14.5,1,B2
//! ```

mod encoders;
mod normalize;
mod primitives;
mod types;
mod writer;

pub use serde_json;
pub use types::{Delimiter, EncodeOptions};

use encoders::encode_value;
use normalize::normalize_value;

/// Encode a serde_json::Value to TOON format
///
/// # Arguments
///
/// * `value` - A reference to a serde_json::Value to encode
/// * `options` - Optional encoding options. If None, defaults are used.
///
/// # Returns
///
/// A String containing the TOON-formatted output
///
/// # Example
///
/// ```
/// use toon::{encode, EncodeOptions, Delimiter};
/// use serde_json::json;
///
/// let data = json!({"name": "Ada", "active": true});
/// let result = encode(&data, None);
/// assert_eq!(result, "active: true\nname: Ada");
/// ```
pub fn encode(value: &serde_json::Value, options: Option<EncodeOptions>) -> String {
    let opts = options.unwrap_or_default();
    let normalized = normalize_value(value);
    encode_value(&normalized, &opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_object() {
        let data = json!({"id": 123, "name": "Ada"});
        let result = encode(&data, None);
        assert!(result.contains("id: 123"));
        assert!(result.contains("name: Ada"));
    }

    #[test]
    fn test_primitive_array() {
        let data = json!({"tags": ["reading", "gaming"]});
        let result = encode(&data, None);
        assert_eq!(result, "tags[2]: reading,gaming");
    }

    #[test]
    fn test_empty_object() {
        let data = json!({});
        let result = encode(&data, None);
        assert_eq!(result, "");
    }
}
