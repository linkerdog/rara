use serde::{Deserialize, Serialize};

/// Controls whether multi-agent tools are available and when the model may
/// choose them without an explicit delegation request.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MultiAgentPolicy {
    Disabled,
    #[default]
    Explicit,
    ProactiveReadOnly,
}

impl MultiAgentPolicy {
    pub(crate) fn is_default(value: &Self) -> bool {
        matches!(value, Self::Explicit)
    }
}

#[cfg(test)]
mod tests {
    use super::MultiAgentPolicy;
    use crate::RaraConfig;

    #[test]
    fn defaults_to_explicit_and_is_omitted() {
        let config = RaraConfig::default();
        assert_eq!(config.multi_agent_policy, MultiAgentPolicy::Explicit);

        let json = serde_json::to_string(&config).expect("serialize config");
        assert!(!json.contains("multi_agent_policy"));
    }

    #[test]
    fn accepts_proactive_read_only() {
        let config: RaraConfig = serde_json::from_str(
            r#"{
                "provider": "mock",
                "multi_agent_policy": "proactive_read_only"
            }"#,
        )
        .expect("deserialize config");

        assert_eq!(
            config.multi_agent_policy,
            MultiAgentPolicy::ProactiveReadOnly
        );
    }
}
