//! Deterministic, bounded text-editing model over Unicode scalar values.
//!
//! Every index is a `usize` position in the scalar sequence, never a byte
//! offset. Operations that would leave the buffer out of bounds or above its
//! capacity return a typed error without mutating it. A secure buffer masks
//! `display()` while `content()` remains the exact stored text.

use std::fmt;

const MASK: char = '•';

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextBufferError {
    CursorOutOfRange {
        index: usize,
        len: usize,
    },
    InvalidRange {
        start: usize,
        end: usize,
    },
    RangeOutOfRange {
        start: usize,
        end: usize,
        len: usize,
    },
    NoSelection,
    CapacityExceeded {
        len: usize,
        added: usize,
        capacity: usize,
    },
}

impl fmt::Display for TextBufferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CursorOutOfRange { index, len } => {
                write!(f, "cursor index {index} is outside 0..={len}")
            }
            Self::InvalidRange { start, end } => {
                write!(f, "selection start {start} is after end {end}")
            }
            Self::RangeOutOfRange { start, end, len } => {
                write!(f, "selection {start}..{end} is outside 0..={len}")
            }
            Self::NoSelection => write!(f, "no active selection to replace"),
            Self::CapacityExceeded {
                len,
                added,
                capacity,
            } => write!(
                f,
                "{len} characters plus {added} exceed capacity {capacity}"
            ),
        }
    }
}

impl std::error::Error for TextBufferError {}

/// A bounded text buffer whose integer indices count Unicode scalar values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextBuffer {
    chars: Vec<char>,
    capacity: usize,
    cursor: usize,
    selection: Option<(usize, usize)>,
    secure: bool,
}

impl TextBuffer {
    /// Creates an empty buffer with a maximum scalar count.
    pub fn new(capacity: usize) -> Self {
        Self {
            chars: Vec::new(),
            capacity,
            cursor: 0,
            selection: None,
            secure: false,
        }
    }

    /// Creates a buffer from text that fits `capacity`; the cursor starts at its end.
    pub fn with_text(text: &str, capacity: usize) -> Result<Self, TextBufferError> {
        let mut buffer = Self::new(capacity);
        buffer.replace_range(0, 0, text)?;
        Ok(buffer)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of Unicode scalar values, not bytes.
    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn content(&self) -> String {
        self.chars.iter().copied().collect()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The half-open selected scalar range, if any.
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.selection
    }

    pub fn secure(&self) -> bool {
        self.secure
    }

    pub fn set_secure(&mut self, secure: bool) {
        self.secure = secure;
    }

    /// Returns one mask character per scalar when secure, otherwise the content.
    pub fn display(&self) -> String {
        if self.secure {
            std::iter::repeat(MASK).take(self.chars.len()).collect()
        } else {
            self.content()
        }
    }

    /// Moves the cursor and clears any selection.
    pub fn move_cursor(&mut self, index: usize) -> Result<(), TextBufferError> {
        if index > self.chars.len() {
            return Err(TextBufferError::CursorOutOfRange {
                index,
                len: self.chars.len(),
            });
        }
        self.cursor = index;
        self.selection = None;
        Ok(())
    }

    /// Selects the half-open scalar range `start..end` and puts the cursor at `end`.
    pub fn select(&mut self, start: usize, end: usize) -> Result<(), TextBufferError> {
        self.check_range(start, end)?;
        self.selection = Some((start, end));
        self.cursor = end;
        Ok(())
    }

    /// Replaces the active selection, or inserts at the cursor when none is active.
    pub fn insert(&mut self, text: &str) -> Result<(), TextBufferError> {
        let (start, end) = self.selection.unwrap_or((self.cursor, self.cursor));
        self.replace_range(start, end, text)
    }

    /// Deletes the selection or one scalar before the cursor; a start-of-buffer edit is a no-op.
    pub fn backspace(&mut self) -> Result<(), TextBufferError> {
        if let Some((start, end)) = self.selection {
            return self.replace_range(start, end, "");
        }
        if self.cursor == 0 {
            return Ok(());
        }
        self.replace_range(self.cursor - 1, self.cursor, "")
    }

    /// Deletes the selection or one scalar at the cursor; an end-of-buffer edit is a no-op.
    pub fn delete(&mut self) -> Result<(), TextBufferError> {
        if let Some((start, end)) = self.selection {
            return self.replace_range(start, end, "");
        }
        if self.cursor >= self.chars.len() {
            return Ok(());
        }
        let Some(end) = self.cursor.checked_add(1) else {
            return Ok(());
        };
        self.replace_range(self.cursor, end, "")
    }

    /// Replaces the active selection; an absent selection is a typed error.
    pub fn replace_selection(&mut self, text: &str) -> Result<(), TextBufferError> {
        let Some((start, end)) = self.selection else {
            return Err(TextBufferError::NoSelection);
        };
        self.replace_range(start, end, text)
    }

    fn check_range(&self, start: usize, end: usize) -> Result<(), TextBufferError> {
        let len = self.chars.len();
        if start > end {
            return Err(TextBufferError::InvalidRange { start, end });
        }
        if end > len {
            return Err(TextBufferError::RangeOutOfRange { start, end, len });
        }
        Ok(())
    }

    fn replace_range(
        &mut self,
        start: usize,
        end: usize,
        text: &str,
    ) -> Result<(), TextBufferError> {
        self.check_range(start, end)?;
        let added: Vec<char> = text.chars().collect();
        let removed = end - start;
        let base_len = self.chars.len() - removed;
        let capacity_error = || TextBufferError::CapacityExceeded {
            len: base_len,
            added: added.len(),
            capacity: self.capacity,
        };
        let Some(total_len) = base_len.checked_add(added.len()) else {
            return Err(capacity_error());
        };
        if total_len > self.capacity {
            return Err(capacity_error());
        }
        let Some(cursor) = start.checked_add(added.len()) else {
            return Err(capacity_error());
        };
        self.chars.splice(start..end, added);
        self.cursor = cursor;
        self.selection = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_scalar_indices_are_not_byte_indices() {
        let mut buffer = TextBuffer::with_text("a😀中", 4).unwrap();
        assert_eq!(buffer.len(), 3);
        buffer.move_cursor(2).unwrap();
        buffer.insert("é").unwrap();
        assert_eq!(buffer.content(), "a😀é中");
        assert_eq!(buffer.cursor(), 3);

        buffer.move_cursor(1).unwrap();
        buffer.delete().unwrap();
        assert_eq!(buffer.content(), "aé中");
        assert_eq!(buffer.cursor(), 1);

        buffer.move_cursor(3).unwrap();
        buffer.backspace().unwrap();
        assert_eq!(buffer.content(), "aé");
        assert_eq!(buffer.cursor(), 2);
    }

    #[test]
    fn cursor_bounds_and_edge_edits_are_typed_errors() {
        let mut buffer = TextBuffer::with_text("abc", 3).unwrap();
        assert_eq!(
            buffer.move_cursor(4),
            Err(TextBufferError::CursorOutOfRange { index: 4, len: 3 })
        );
        assert_eq!(
            buffer.move_cursor(usize::MAX),
            Err(TextBufferError::CursorOutOfRange {
                index: usize::MAX,
                len: 3,
            })
        );
        assert_eq!(buffer.cursor(), 3);

        buffer.move_cursor(0).unwrap();
        buffer.backspace().unwrap();
        assert_eq!(buffer.content(), "abc");
        assert_eq!(buffer.cursor(), 0);

        buffer.move_cursor(3).unwrap();
        buffer.delete().unwrap();
        assert_eq!(buffer.content(), "abc");
        assert_eq!(buffer.cursor(), 3);
    }

    #[test]
    fn selection_replace_and_insert_use_scalar_ranges() {
        let mut buffer = TextBuffer::with_text("a😀中z", 6).unwrap();
        buffer.select(1, 3).unwrap();
        assert_eq!(buffer.selection(), Some((1, 3)));
        assert_eq!(buffer.cursor(), 3);
        buffer.replace_selection("✨界").unwrap();
        assert_eq!(buffer.content(), "a✨界z");
        assert_eq!(buffer.cursor(), 3);
        assert_eq!(buffer.selection(), None);

        buffer.select(0, 1).unwrap();
        buffer.insert("X").unwrap();
        assert_eq!(buffer.content(), "X✨界z");
        assert_eq!(buffer.cursor(), 1);
    }

    #[test]
    fn secure_masking_does_not_mutate_content() {
        let mut buffer = TextBuffer::with_text("a😀中", 3).unwrap();
        buffer.set_secure(true);
        assert!(buffer.secure());
        assert_eq!(buffer.display(), "•••");
        assert_eq!(buffer.content(), "a😀中");

        buffer.move_cursor(3).unwrap();
        buffer.backspace().unwrap();
        assert_eq!(buffer.display(), "••");
        assert_eq!(buffer.content(), "a😀");
        buffer.set_secure(false);
        assert_eq!(buffer.display(), "a😀");
    }

    #[test]
    fn malformed_ranges_are_rejected_with_typed_errors() {
        let mut buffer = TextBuffer::with_text("a😀中", 3).unwrap();
        assert_eq!(
            buffer.select(2, 1),
            Err(TextBufferError::InvalidRange { start: 2, end: 1 })
        );
        assert_eq!(
            buffer.select(0, 4),
            Err(TextBufferError::RangeOutOfRange {
                start: 0,
                end: 4,
                len: 3,
            })
        );
        assert_eq!(
            buffer.select(usize::MAX, usize::MAX),
            Err(TextBufferError::RangeOutOfRange {
                start: usize::MAX,
                end: usize::MAX,
                len: 3,
            })
        );
        assert_eq!(buffer.selection(), None);
        assert_eq!(
            buffer.replace_selection("x"),
            Err(TextBufferError::NoSelection)
        );
        assert_eq!(buffer.content(), "a😀中");
    }

    #[test]
    fn capacity_is_enforced_without_mutating_the_buffer() {
        assert_eq!(
            TextBuffer::with_text("abcd", 3).unwrap_err(),
            TextBufferError::CapacityExceeded {
                len: 0,
                added: 4,
                capacity: 3,
            }
        );

        let mut buffer = TextBuffer::with_text("ab", 3).unwrap();
        buffer.move_cursor(2).unwrap();
        assert_eq!(
            buffer.insert("xyz"),
            Err(TextBufferError::CapacityExceeded {
                len: 2,
                added: 3,
                capacity: 3,
            })
        );
        assert_eq!(buffer.content(), "ab");
        assert_eq!(buffer.cursor(), 2);

        buffer.select(0, 2).unwrap();
        buffer.replace_selection("xyz").unwrap();
        assert_eq!(buffer.content(), "xyz");
        assert_eq!(buffer.cursor(), 3);
    }

    #[test]
    fn same_operation_sequence_is_deterministic() {
        fn sequence() -> TextBuffer {
            let mut buffer = TextBuffer::with_text("中😀", 8).unwrap();
            buffer.select(0, 2).unwrap();
            buffer.insert("A✨é").unwrap();
            buffer.move_cursor(2).unwrap();
            buffer.backspace().unwrap();
            buffer.delete().unwrap();
            buffer.set_secure(true);
            buffer
        }

        let first = sequence();
        let second = sequence();
        assert_eq!(first, second);
        assert_eq!(first.content(), second.content());
        assert_eq!(first.display(), second.display());
        assert_eq!(first.cursor(), second.cursor());
        assert_eq!(first.selection(), second.selection());
    }
}
