# Model Routing Status and Context Visibility

RARA already supports an `auxiliary_model` config field and conservative DeepSeek lite-model
inference for helper summarization paths, but the active routing decision was not visible in the TUI.
That made later context-compression work harder to debug because users could not tell whether helper
work would use the configured main model, an explicitly configured auxiliary model, or an inferred
provider-lite model.

This checkpoint adds a structured TUI model-routing view:

- main model and source from the effective provider surface;
- auxiliary model, source, and route;
- explicit auxiliary config before provider inference;
- conservative DeepSeek `deepseek-v4-pro` to `deepseek-v4-flash` inference;
- fallback-to-main reporting for providers without a helper route.

The `/status` overview, `/status` runtime text, and `/context` usage summary now render the same
structured routing view. This keeps the visible model-routing surface aligned with the future
auxiliary compression hook without starting the hook implementation yet.
