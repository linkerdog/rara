use super::state::{TuiApp, UnifiedModelPreset};

/// Keep visible rows, navigation bounds, and selection on one projection.
pub(super) fn matching_model_presets(app: &TuiApp) -> Vec<UnifiedModelPreset> {
    let query = app.model_search_query.to_ascii_lowercase();
    app.available_unified_model_presets()
        .into_iter()
        .filter(|preset| {
            preset.model_label.to_ascii_lowercase().contains(&query)
                || preset.provider_label.to_ascii_lowercase().contains(&query)
        })
        .collect()
}
