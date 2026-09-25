//! Converting between byte offsets in a document and protocol positions.
//!
//! pyprojx's spans are byte offsets. The protocol counts lines from 0 and
//! characters in UTF-16 code units by default, or in UTF-8 bytes if the client
//! and server agree to.

use lsp_types::{Position, Range};

/// How the protocol counts characters within a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
}

/// The start of each line in a text, for converting positions.
pub struct LineIndex<'t> {
    text: &'t str,
    /// Byte offsets where lines start; the first is 0.
    starts: Vec<usize>,
    encoding: Encoding,
}

impl<'t> LineIndex<'t> {
    pub fn new(text: &'t str, encoding: Encoding) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        Self {
            text,
            starts,
            encoding,
        }
    }

    /// The position of a byte offset, which must be on a character boundary.
    pub fn position(&self, offset: usize) -> Position {
        let offset = offset.min(self.text.len());
        let line = self.starts.partition_point(|&start| start <= offset) - 1;
        let before = &self.text[self.starts[line]..offset];
        let character = match self.encoding {
            Encoding::Utf8 => before.len(),
            Encoding::Utf16 => before.chars().map(char::len_utf16).sum(),
        };
        Position {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            character: u32::try_from(character).unwrap_or(u32::MAX),
        }
    }

    pub fn range(&self, span: std::ops::Range<usize>) -> Range {
        Range {
            start: self.position(span.start),
            end: self.position(span.end),
        }
    }

    /// The byte offset of a position, clamped to the end of its line or of the
    /// text, and moved back to a character boundary if it falls inside one.
    pub fn offset(&self, position: Position) -> usize {
        let Some(&start) = self.starts.get(position.line as usize) else {
            return self.text.len();
        };
        let end = self
            .starts
            .get(position.line as usize + 1)
            .map_or(self.text.len(), |&next| next - 1);
        let mut units = 0;
        for (at, char) in self.text[start..end].char_indices() {
            let width = match self.encoding {
                Encoding::Utf8 => char.len_utf8(),
                Encoding::Utf16 => char.len_utf16(),
            };
            if units + width > position.character as usize {
                return start + at;
            }
            units += width;
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "a = \"é𝄞\"\nb = 1\n";

    fn at(text: &str, needle: &str) -> usize {
        text.find(needle).unwrap()
    }

    #[test]
    fn positions_count_utf16_units_or_utf8_bytes() {
        let utf16 = LineIndex::new(TEXT, Encoding::Utf16);
        let utf8 = LineIndex::new(TEXT, Encoding::Utf8);
        // The closing quote follows `é` (1 UTF-16 unit, 2 bytes) and `𝄞` (2 units,
        // 4 bytes).
        let quote = TEXT.rfind('"').unwrap();
        assert_eq!(
            utf16.position(quote),
            Position {
                line: 0,
                character: 8
            }
        );
        assert_eq!(
            utf8.position(quote),
            Position {
                line: 0,
                character: 11
            }
        );
        assert_eq!(
            utf16.position(at(TEXT, "b")),
            Position {
                line: 1,
                character: 0
            }
        );
        assert_eq!(
            utf16.position(TEXT.len()),
            Position {
                line: 2,
                character: 0
            }
        );
    }

    #[test]
    fn offsets_round_trip() {
        for encoding in [Encoding::Utf8, Encoding::Utf16] {
            let index = LineIndex::new(TEXT, encoding);
            for (offset, _) in TEXT.char_indices() {
                assert_eq!(index.offset(index.position(offset)), offset, "{encoding:?}");
            }
        }
    }

    #[test]
    fn offsets_clamp() {
        let index = LineIndex::new(TEXT, Encoding::Utf16);
        // Past the end of the first line: its end, before the newline.
        assert_eq!(
            index.offset(Position {
                line: 0,
                character: 99
            }),
            at(TEXT, "\n")
        );
        // Inside the surrogate pair of `𝄞`: the start of the character.
        assert_eq!(
            index.offset(Position {
                line: 0,
                character: 7
            }),
            at(TEXT, "𝄞")
        );
        assert_eq!(
            index.offset(Position {
                line: 9,
                character: 0
            }),
            TEXT.len()
        );
    }
}
