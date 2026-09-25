use crate::normalize::{
    is_array_of_arrays, is_array_of_objects, is_array_of_primitives, is_primitive,
};
use crate::primitives::{encode_key, encode_primitive, format_header, join_encoded_values};
use crate::types::{Depth, EncodeOptions, JsonPrimitive, JsonValue};
use crate::writer::LineWriter;
use std::collections::HashMap;

const LIST_ITEM_PREFIX: &str = "- ";

/// Encode a JsonValue to TOON format
pub fn encode_value(value: &JsonValue, options: &EncodeOptions) -> String {
    if is_primitive(value) {
        if let JsonValue::Primitive(p) = value {
            return encode_primitive(p, &options.delimiter);
        }
    }

    let mut writer = LineWriter::new(options.indent);

    match value {
        JsonValue::Array(arr) => encode_array(None, arr, &mut writer, 0, options),
        JsonValue::Object(obj) => encode_object(obj, &mut writer, 0, options),
        _ => {}
    }

    writer.to_string()
}

/// Encode an object
pub fn encode_object(
    obj: &HashMap<String, JsonValue>,
    writer: &mut LineWriter,
    depth: Depth,
    options: &EncodeOptions,
) {
    // We need to preserve insertion order, but HashMap doesn't guarantee it
    // For now, we'll sort keys alphabetically (JS version uses object key order)
    let mut keys: Vec<_> = obj.keys().collect();
    keys.sort();

    for key in keys {
        if let Some(value) = obj.get(key.as_str()) {
            encode_key_value_pair(key, value, writer, depth, options);
        }
    }
}

/// Encode a key-value pair
fn encode_key_value_pair(
    key: &str,
    value: &JsonValue,
    writer: &mut LineWriter,
    depth: Depth,
    options: &EncodeOptions,
) {
    let encoded_key = encode_key(key);

    match value {
        JsonValue::Primitive(p) => {
            writer.push(
                depth,
                format!(
                    "{}: {}",
                    encoded_key,
                    encode_primitive(p, &options.delimiter)
                ),
            );
        }
        JsonValue::Array(arr) => {
            encode_array(Some(key), arr, writer, depth, options);
        }
        JsonValue::Object(nested_obj) => {
            if nested_obj.is_empty() {
                writer.push(depth, format!("{}:", encoded_key));
            } else {
                writer.push(depth, format!("{}:", encoded_key));
                encode_object(nested_obj, writer, depth + 1, options);
            }
        }
    }
}

/// Encode an array
pub fn encode_array(
    key: Option<&str>,
    arr: &[JsonValue],
    writer: &mut LineWriter,
    depth: Depth,
    options: &EncodeOptions,
) {
    if arr.is_empty() {
        let header = format_header(0, key, None, &options.delimiter, options.length_marker);
        writer.push(depth, header);
        return;
    }

    // Primitive array
    if is_array_of_primitives(arr) {
        encode_inline_primitive_array(key, arr, writer, depth, options);
        return;
    }

    // Array of arrays (all primitives)
    if is_array_of_arrays(arr) {
        let all_primitive_arrays = arr.iter().all(|v| {
            if let JsonValue::Array(inner) = v {
                is_array_of_primitives(inner)
            } else {
                false
            }
        });

        if all_primitive_arrays {
            encode_array_of_arrays_as_list_items(key, arr, writer, depth, options);
            return;
        }
    }

    // Array of objects
    if is_array_of_objects(arr) {
        if let Some(header) = detect_tabular_header(arr) {
            encode_array_of_objects_as_tabular(key, arr, &header, writer, depth, options);
        } else {
            encode_mixed_array_as_list_items(key, arr, writer, depth, options);
        }
        return;
    }

    // Mixed array: fallback to expanded format
    encode_mixed_array_as_list_items(key, arr, writer, depth, options);
}

/// Encode primitive array inline
fn encode_inline_primitive_array(
    key: Option<&str>,
    arr: &[JsonValue],
    writer: &mut LineWriter,
    depth: Depth,
    options: &EncodeOptions,
) {
    let primitives: Vec<&JsonPrimitive> = arr
        .iter()
        .filter_map(|v| {
            if let JsonValue::Primitive(p) = v {
                Some(p)
            } else {
                None
            }
        })
        .collect();

    let header = format_header(
        arr.len(),
        key,
        None,
        &options.delimiter,
        options.length_marker,
    );
    let joined = join_encoded_values(&primitives, &options.delimiter);

    if arr.is_empty() {
        writer.push(depth, header);
    } else {
        writer.push(depth, format!("{} {}", header, joined));
    }
}

/// Encode array of arrays as list items
fn encode_array_of_arrays_as_list_items(
    key: Option<&str>,
    arr: &[JsonValue],
    writer: &mut LineWriter,
    depth: Depth,
    options: &EncodeOptions,
) {
    let header = format_header(
        arr.len(),
        key,
        None,
        &options.delimiter,
        options.length_marker,
    );
    writer.push(depth, header);

    for item in arr {
        if let JsonValue::Array(inner) = item {
            if is_array_of_primitives(inner) {
                let primitives: Vec<&JsonPrimitive> = inner
                    .iter()
                    .filter_map(|v| {
                        if let JsonValue::Primitive(p) = v {
                            Some(p)
                        } else {
                            None
                        }
                    })
                    .collect();

                let inline_header = format_header(
                    inner.len(),
                    None,
                    None,
                    &options.delimiter,
                    options.length_marker,
                );
                let joined = join_encoded_values(&primitives, &options.delimiter);

                if inner.is_empty() {
                    writer.push(depth + 1, format!("{}{}", LIST_ITEM_PREFIX, inline_header));
                } else {
                    writer.push(
                        depth + 1,
                        format!("{}{} {}", LIST_ITEM_PREFIX, inline_header, joined),
                    );
                }
            }
        }
    }
}

/// Detect if array of objects can use tabular format
fn detect_tabular_header(arr: &[JsonValue]) -> Option<Vec<String>> {
    if arr.is_empty() {
        return None;
    }

    let first_obj = match &arr[0] {
        JsonValue::Object(obj) => obj,
        _ => return None,
    };

    let mut first_keys: Vec<String> = first_obj.keys().cloned().collect();
    first_keys.sort();

    if first_keys.is_empty() {
        return None;
    }

    // Check if it's a tabular array
    if is_tabular_array(arr, &first_keys) {
        Some(first_keys)
    } else {
        None
    }
}

/// Check if array can use tabular format
fn is_tabular_array(arr: &[JsonValue], header: &[String]) -> bool {
    for value in arr {
        if let JsonValue::Object(obj) = value {
            let mut keys: Vec<String> = obj.keys().cloned().collect();
            keys.sort();

            // All objects must have the same keys
            if keys.len() != header.len() {
                return false;
            }

            // Check all header keys exist and values are primitives
            for key in header {
                match obj.get(key) {
                    Some(JsonValue::Primitive(_)) => {}
                    _ => return false,
                }
            }
        } else {
            return false;
        }
    }
    true
}

/// Encode array of objects in tabular format
fn encode_array_of_objects_as_tabular(
    key: Option<&str>,
    arr: &[JsonValue],
    header: &[String],
    writer: &mut LineWriter,
    depth: Depth,
    options: &EncodeOptions,
) {
    let header_str = format_header(
        arr.len(),
        key,
        Some(header),
        &options.delimiter,
        options.length_marker,
    );
    writer.push(depth, header_str);

    write_tabular_rows(arr, header, writer, depth + 1, options);
}

/// Write tabular rows
fn write_tabular_rows(
    arr: &[JsonValue],
    header: &[String],
    writer: &mut LineWriter,
    depth: Depth,
    options: &EncodeOptions,
) {
    for value in arr {
        if let JsonValue::Object(obj) = value {
            let values: Vec<&JsonPrimitive> = header
                .iter()
                .filter_map(|key| {
                    if let Some(JsonValue::Primitive(p)) = obj.get(key) {
                        Some(p)
                    } else {
                        None
                    }
                })
                .collect();

            let joined = join_encoded_values(&values, &options.delimiter);
            writer.push(depth, joined);
        }
    }
}

/// Encode mixed array as list items
fn encode_mixed_array_as_list_items(
    key: Option<&str>,
    arr: &[JsonValue],
    writer: &mut LineWriter,
    depth: Depth,
    options: &EncodeOptions,
) {
    let header = format_header(
        arr.len(),
        key,
        None,
        &options.delimiter,
        options.length_marker,
    );
    writer.push(depth, header);

    for item in arr {
        match item {
            JsonValue::Primitive(p) => {
                writer.push(
                    depth + 1,
                    format!(
                        "{}{}",
                        LIST_ITEM_PREFIX,
                        encode_primitive(p, &options.delimiter)
                    ),
                );
            }
            JsonValue::Array(inner) => {
                if is_array_of_primitives(inner) {
                    let primitives: Vec<&JsonPrimitive> = inner
                        .iter()
                        .filter_map(|v| {
                            if let JsonValue::Primitive(p) = v {
                                Some(p)
                            } else {
                                None
                            }
                        })
                        .collect();

                    let inline_header = format_header(
                        inner.len(),
                        None,
                        None,
                        &options.delimiter,
                        options.length_marker,
                    );
                    let joined = join_encoded_values(&primitives, &options.delimiter);

                    if inner.is_empty() {
                        writer.push(depth + 1, format!("{}{}", LIST_ITEM_PREFIX, inline_header));
                    } else {
                        writer.push(
                            depth + 1,
                            format!("{}{} {}", LIST_ITEM_PREFIX, inline_header, joined),
                        );
                    }
                }
            }
            JsonValue::Object(obj) => {
                encode_object_as_list_item(obj, writer, depth + 1, options);
            }
        }
    }
}

/// Encode object as list item
fn encode_object_as_list_item(
    obj: &HashMap<String, JsonValue>,
    writer: &mut LineWriter,
    depth: Depth,
    options: &EncodeOptions,
) {
    let mut keys: Vec<_> = obj.keys().collect();
    keys.sort();

    if keys.is_empty() {
        writer.push(depth, "-".to_string());
        return;
    }

    // First key-value on the same line as "- "
    let first_key = keys[0];
    let encoded_key = encode_key(first_key);
    let first_value = &obj[first_key.as_str()];

    match first_value {
        JsonValue::Primitive(p) => {
            writer.push(
                depth,
                format!(
                    "{}{}: {}",
                    LIST_ITEM_PREFIX,
                    encoded_key,
                    encode_primitive(p, &options.delimiter)
                ),
            );
        }
        JsonValue::Array(arr) => {
            if is_array_of_primitives(arr) {
                let primitives: Vec<&JsonPrimitive> = arr
                    .iter()
                    .filter_map(|v| {
                        if let JsonValue::Primitive(p) = v {
                            Some(p)
                        } else {
                            None
                        }
                    })
                    .collect();

                let inline_header = format_header(
                    arr.len(),
                    Some(first_key),
                    None,
                    &options.delimiter,
                    options.length_marker,
                );
                let joined = join_encoded_values(&primitives, &options.delimiter);

                if arr.is_empty() {
                    writer.push(depth, format!("{}{}", LIST_ITEM_PREFIX, inline_header));
                } else {
                    writer.push(
                        depth,
                        format!("{}{} {}", LIST_ITEM_PREFIX, inline_header, joined),
                    );
                }
            } else if is_array_of_objects(arr) {
                if let Some(header) = detect_tabular_header(arr) {
                    let header_str = format_header(
                        arr.len(),
                        Some(first_key),
                        Some(&header),
                        &options.delimiter,
                        options.length_marker,
                    );
                    writer.push(depth, format!("{}{}", LIST_ITEM_PREFIX, header_str));
                    write_tabular_rows(arr, &header, writer, depth + 1, options);
                } else {
                    writer.push(
                        depth,
                        format!("{}{}[{}]:", LIST_ITEM_PREFIX, encoded_key, arr.len()),
                    );
                    for inner_item in arr {
                        if let JsonValue::Object(inner_obj) = inner_item {
                            encode_object_as_list_item(inner_obj, writer, depth + 1, options);
                        }
                    }
                }
            } else {
                writer.push(
                    depth,
                    format!("{}{}[{}]:", LIST_ITEM_PREFIX, encoded_key, arr.len()),
                );
                encode_array(None, arr, writer, depth + 1, options);
            }
        }
        JsonValue::Object(nested_obj) => {
            if nested_obj.is_empty() {
                writer.push(depth, format!("{}{}:", LIST_ITEM_PREFIX, encoded_key));
            } else {
                writer.push(depth, format!("{}{}:", LIST_ITEM_PREFIX, encoded_key));
                encode_object(nested_obj, writer, depth + 2, options);
            }
        }
    }

    // Remaining keys on indented lines
    for key in keys.iter().skip(1) {
        encode_key_value_pair(key, &obj[key.as_str()], writer, depth + 1, options);
    }
}
