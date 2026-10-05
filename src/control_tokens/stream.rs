use super::{DEEPSEEK_EOS, INTERNAL_BLOCK_TAGS};
use crate::llm::deepseek_dsml::DSML_TOKENS;

const CONTEXT_TOKENS: [&str; 6] = [
    INTERNAL_BLOCK_TAGS[0].open,
    INTERNAL_BLOCK_TAGS[1].open,
    INTERNAL_BLOCK_TAGS[2].open,
    DEEPSEEK_EOS,
    DSML_TOKENS[0],
    DSML_TOKENS[1],
];
const CONTEXT_WINDOW: usize = {
    let mut longest = 0;
    let mut index = 0;
    while index < CONTEXT_TOKENS.len() {
        if CONTEXT_TOKENS[index].len() > longest {
            longest = CONTEXT_TOKENS[index].len();
        }
        index += 1;
    }
    longest
};

#[derive(Default)]
enum LegacyPrefix {
    #[default]
    None,
    Open,
    Name,
    Pipe,
}

/// Recognizes when appending source can change canonical control-token cleanup.
/// Complex contexts stay on replay because later text can revise earlier output.
#[derive(Default)]
pub(crate) struct ControlTokenReplay {
    context_tail: [u8; CONTEXT_WINDOW],
    context_len: usize,
    legacy: LegacyPrefix,
    canonical_replay: bool,
    #[cfg(test)]
    pub(crate) scanned_bytes: usize,
}

impl ControlTokenReplay {
    pub(crate) fn requires_replay(&mut self, delta: &str) -> bool {
        if delta.is_empty() {
            return false;
        }
        if self.canonical_replay {
            return true;
        }
        let mut legacy_completed = false;
        for byte in delta.bytes() {
            #[cfg(test)]
            {
                self.scanned_bytes += 1;
            }
            if self.context_len == CONTEXT_WINDOW {
                self.context_tail.copy_within(1.., 0);
                self.context_len -= 1;
            }
            self.context_tail[self.context_len] = byte;
            self.context_len += 1;
            if CONTEXT_TOKENS
                .iter()
                .any(|token| self.context_tail[..self.context_len].ends_with(token.as_bytes()))
            {
                self.canonical_replay = true;
                return true;
            }

            self.legacy = match (&self.legacy, byte) {
                (_, b'<') => LegacyPrefix::Open,
                (LegacyPrefix::Open | LegacyPrefix::Name, byte)
                    if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') =>
                {
                    LegacyPrefix::Name
                }
                (LegacyPrefix::Name, b'|') => LegacyPrefix::Pipe,
                (LegacyPrefix::Pipe, b'>') => {
                    legacy_completed = true;
                    LegacyPrefix::None
                }
                (
                    LegacyPrefix::None
                    | LegacyPrefix::Open
                    | LegacyPrefix::Name
                    | LegacyPrefix::Pipe,
                    _,
                ) => LegacyPrefix::None,
            };
        }
        legacy_completed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_tokens::scrub_internal_control_tokens;

    #[test]
    fn every_canonical_context_token_selects_replay_across_splits() {
        let tokens = INTERNAL_BLOCK_TAGS
            .iter()
            .map(|tag| tag.open)
            .chain([DEEPSEEK_EOS])
            .chain(DSML_TOKENS);
        for token in tokens {
            for split in token
                .char_indices()
                .map(|(index, _)| index)
                .chain([token.len()])
            {
                let mut recognizer = ControlTokenReplay::default();
                let first = recognizer.requires_replay(&token[..split]);
                let second = recognizer.requires_replay(&token[split..]);
                assert!(
                    first || second,
                    "missing context token {token:?} at {split}"
                );
                assert!(recognizer.requires_replay("following text"));
            }
        }
    }

    #[test]
    fn legacy_recognition_matches_canonical_grammar_across_splits() {
        let sources = (0_u8..=127)
            .flat_map(|byte| {
                let ch = char::from(byte);
                [format!("<{ch}|>"), format!("<a{ch}b|>")]
            })
            .chain(
                [
                    "<|>",
                    "<\u{e9}|>",
                    "<\u{540d}|>",
                    "<a\u{e9}z|>",
                    "<name_-09|>",
                ]
                .into_iter()
                .map(str::to_owned),
            );
        for source in sources {
            let expected = scrub_internal_control_tokens(&source) != source;
            for split in source
                .char_indices()
                .map(|(index, _)| index)
                .chain([source.len()])
            {
                let mut recognizer = ControlTokenReplay::default();
                let first = recognizer.requires_replay(&source[..split]);
                let second = recognizer.requires_replay(&source[split..]);
                assert_eq!(
                    first || second,
                    expected,
                    "legacy source {source:?} at {split}"
                );
            }
        }
    }
}
