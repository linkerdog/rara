use anyhow::{Context, Result};

pub(in crate::tui) struct EditorCommand {
    pub program: String,
    pub arguments: Vec<String>,
}

impl EditorCommand {
    pub fn from_environment() -> Result<Self> {
        for name in ["VISUAL", "EDITOR"] {
            if let Some(value) = std::env::var_os(name) {
                let value = value
                    .to_str()
                    .with_context(|| format!("{name} is not valid UTF-8"))?;
                if !value.trim().is_empty() {
                    return Self::resolve(Some(value), None);
                }
            }
        }
        Self::resolve(None, None)
    }

    pub(super) fn resolve(visual: Option<&str>, editor: Option<&str>) -> Result<Self> {
        let value = visual
            .filter(|value| !value.trim().is_empty())
            .or_else(|| editor.filter(|value| !value.trim().is_empty()))
            .context(
                "Set VISUAL or EDITOR to edit the draft (for example, EDITOR='code --wait')",
            )?;
        let mut words = shlex::split(value)
            .context("Invalid quotes in VISUAL/EDITOR")?
            .into_iter();
        let program = words
            .next()
            .filter(|word| !word.is_empty())
            .context("Editor command is empty")?;
        Ok(Self {
            program,
            arguments: words.collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_precedence_and_quoted_arguments_are_explicit() {
        let command =
            EditorCommand::resolve(Some("'editor path' --wait 'two words'"), Some("ignored"))
                .unwrap();
        assert_eq!(command.program, "editor path");
        assert_eq!(command.arguments, ["--wait", "two words"]);
        let fallback = EditorCommand::resolve(Some(" \t"), Some("vim -f")).unwrap();
        assert_eq!(fallback.program, "vim");
        assert_eq!(fallback.arguments, ["-f"]);
        for value in [None, Some(" "), Some("''"), Some("'unclosed")] {
            assert!(EditorCommand::resolve(value, None).is_err());
        }
    }

    #[test]
    fn shell_expansions_are_literal_arguments() {
        let command =
            EditorCommand::resolve(None, Some("editor '$HOME' '$(touch sentinel)' ';' '*.md'"))
                .unwrap();
        assert_eq!(
            command.arguments,
            ["$HOME", "$(touch sentinel)", ";", "*.md"]
        );
    }
}
