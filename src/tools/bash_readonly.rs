use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn prefix_from_tokens(tokens: &[String]) -> Option<String> {
    let program = tokens.first()?;
    let program = command_basename(program);
    if let Some(subcommand) = approval_subcommand_token(program, &tokens[1..]) {
        Some(format!("{program} {subcommand}"))
    } else {
        Some(program.to_string())
    }
}

pub(crate) fn normalized_tokens_summary(tokens: &[String]) -> String {
    let Some(program) = tokens.first() else {
        return String::new();
    };
    let program = command_basename(program);
    let rest = &tokens[1..];
    let args = approval_subcommand_index(program, rest)
        .map(|index| rest[index..].to_vec())
        .unwrap_or_else(|| rest.to_vec());
    std::iter::once(program.to_string())
        .chain(args)
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn command_basename(command: &str) -> &str {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command)
}

pub(crate) fn approval_subcommand_token<'a>(program: &str, args: &'a [String]) -> Option<&'a str> {
    approval_subcommand_index(program, args).and_then(|index| args.get(index).map(String::as_str))
}

pub(crate) fn approval_subcommand_index(program: &str, args: &[String]) -> Option<usize> {
    match program {
        "git" => skip_known_global_options(
            args,
            &["--no-pager", "--no-optional-locks"],
            &["-C", "-c", "--git-dir", "--work-tree"],
        ),
        "docker" => skip_known_global_options(
            args,
            &["--debug", "--tls", "--tlsverify"],
            &["--config", "--context", "--host", "-H", "--log-level"],
        ),
        _ => args.first().map(|_| 0),
    }
}

#[allow(clippy::if_same_then_else)]
pub(crate) fn skip_known_global_options(
    args: &[String],
    valueless_options: &[&str],
    value_options: &[&str],
) -> Option<usize> {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if valueless_options.contains(&arg) {
            index += 1;
        } else if value_options.contains(&arg) {
            index += 2;
        } else if value_options
            .iter()
            .any(|option| arg.starts_with(&format!("{option}=")))
        {
            index += 1;
        } else if arg.starts_with('-') {
            index += 1;
        } else {
            return Some(index);
        }
    }
    None
}

pub(crate) fn shell_command_is_read_only(command: &str) -> bool {
    if command.contains('\n')
        || command.contains('`')
        || command.contains("$(")
        || command.contains('>')
    {
        return false;
    }
    split_shell_segments(command)
        .filter(|segments| !segments.is_empty())
        .is_some_and(|segments| {
            segments.into_iter().all(|segment| {
                tokenize_shell_segment(&segment).is_some_and(|tokens| {
                    if tokens.is_empty() {
                        return false;
                    }
                    argv_is_read_only(&tokens[0], &tokens[1..])
                })
            })
        })
}

pub(crate) fn argv_is_read_only(program: &str, args: &[String]) -> bool {
    let program = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    match program {
        "pwd" | "ls" | "tree" | "cat" | "head" | "tail" | "wc" | "stat" | "file" | "du" | "df"
        | "which" | "type" | "whereis" | "uname" => true,
        "rg" | "grep" => !args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--files-with-matches=")),
        "sed" => !args.iter().any(|arg| {
            arg == "-i"
                || arg.starts_with("-i.")
                || arg == "--in-place"
                || arg.starts_with("--in-place=")
        }),
        "find" => !args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"
            )
        }),
        "fd" | "fdfind" => !args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "-x" | "--exec" | "-X" | "--exec-batch" | "--list-details"
            )
        }),
        "git" => git_args_are_read_only(args),
        "docker" => docker_args_are_read_only(args),
        "pyright" => !args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--watch" | "-w")),
        _ => false,
    }
}

pub(crate) fn git_args_are_read_only(args: &[String]) -> bool {
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--no-pager" | "--no-optional-locks" => index += 1,
            "-C" | "-c" | "--git-dir" | "--work-tree" => return false,
            value if value.starts_with('-') => return false,
            _ => break,
        }
    }
    let Some(subcommand) = args.get(index).map(String::as_str) else {
        return false;
    };
    let rest = &args[index + 1..];
    match subcommand {
        "diff" | "log" | "show" | "shortlog" | "status" | "blame" | "ls-files" | "merge-base"
        | "rev-parse" | "rev-list" | "describe" | "cat-file" | "for-each-ref" | "grep" => true,
        "stash" => rest.first().is_some_and(|value| value == "list"),
        "remote" => rest.is_empty() || rest == ["-v"] || rest == ["--verbose"],
        "config" => rest.first().is_some_and(|value| value == "--get"),
        "reflog" => !rest
            .iter()
            .any(|value| matches!(value.as_str(), "expire" | "delete" | "exists")),
        "branch" => {
            rest.is_empty()
                || rest.iter().all(|value| {
                    matches!(
                        value.as_str(),
                        "--list" | "-l" | "-a" | "--all" | "-r" | "--remotes" | "-v" | "-vv"
                    )
                })
        }
        _ => false,
    }
}

pub(crate) fn docker_args_are_read_only(args: &[String]) -> bool {
    args.first()
        .is_some_and(|value| matches!(value.as_str(), "ps" | "images" | "logs" | "inspect"))
}

pub(crate) fn shell_command_matches_single_approval_prefix(command: &str, prefix: &str) -> bool {
    let Some(segments) = shell_command_prefix_segments(command) else {
        return false;
    };
    if segments.len() != 1 {
        return false;
    }
    tokens_match_approval_prefix(&segments[0], prefix)
}

pub(crate) fn shell_command_allowed_by_approval_prefixes(
    command: &str,
    prefixes: &[String],
    allow_read_only_segments: bool,
) -> bool {
    let Some(segments) = shell_command_prefix_segments(command) else {
        return false;
    };
    if segments.is_empty() {
        return false;
    }

    segments.iter().all(|tokens| {
        if tokens.is_empty() {
            return false;
        }
        if allow_read_only_segments && argv_is_read_only(&tokens[0], &tokens[1..]) {
            return true;
        }
        prefixes
            .iter()
            .any(|prefix| tokens_match_approval_prefix(tokens, prefix))
    })
}

pub(crate) fn shell_command_prefix_segments(command: &str) -> Option<Vec<Vec<String>>> {
    if command_has_prefix_rule_unsafe_syntax(command) {
        return None;
    }
    split_shell_segments(command).and_then(|segments| {
        segments
            .into_iter()
            .map(|segment| {
                let tokens = tokenize_shell_segment(&segment)?;
                if tokens_have_env_assignment_prefix(&tokens) {
                    return None;
                }
                Some(tokens)
            })
            .collect()
    })
}

pub(crate) fn command_has_prefix_rule_unsafe_syntax(command: &str) -> bool {
    command.contains('\n')
        || command.contains('`')
        || command.contains('$')
        || command.contains('>')
        || command.contains('<')
        || command.contains('*')
        || command.contains('?')
        || command.contains('(')
        || command.contains(')')
}

pub(crate) fn tokens_have_env_assignment_prefix(tokens: &[String]) -> bool {
    tokens.first().is_some_and(|token| is_env_assignment(token))
}

pub(crate) fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub(crate) fn tokens_match_approval_prefix(tokens: &[String], prefix: &str) -> bool {
    let normalized = normalized_tokens_summary(tokens);
    normalized_summary_matches_prefix(&normalized, prefix)
}

pub(crate) fn summary_matches_exact_approval(summary: &str, prefix: &str) -> bool {
    let prefix = prefix.trim();
    !prefix.is_empty() && summary.trim() == prefix
}

pub(crate) fn normalized_summary_matches_prefix(normalized: &str, prefix: &str) -> bool {
    let prefix = prefix.trim();
    !prefix.is_empty()
        && (normalized == prefix
            || normalized
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with(char::is_whitespace)))
}

pub(crate) fn split_shell_segments(command: &str) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(ch) = chars.next() {
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            current.push(ch);
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                current.push(ch);
            }
            ';' | '|' => {
                push_shell_segment(&mut segments, &mut current);
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                push_shell_segment(&mut segments, &mut current);
            }
            '&' => return None,
            _ => current.push(ch),
        }
    }
    if quote.is_some() {
        return None;
    }
    push_shell_segment(&mut segments, &mut current);
    Some(segments)
}

pub(crate) fn push_shell_segment(segments: &mut Vec<String>, current: &mut String) {
    let segment = current.trim();
    if !segment.is_empty() {
        segments.push(segment.to_string());
    }
    current.clear();
}

pub(crate) fn tokenize_shell_segment(segment: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = segment.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(ch) = chars.next() {
        match quote {
            Some(active_quote) => {
                if ch == active_quote {
                    quote = None;
                } else if ch == '\\' && active_quote == '"' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                '<' => return None,
                value if value.is_whitespace() => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                _ => current.push(ch),
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Some(tokens)
}
