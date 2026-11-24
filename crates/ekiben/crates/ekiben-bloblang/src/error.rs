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

/// The text of an error, for `map_err`: one named function in place of a closure per call site,
/// so a call whose error never happens (a write into a `Vec`, an HMAC key) leaves no dead closure.
pub(crate) fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_structured_position_quotes_the_line_with_a_caret_under_the_column() {
        let e = ParseError::new(2, 3, "expected something");
        assert_eq!(e.to_string(), "line 2 char 3: expected something");
        assert_eq!(
            e.at_position_structured("root = 1\nab!cd"),
            "line 2 char 3: expected something\n  |\n2 | ab!cd\n  |   ^---"
        );
        let past = ParseError::new(9, 1, "eof");
        assert_eq!(
            past.at_position_structured("x"),
            "line 9 char 1: eof\n  |\n9 | \n  | ^---"
        );
        let long = "a".repeat(100);
        let cut = ParseError::new(1, 1, "m").at_position_structured(&long);
        assert!(cut.ends_with(&format!("1 | {}\n  | ^---", "a".repeat(60))));
    }

    #[test]
    fn an_offset_counts_lines_and_characters_and_clamps_past_the_end() {
        let e = ParseError::from_offset("ab\nčd", 5, "m");
        assert_eq!((e.line, e.col), (2, 2));
        let end = ParseError::from_offset("ab", 99, "m");
        assert_eq!((end.line, end.col), (1, 3));
        assert_eq!(BloblangError::runtime("boom").to_string(), "boom");
    }
}
