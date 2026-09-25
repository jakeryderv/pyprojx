//! Applying the fixes diagnostics carry.
//!
//! Fixes are edits to ranges of the source text, so everything they do not
//! touch, including formatting and comments, stays as it was. Fixing repeats
//! until no applicable fix remains, since one fix can make another possible.
//! A fix that would make the file invalid TOML, alone or with the fixes
//! applied before it, such as two typos renamed to the same key, is skipped.

use crate::diagnostic::{Applicability, Diagnostic, Edit, Fix, Rule};
use crate::lock::Lock;
use crate::{Checked, check_with_lock, parse};

/// How many rounds of fixes to apply before giving up on reaching a fixed point.
const MAX_ROUNDS: usize = 10;

/// The result of fixing a file.
#[derive(Debug)]
pub struct Fixed {
    /// The fixed source text.
    pub text: String,
    /// How many problems were fixed.
    pub fixed: usize,
    /// How many fixes were skipped because they would make the file invalid.
    pub skipped: usize,
    /// The fixed text, checked again.
    pub checked: Checked,
}

/// Fixes the contents of a `pyproject.toml` file, applying fixes at least as
/// applicable as `applicability`. A file that is not valid UTF-8 or TOML is
/// left as it is.
pub fn fix(bytes: Vec<u8>, lock: Option<&Lock>, applicability: Applicability) -> Fixed {
    let mut checked = check_with_lock(bytes, lock);
    let mut fixed = 0;
    let mut skipped = 0;
    let broken = checked
        .diagnostics
        .iter()
        .any(|d| d.rule == Rule::InvalidToml);
    for _ in 0..if broken { 0 } else { MAX_ROUNDS } {
        let round = apply(&checked.text, &checked.diagnostics, applicability);
        skipped = skipped.max(round.skipped);
        if round.applied == 0 {
            break;
        }
        fixed += round.applied;
        checked = check_with_lock(round.text.into_bytes(), lock);
    }
    Fixed {
        text: checked.text.clone(),
        fixed,
        skipped,
        checked,
    }
}

/// The result of applying one round of fixes.
#[derive(Debug)]
pub struct Round {
    pub text: String,
    pub applied: usize,
    pub skipped: usize,
}

/// Applies the fixes of `diagnostics` at least as applicable as
/// `applicability` to `text`, in order, skipping any that overlap a fix
/// applied before it or that would make the text invalid TOML.
pub fn apply(text: &str, diagnostics: &[Diagnostic], applicability: Applicability) -> Round {
    let mut fixes: Vec<&Fix> = diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.fix.as_ref())
        .filter(|fix| fix.applicability >= applicability && !fix.edits.is_empty())
        .collect();
    fixes.sort_by_key(|fix| fix.edits.iter().map(|edit| edit.range.start).min());

    let mut chosen: Vec<&Edit> = Vec::new();
    let mut output = text.to_owned();
    let (mut applied, mut skipped) = (0, 0);
    for fix in fixes {
        let overlaps = fix.edits.iter().any(|edit| {
            chosen.iter().any(|other| {
                edit.range.start < other.range.end && other.range.start < edit.range.end
                    || edit.range == other.range
            })
        });
        if overlaps {
            continue;
        }
        let mut trial = chosen.clone();
        trial.extend(&fix.edits);
        let result = splice(text, &mut trial);
        if parse::parse(&result).diagnostics.is_empty() {
            chosen = trial;
            output = result;
            applied += 1;
        } else {
            skipped += 1;
        }
    }
    Round {
        text: output,
        applied,
        skipped,
    }
}

/// `text` with non-overlapping `edits` applied.
fn splice(text: &str, edits: &mut [&Edit]) -> String {
    edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
    let mut output = String::with_capacity(text.len());
    let mut last = 0;
    for edit in edits.iter() {
        output.push_str(&text[last..edit.range.start]);
        output.push_str(&edit.replacement);
        last = edit.range.end;
    }
    output.push_str(&text[last..]);
    output
}

/// The edit that renames a key or replaces a string's contents at `span` in
/// `text`, keeping its quotes, if any.
pub fn rename(text: &str, span: std::ops::Range<usize>, old: &str, new: &str) -> Edit {
    let written = &text[span.clone()];
    let replacement = match written.find(old) {
        Some(_) => written.replacen(old, new, 1),
        None => new.to_owned(),
    };
    Edit::replace(span, replacement)
}

/// The edit that removes the array entry at `entries[index]`, with its
/// separating comma, and its whole line if it is alone on it.
pub fn remove_entry(text: &str, entries: &[std::ops::Range<usize>], index: usize) -> Edit {
    let entry = &entries[index];
    let bytes = text.as_bytes();
    let skip_blanks = |mut at: usize| {
        while at < bytes.len() && matches!(bytes[at], b' ' | b'\t') {
            at += 1;
        }
        at
    };
    let mut end = skip_blanks(entry.end);
    let has_comma = end < bytes.len() && bytes[end] == b',';
    if has_comma {
        end = skip_blanks(end + 1);
        // A comment after the entry is about it.
        if end < bytes.len() && bytes[end] == b'#' {
            while end < bytes.len() && bytes[end] != b'\n' {
                end += 1;
            }
        }
    }
    let line_start = text[..entry.start].rfind('\n').map_or(0, |at| at + 1);
    let alone = text[line_start..entry.start].trim().is_empty()
        && (end >= bytes.len() || matches!(bytes[end], b'\n' | b'\r'));
    if alone && has_comma {
        let after_newline = text[end..].find('\n').map_or(text.len(), |at| end + at + 1);
        return Edit::delete(line_start..after_newline);
    }
    if !has_comma && index > 0 {
        // The last entry without a trailing comma: remove the comma before it.
        return Edit::delete(entries[index - 1].end..entry.end);
    }
    if has_comma {
        return Edit::delete(entry.start..end);
    }
    Edit::delete(entry.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remove(text: &str, entries: &[&str], index: usize) -> String {
        let spans: Vec<_> = entries
            .iter()
            .map(|entry| {
                let start = text.find(entry).unwrap();
                start..start + entry.len()
            })
            .collect();
        let edit = remove_entry(text, &spans, index);
        format!(
            "{}{}{}",
            &text[..edit.range.start],
            edit.replacement,
            &text[edit.range.end..]
        )
    }

    #[test]
    fn removes_array_entries() {
        assert_eq!(
            remove("a = [\"x\", \"y\"]\n", &["\"x\"", "\"y\""], 0),
            "a = [\"y\"]\n"
        );
        assert_eq!(
            remove("a = [\"x\", \"y\"]\n", &["\"x\"", "\"y\""], 1),
            "a = [\"x\"]\n"
        );
        assert_eq!(remove("a = [\"x\"]\n", &["\"x\""], 0), "a = []\n");
        let text = "a = [\n  \"x\",  # why\n  # about y\n  \"y\",\n]\n";
        assert_eq!(
            remove(text, &["\"x\"", "\"y\""], 0),
            "a = [\n  # about y\n  \"y\",\n]\n"
        );
        assert_eq!(
            remove(text, &["\"x\"", "\"y\""], 1),
            "a = [\n  \"x\",  # why\n  # about y\n]\n"
        );
    }

    #[test]
    fn skips_fixes_that_would_break_the_file() {
        use crate::diagnostic::Diagnostic;
        let text = "[project]\nnam = \"a\"\nnme = \"a\"\n";
        let typo = |name: &str| {
            let start = text.find(name).unwrap();
            Diagnostic::new(Rule::UnknownKey, "typo", start..start + 3).with_fix(Fix::unsafe_(
                "rename to `name`",
                vec![rename(text, start..start + 3, name, "name")],
            ))
        };
        let round = apply(text, &[typo("nam"), typo("nme")], Applicability::Unsafe);
        assert_eq!(round.text, "[project]\nname = \"a\"\nnme = \"a\"\n");
        assert_eq!((round.applied, round.skipped), (1, 1));
        // Unsafe fixes wait for `--unsafe-fixes`.
        assert_eq!(apply(text, &[typo("nam")], Applicability::Safe).applied, 0);
    }

    #[test]
    fn renames_keep_quotes() {
        let text = "\"line-lenght\" = 1";
        let edit = rename(text, 0..13, "line-lenght", "line-length");
        assert_eq!(edit.replacement, "\"line-length\"");
    }
}
