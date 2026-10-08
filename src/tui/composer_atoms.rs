//! Source ranges shared by composer editing, wrapping and submission.

use std::ops::Range;

use super::input_text::{ceil_grapheme_offset, floor_grapheme_offset};
use super::state::{BottomPaneModel, char_offset_to_byte_index};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OwnedPaste {
    pub range: Range<usize>,
    pub label: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ComposerAtomKind {
    FileMention,
    LargePaste,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ComposerAtom {
    pub range: Range<usize>,
    pub kind: ComposerAtomKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ComposerDraft {
    pub input: String,
    pub cursor: Option<usize>,
    pub pastes: Vec<OwnedPaste>,
    pub paste_counter: u32,
}

/// A JSON string after `@` preserves spaces, quotes and Unicode in text history.
pub(crate) fn encode_mention(path: &str) -> String {
    format!("@{}", serde_json::Value::String(path.to_owned()))
}

fn valid_paste(input: &str, paste: &OwnedPaste) -> bool {
    let start = char_offset_to_byte_index(input, paste.range.start);
    let end = char_offset_to_byte_index(input, paste.range.end);
    input.get(start..end) == Some(paste.label.as_str())
        && paste.range.end - paste.range.start == paste.label.chars().count()
}

pub(crate) fn atoms(input: &str, pastes: &[OwnedPaste]) -> Vec<ComposerAtom> {
    let mut atoms = pastes
        .iter()
        .filter(|paste| valid_paste(input, paste))
        .map(|paste| ComposerAtom {
            range: paste.range.clone(),
            kind: ComposerAtomKind::LargePaste,
        })
        .collect::<Vec<_>>();
    for (byte, _) in input.match_indices("@\"") {
        if byte > 0
            && input[..byte]
                .chars()
                .next_back()
                .is_some_and(|ch| !ch.is_whitespace())
        {
            continue;
        }
        let start = input[..byte].chars().count();
        if atoms.iter().any(|atom| atom.range.contains(&start)) {
            continue;
        }
        let mut decoded =
            serde_json::Deserializer::from_str(&input[byte + 1..]).into_iter::<String>();
        if !matches!(decoded.next(), Some(Ok(path)) if !path.is_empty()) {
            continue;
        }
        let end = byte + 1 + decoded.byte_offset();
        atoms.push(ComposerAtom {
            range: start..start + input[byte..end].chars().count(),
            kind: ComposerAtomKind::FileMention,
        });
    }
    atoms.sort_by_key(|atom| atom.range.start);
    for atom in &mut atoms {
        atom.range.start = floor_grapheme_offset(input, atom.range.start);
        atom.range.end = ceil_grapheme_offset(input, atom.range.end);
    }
    atoms
}

impl BottomPaneModel {
    pub(crate) fn composer_atoms(&self) -> Vec<ComposerAtom> {
        atoms(&self.input, &self.large_paste_pending)
    }

    pub(crate) fn floor_atom_boundary(&self, offset: usize) -> usize {
        let offset = floor_grapheme_offset(&self.input, offset);
        self.composer_atoms()
            .iter()
            .find(|atom| atom.range.start < offset && offset < atom.range.end)
            .map_or(offset, |atom| {
                floor_grapheme_offset(&self.input, atom.range.start)
            })
    }

    pub(crate) fn ceil_atom_boundary(&self, offset: usize) -> usize {
        let offset = ceil_grapheme_offset(&self.input, offset);
        self.composer_atoms()
            .iter()
            .find(|atom| atom.range.start < offset && offset < atom.range.end)
            .map_or(offset, |atom| {
                ceil_grapheme_offset(&self.input, atom.range.end)
            })
    }

    /// Edits expand across touched atoms and rebase only owned paste ranges.
    pub(crate) fn edit_composer(&mut self, range: Range<usize>, inserted: &str) {
        let start = self.floor_atom_boundary(range.start);
        let end = if range.is_empty() {
            start
        } else {
            self.ceil_atom_boundary(range.end)
        };
        let inserted_count = inserted.chars().count();
        self.large_paste_pending
            .retain(|paste| valid_paste(&self.input, paste));
        self.large_paste_pending.retain_mut(|paste| {
            if paste.range.end <= start {
                return true;
            }
            if paste.range.start < end {
                return false;
            }
            paste.range.start = paste.range.start - (end - start) + inserted_count;
            paste.range.end = paste.range.end - (end - start) + inserted_count;
            true
        });
        let start_byte = char_offset_to_byte_index(&self.input, start);
        let end_byte = char_offset_to_byte_index(&self.input, end);
        self.input.replace_range(start_byte..end_byte, inserted);
        self.input_cursor_offset = Some(if inserted.is_empty() {
            self.floor_atom_boundary(start)
        } else {
            self.ceil_atom_boundary(start + inserted_count)
        });
    }

    pub(crate) fn saved_draft(&self) -> ComposerDraft {
        ComposerDraft {
            input: self.input.clone(),
            cursor: self.input_cursor_offset,
            pastes: self.large_paste_pending.clone(),
            paste_counter: self.large_paste_counter,
        }
    }

    pub(crate) fn restore_draft(&mut self, draft: ComposerDraft) {
        self.input = draft.input;
        self.input_cursor_offset = draft.cursor;
        self.large_paste_pending = draft.pastes;
        self.large_paste_counter = draft.paste_counter;
    }

    pub(crate) fn expand_owned_pastes(&mut self) {
        let mut pastes = std::mem::take(&mut self.large_paste_pending);
        pastes.sort_by_key(|paste| paste.range.start);
        // Reverse source order keeps every earlier range stable. A payload that
        // resembles another paste label is data, never a second replacement.
        for paste in pastes.into_iter().rev() {
            if valid_paste(&self.input, &paste) {
                let start = char_offset_to_byte_index(&self.input, paste.range.start);
                let end = char_offset_to_byte_index(&self.input, paste.range.end);
                self.input.replace_range(start..end, &paste.content);
            }
        }
        self.large_paste_counter = 0;
    }
}

#[cfg(test)]
mod tests;
