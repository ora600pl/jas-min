use std::collections::HashMap;

/// JSON primitive types
#[derive(Debug, Clone, PartialEq)]
pub enum JsonPrimitive {
    String(String),
    Number(f64),
    Boolean(bool),
    Null,
}

/// JSON value types
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Primitive(JsonPrimitive),
    Object(HashMap<String, JsonValue>),
    Array(Vec<JsonValue>),
}

/// Delimiter types for array values and tabular rows
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delimiter {
    Comma,
    Tab,
    Pipe,
}

impl Delimiter {
    pub fn as_str(&self) -> &'static str {
        match self {
            Delimiter::Comma => ",",
            Delimiter::Tab => "\t",
            Delimiter::Pipe => "|",
        }
    }

    pub fn is_default(&self) -> bool {
        matches!(self, Delimiter::Comma)
    }
}

impl Default for Delimiter {
    fn default() -> Self {
        Delimiter::Comma
    }
}

/// Encoding options
#[derive(Debug, Clone)]
pub struct EncodeOptions {
    /// Number of spaces per indentation level
    pub indent: usize,
    /// Delimiter to use for arrays and tabular rows
    pub delimiter: Delimiter,
    /// Optional marker to prefix array lengths
    pub length_marker: Option<char>,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            indent: 2,
            delimiter: Delimiter::Comma,
            length_marker: None,
        }
    }
}

pub type Depth = usize;
