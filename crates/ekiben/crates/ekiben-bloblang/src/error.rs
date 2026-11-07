use thiserror::Error;

/// Error returned when parsing a Bloblang mapping or field interpolation fails.
#[derive(Debug, Clone, Error)]
#[error("line {line} char {col}: {message}")]
pub struct ParseError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl ParseError {
    pub fn new(line: usize, col: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            col,
            message: message.into(),
        }
    }

    /// Bento's `ErrorAtPositionStructured`: the position line, then the source line with a caret under it.
    pub fn at_position_structured(&self, input: &str) -> String {
        let src = input
            .split('\n')
            .nth(self.line.saturating_sub(1))
            .unwrap_or("");
        let char = self.col.saturating_sub(1);
        let src: String = src.chars().take(char.saturating_add(60)).collect();
        let num = self.line.to_string();
        let pad = " ".repeat(num.len());
        format!(
            "line {} char {}: {}\n{pad} |\n{num} | {src}\n{pad} | {}^---",
            self.line,
            self.col,
            self.message,
            " ".repeat(char)
        )
    }

    pub fn from_offset(input: &str, byte_offset: usize, message: impl Into<String>) -> Self {
        let clamped = byte_offset.min(input.len());
        let prefix = &input[..clamped];
        let mut line = 1usize;
        let mut line_start = 0usize;
        for (i, ch) in prefix.char_indices() {
            if ch == '\n' {
                line += 1;
                line_start = i + 1;
            }
        }
        let col = input[line_start..clamped].chars().count() + 1;
        Self {
            line,
            col,
            message: message.into(),
        }
    }
}

/// Error returned during Bloblang execution or compilation.
#[derive(Debug, Clone, Error)]
pub enum BloblangError {
    #[error(transparent)]
    Parse(#[from] ParseError),

    #[error("failed assignment (line {line}): {message}")]
    Assignment { line: usize, message: String },

    #[error("entering {annotation} exceeded maximum allowed stacks of {max_stacks}, this could be due to unbounded recursion")]
    MaxStacksExceeded {
        annotation: String,
        max_stacks: usize,
    },

    #[error("unable to reference message as structured (with '{field}'): {source_err}")]
    NoStructuredContext { field: String, source_err: String },

    #[error("{0}")]
    Runtime(String),
}

impl BloblangError {
    pub fn runtime(msg: impl Into<String>) -> Self {
        Self::Runtime(msg.into())
    }
}
