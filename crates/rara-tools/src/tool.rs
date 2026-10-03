//! Compatibility exports for the canonical portable tool contract.

pub use rara_core::tool::{
    Tool, ToolCallContext, ToolError, ToolManager, ToolOutputStream, ToolProgressEvent,
};

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::{Value, json};

    use super::{ToolError, ToolManager};

    struct CoreTool;

    #[async_trait]
    impl rara_core::tool::Tool for CoreTool {
        fn name(&self) -> &str {
            "core_tool"
        }
        fn description(&self) -> &str {
            "A tool defined against the portable contract"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn call(&self, input: Value) -> Result<Value, ToolError> {
            Ok(input)
        }
    }

    #[test]
    fn compatibility_registry_accepts_the_canonical_trait() -> anyhow::Result<()> {
        let mut registry = ToolManager::new();
        registry.register(Box::new(CoreTool));
        let canonical: rara_core::tool::ToolManager = registry;
        let compatibility: &dyn super::Tool = canonical
            .get_tool("core_tool")
            .ok_or_else(|| anyhow::anyhow!("missing registered tool"))?;
        assert_eq!(compatibility.name(), "core_tool");
        Ok(())
    }
}
