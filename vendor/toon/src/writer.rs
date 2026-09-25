use crate::types::Depth;

/// Line writer for building indented output
pub struct LineWriter {
    lines: Vec<String>,
    indentation_string: String,
}

impl LineWriter {
    pub fn new(indent_size: usize) -> Self {
        Self {
            lines: Vec::new(),
            indentation_string: " ".repeat(indent_size),
        }
    }

    pub fn push(&mut self, depth: Depth, content: String) {
        let indent = self.indentation_string.repeat(depth);
        self.lines.push(format!("{}{}", indent, content));
    }

    pub fn to_string(self) -> String {
        self.lines.join("\n")
    }
}
