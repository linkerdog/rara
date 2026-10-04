use std::collections::HashMap;

use pulldown_cmark::{CowStr, RefDefs};
use unicase::UniCase;

/// Source statistics for one parser invocation or the complete document.
pub(crate) struct ReferenceBudget {
    pub source_bytes: usize,
    pub closing_brackets: usize,
}

/// Owned parser definitions shared by suffix and stable-prefix rendering.
#[derive(Default)]
pub(crate) struct ReferenceContext {
    definitions: HashMap<UniCase<String>, (CowStr<'static>, CowStr<'static>)>,
    last_definition_end: usize,
    max_expansion: usize,
}

impl ReferenceContext {
    pub(crate) fn from_definitions(definitions: &RefDefs<'_>) -> Self {
        let mut context = Self::default();
        for (label, definition) in definitions.iter() {
            let destination = definition.dest.clone().into_static();
            let title = definition
                .title
                .clone()
                .unwrap_or(CowStr::Borrowed(""))
                .into_static();
            context.max_expansion = context.max_expansion.max(destination.len() + title.len());
            context.last_definition_end = context.last_definition_end.max(definition.span.end);
            context
                .definitions
                .insert(UniCase::new(label.to_owned()), (destination, title));
        }
        context
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    pub(crate) fn resolve(&self, label: &str) -> Option<(CowStr<'static>, CowStr<'static>)> {
        if self.is_empty() {
            return None;
        }
        self.definitions
            .get(&UniCase::new(label.to_owned()))
            .cloned()
    }

    pub(crate) fn has_mutable_definition(&self, stable_source_len: usize) -> bool {
        self.last_definition_end > stable_source_len
    }

    pub(crate) fn allows_incremental(&self, budget: ReferenceBudget) -> bool {
        // pulldown-cmark 0.13 spends destination/title bytes at most once per
        // closing-bracket candidate, with max(input.len(), 100_000) fuel. Each
        // fragment and the whole document must fit their own budget; otherwise
        // splitting could change which links are resolved.
        self.max_expansion.saturating_mul(budget.closing_brackets)
            < budget.source_bytes.max(100_000)
    }
}
