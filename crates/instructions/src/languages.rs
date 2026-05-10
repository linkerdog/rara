use std::path::Path;

/// High-quality software engineering and idiomatic patterns for specific languages.
/// Inspired by Claude Code's data references.

pub fn get_language_prompt(cwd: &str) -> Option<String> {
    let root = Path::new(cwd);

    // Simple detection based on project markers.
    if root.join("Cargo.toml").exists() {
        return Some(rust_prompt());
    }
    if root.join("package.json").exists() {
        return Some(typescript_prompt());
    }
    if root.join("go.mod").exists() {
        return Some(go_prompt());
    }
    if root.join("requirements.txt").exists() || root.join("pyproject.toml").exists() {
        return Some(python_prompt());
    }
    if root.join("pom.xml").exists() || root.join("build.gradle").exists() {
        return Some(java_prompt());
    }
    if root.join("CMakeLists.txt").exists() || root.join("Makefile").exists() {
        return Some(cpp_prompt());
    }
    if root.join("composer.json").exists() {
        return Some(php_prompt());
    }
    if root.join("Gemfile").exists() {
        return Some(ruby_prompt());
    }

    None
}

fn rust_prompt() -> String {
    "# Rust Best Practices\n\
     - Follow idiomatic Rust (ownership, borrowing, lifetimes).\n\
     - Prefer 'anyhow' for application-level error handling and 'thiserror' for libraries.\n\
     - Use 'tokio' for async tasks if already present in dependencies.\n\
     - Avoid 'unsafe' unless strictly necessary and justified.\n\
     - Use 'cargo fmt' for formatting and 'cargo clippy' for linting.\n\
     - Extend existing tests in 'tests/' or 'mod tests' when adding functionality."
        .to_string()
}

fn typescript_prompt() -> String {
    "# TypeScript Best Practices\n\
     - Use strict typing; avoid 'any' when possible.\n\
     - Prefer interfaces for public APIs and types for internal data structures.\n\
     - Use modern ESNext features (async/await, destructuring).\n\
     - Follow project-specific linting (ESLint, Prettier).\n\
     - Use 'npm test' or 'vitest/jest' to verify changes."
        .to_string()
}

fn go_prompt() -> String {
    "# Go Best Practices\n\
     - Follow standard 'Go way' (Effective Go).\n\
     - Handle errors explicitly; do not ignore them.\n\
     - Use 'go fmt' and 'go vet'.\n\
     - Prefer small, focused interfaces.\n\
     - Use 'go test' for verification."
        .to_string()
}

fn python_prompt() -> String {
    "# Python Best Practices\n\
     - Follow PEP 8 style guidelines.\n\
     - Use type hints (PEP 484) for clarity.\n\
     - Prefer 'pytest' for testing.\n\
     - Use virtual environments (venv/conda) and keep 'requirements.txt' updated.\n\
     - Use 'black' or 'ruff' for formatting if configured."
        .to_string()
}

fn java_prompt() -> String {
    "# Java Best Practices\n\
     - Follow standard Java naming conventions (CamelCase).\n\
     - Use modern Java features (Streams, Optionals).\n\
     - Prefer JUnit/Mockito for testing.\n\
     - Follow project-specific Checkstyle/Google Style Guide if present.\n\
     - Use Maven or Gradle tasks for verification."
        .to_string()
}

fn cpp_prompt() -> String {
    "# C++ Best Practices\n\
     - Use modern C++ (C++17/20) features.\n\
     - Prefer RAII and smart pointers over raw pointers.\n\
     - Use 'clang-format' and 'clang-tidy' if available.\n\
     - Follow project-specific style (e.g., Google, LLVM).\n\
     - Verify with existing build system (CMake/Make)."
        .to_string()
}

fn php_prompt() -> String {
    "# PHP Best Practices\n\
     - Follow PSR coding standards (PSR-1, PSR-12).\n\
     - Use strict typing (declare(strict_types=1)).\n\
     - Prefer PHPUnit for testing.\n\
     - Use Composer for dependency management.\n\
     - Follow project-specific linting (PHPStan, Psalm)."
        .to_string()
}

fn ruby_prompt() -> String {
    "# Ruby Best Practices\n\
     - Follow the Ruby Style Guide.\n\
     - Use idiomatic Ruby patterns (blocks, symbols).\n\
     - Prefer RSpec or Minitest for testing.\n\
     - Use Bundler for dependencies.\n\
     - Follow RuboCop rules if configured."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn detects_rust_project() {
        let temp = tempdir().expect("tempdir");
        fs::write(temp.path().join("Cargo.toml"), "").expect("write");
        let prompt = get_language_prompt(temp.path().to_str().unwrap());
        assert!(prompt.is_some());
        assert!(prompt.unwrap().contains("# Rust Best Practices"));
    }

    #[test]
    fn detects_typescript_project() {
        let temp = tempdir().expect("tempdir");
        fs::write(temp.path().join("package.json"), "").expect("write");
        let prompt = get_language_prompt(temp.path().to_str().unwrap());
        assert!(prompt.is_some());
        assert!(prompt.unwrap().contains("# TypeScript Best Practices"));
    }

    #[test]
    fn detects_python_project() {
        let temp = tempdir().expect("tempdir");
        fs::write(temp.path().join("requirements.txt"), "").expect("write");
        let prompt = get_language_prompt(temp.path().to_str().unwrap());
        assert!(prompt.is_some());
        assert!(prompt.unwrap().contains("# Python Best Practices"));
    }

    #[test]
    fn returns_none_for_unknown_project() {
        let temp = tempdir().expect("tempdir");
        let prompt = get_language_prompt(temp.path().to_str().unwrap());
        assert!(prompt.is_none());
    }
}
