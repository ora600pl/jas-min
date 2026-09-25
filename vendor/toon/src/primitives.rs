use crate::types::{Delimiter, JsonPrimitive};
use regex::Regex;
use std::sync::LazyLock;

/// Encode a primitive value
pub fn encode_primitive(value: &JsonPrimitive, delimiter: &Delimiter) -> String {
    match value {
        JsonPrimitive::Null => "null".to_string(),
        JsonPrimitive::Boolean(b) => b.to_string(),
        JsonPrimitive::Number(n) => format_number(*n),
        JsonPrimitive::String(s) => encode_string_literal(s, delimiter),
    }
}

/// Format number without scientific notation
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{:.0}", n)
    } else {
        // Remove trailing zeros after decimal point
        let s = format!("{}", n);
        s
    }
}

/// Encode string literal with quoting if necessary
pub fn encode_string_literal(value: &str, delimiter: &Delimiter) -> String {
    if is_safe_unquoted(value, delimiter) {
        value.to_string()
    } else {
        format!("\"{}\"", escape_string(value))
    }
}

/// Escape special characters in strings
pub fn escape_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Check if string can be safely unquoted
fn is_safe_unquoted(value: &str, delimiter: &Delimiter) -> bool {
    if value.is_empty() {
        return false;
    }

    if is_padded_with_whitespace(value) {
        return false;
    }

    if value == "true" || value == "false" || value == "null" {
        return false;
    }

    if is_numeric_like(value) {
        return false;
    }

    // Check for colon (always structural)
    if value.contains(':') {
        return false;
    }

    // Check for quotes and backslash
    if value.contains('"') || value.contains('\\') {
        return false;
    }

    // Check for brackets and braces
    if value.contains('[') || value.contains(']') || value.contains('{') || value.contains('}') {
        return false;
    }

    // Check for control characters
    if value.contains('\n') || value.contains('\r') || value.contains('\t') {
        return false;
    }

    // Check for the active delimiter
    if value.contains(delimiter.as_str()) {
        return false;
    }

    // Check for list marker at start
    if value.starts_with("- ") {
        return false;
    }

    true
}

/// Check if string looks like a number
fn is_numeric_like(value: &str) -> bool {
    // Match numbers like: 42, -3.14, 1e-6, 05, etc.
    static NUMERIC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^-?\d+(?:\.\d+)?(?:e[+-]?\d+)?$|^0\d+$").unwrap());
    NUMERIC.is_match(value)
}

/// Check if string has leading or trailing whitespace
fn is_padded_with_whitespace(value: &str) -> bool {
    value != value.trim()
}

/// Encode a key (object property name)
pub fn encode_key(key: &str) -> String {
    if is_valid_unquoted_key(key) {
        key.to_string()
    } else {
        format!("\"{}\"", escape_string(key))
    }
}

/// Check if key can be unquoted
fn is_valid_unquoted_key(key: &str) -> bool {
    static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][\w.]*$").unwrap());
    KEY.is_match(key)
}

/// Join encoded values with delimiter
pub fn join_encoded_values(values: &[&JsonPrimitive], delimiter: &Delimiter) -> String {
    values
        .iter()
        .map(|v| encode_primitive(v, delimiter))
        .collect::<Vec<_>>()
        .join(delimiter.as_str())
}

/// Format array header with optional key, fields, and delimiter marker
pub fn format_header(
    length: usize,
    key: Option<&str>,
    fields: Option<&[String]>,
    delimiter: &Delimiter,
    length_marker: Option<char>,
) -> String {
    let mut header = String::new();

    if let Some(k) = key {
        header.push_str(&encode_key(k));
    }

    header.push('[');
    if let Some(marker) = length_marker {
        header.push(marker);
    }
    header.push_str(&length.to_string());

    // Only include delimiter if it's not the default (comma)
    if !delimiter.is_default() {
        header.push_str(delimiter.as_str());
    }

    header.push(']');

    if let Some(field_list) = fields {
        header.push('{');
        let encoded_fields: Vec<String> = field_list.iter().map(|f| encode_key(f)).collect();
        header.push_str(&encoded_fields.join(delimiter.as_str()));
        header.push('}');
    }

    header.push(':');
    header
}
