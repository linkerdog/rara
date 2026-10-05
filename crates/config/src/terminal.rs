use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TuiTerminalConfig {
    pub title: bool,
    pub notifications: TerminalNotificationMethod,
}

impl Default for TuiTerminalConfig {
    fn default() -> Self {
        Self {
            title: true,
            notifications: TerminalNotificationMethod::Off,
        }
    }
}

impl TuiTerminalConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalNotificationMethod {
    #[default]
    Off,
    Bell,
    Osc9,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RaraConfig;

    #[test]
    fn terminal_feedback_defaults_and_methods_round_trip() {
        let legacy_json = serde_json::to_value(RaraConfig::default()).unwrap();
        let legacy: RaraConfig = serde_json::from_value(legacy_json.clone()).unwrap();
        assert_eq!(legacy.tui.terminal, TuiTerminalConfig::default());
        assert!(serde_json::to_value(&legacy).unwrap().get("tui").is_none());
        for (value, method) in [
            ("off", TerminalNotificationMethod::Off),
            ("bell", TerminalNotificationMethod::Bell),
            ("osc9", TerminalNotificationMethod::Osc9),
        ] {
            let mut json = legacy_json.clone();
            json["tui"] =
                serde_json::json!({ "terminal": { "title": false, "notifications": value } });
            let mut config: RaraConfig = serde_json::from_value(json).unwrap();
            assert_eq!(config.tui.terminal.notifications, method);
            assert!(!config.tui.terminal.title);
            config = serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
            assert_eq!(config.tui.terminal.notifications, method);
            assert!(!config.tui.terminal.title);
        }
        let mut invalid = legacy_json;
        invalid["tui"] = serde_json::json!({ "terminal": { "notifications": "automatic" } });
        assert!(serde_json::from_value::<RaraConfig>(invalid).is_err());
    }
}
