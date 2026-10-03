use super::{
    PersistedInteraction, PersistedLegacyRolloutMigration, PersistedPlanStep,
    PersistedRuntimeRolloutItem, PersistedStructuredRolloutEvent,
};

pub(super) fn canonical_rollout_events_for_legacy_migration(
    migration: &PersistedLegacyRolloutMigration,
) -> Vec<PersistedStructuredRolloutEvent> {
    let mut events = migration.structured_events.clone();
    if events
        .iter()
        .any(|event| matches!(event, PersistedStructuredRolloutEvent::RuntimeState { .. }))
    {
        return events;
    }

    let saw_plan_state = events
        .iter()
        .any(|event| matches!(event, PersistedStructuredRolloutEvent::PlanState { .. }));
    let saw_interaction = events
        .iter()
        .any(|event| matches!(event, PersistedStructuredRolloutEvent::Interaction { .. }));

    if !saw_plan_state
        && let Some((explanation, steps)) = legacy_runtime_plan_state(&migration.runtime_rollout)
    {
        events.push(PersistedStructuredRolloutEvent::PlanState {
            recorded_at: None,
            explanation,
            steps,
        });
    }
    if !saw_interaction {
        events.extend(
            legacy_runtime_interactions(&migration.runtime_rollout)
                .into_iter()
                .map(|interaction| PersistedStructuredRolloutEvent::Interaction {
                    recorded_at: None,
                    interaction,
                }),
        );
    }

    events
}

fn legacy_runtime_plan_state(
    items: &[PersistedRuntimeRolloutItem],
) -> Option<(Option<String>, Vec<PersistedPlanStep>)> {
    items.iter().find_map(|item| match item {
        PersistedRuntimeRolloutItem::PlanState { explanation, steps } => {
            Some((explanation.clone(), steps.clone()))
        }
        PersistedRuntimeRolloutItem::Interaction(_) => None,
    })
}

fn legacy_runtime_interactions(items: &[PersistedRuntimeRolloutItem]) -> Vec<PersistedInteraction> {
    items
        .iter()
        .filter_map(|item| match item {
            PersistedRuntimeRolloutItem::Interaction(interaction) => Some(interaction.clone()),
            PersistedRuntimeRolloutItem::PlanState { .. } => None,
        })
        .collect()
}
