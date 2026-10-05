/// The output palette supported by the terminal environment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorLevel {
    Monochrome,
    Ansi8,
    Ansi16,
    Ansi256,
    TrueColor,
}

/// The display encoding available for UI symbols and rendered text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlyphSet {
    Ascii,
    Unicode,
}

/// Immutable presentation capabilities; detect once when binding a terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalCapabilities {
    pub colors: ColorLevel,
    pub glyphs: GlyphSet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Platform {
    Windows,
    Other,
}

impl TerminalCapabilities {
    /// Unrestricted rendering for offscreen consumers without a terminal binding.
    pub const FULL: Self = Self {
        colors: ColorLevel::TrueColor,
        glyphs: GlyphSet::Unicode,
    };

    pub fn detect() -> Self {
        let platform = if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Other
        };
        Self::from_env(platform, |name| {
            std::env::var_os(name).map(|value| value.to_string_lossy().into_owned())
        })
    }

    fn from_env(platform: Platform, get: impl Fn(&str) -> Option<String>) -> Self {
        let windows_terminal = platform == Platform::Windows
            || get("WT_SESSION").is_some_and(|value| !value.is_empty());
        let term = get("TERM").unwrap_or_default().to_ascii_lowercase();
        let color_term = get("COLORTERM").unwrap_or_default().to_ascii_lowercase();
        let no_color = get("NO_COLOR").is_some_and(|value| !value.is_empty());
        let colors = if no_color || term == "dumb" {
            ColorLevel::Monochrome
        } else if matches!(color_term.as_str(), "truecolor" | "24bit")
            || term.contains("direct")
            || term.contains("truecolor")
            || term.contains("24bit")
            || (term.is_empty() && windows_terminal)
        {
            ColorLevel::TrueColor
        } else if term.contains("256color") {
            ColorLevel::Ansi256
        } else if term.contains("16color") || term == "linux" {
            ColorLevel::Ansi16
        } else if term.is_empty() || matches!(term.as_str(), "vt100" | "vt102") {
            ColorLevel::Monochrome
        } else {
            ColorLevel::Ansi8
        };
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
            .into_iter()
            .find_map(|key| get(key).filter(|value| !value.is_empty()))
            .unwrap_or_default()
            .to_ascii_lowercase()
            .replace('-', "");
        let encoding = locale.split('@').next().unwrap_or_default();
        let glyphs = if term != "dumb"
            && (encoding == "utf8"
                || encoding.ends_with(".utf8")
                || (locale.is_empty() && windows_terminal))
        {
            GlyphSet::Unicode
        } else {
            GlyphSet::Ascii
        };
        Self { colors, glyphs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(vars: &[(&str, &str)]) -> TerminalCapabilities {
        detect_on(Platform::Other, vars)
    }

    fn detect_on(platform: Platform, vars: &[(&str, &str)]) -> TerminalCapabilities {
        TerminalCapabilities::from_env(platform, |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).into())
        })
    }

    #[test]
    fn windows_terminal_without_unix_environment_keeps_full_capabilities() {
        assert_eq!(
            detect(&[("WT_SESSION", "session")]),
            TerminalCapabilities::FULL
        );
    }

    #[test]
    fn native_windows_without_unix_environment_keeps_full_capabilities() {
        for vars in [vec![], vec![("TERM", ""), ("LANG", "")]] {
            assert_eq!(
                detect_on(Platform::Windows, &vars),
                TerminalCapabilities::FULL
            );
        }
        assert_eq!(detect(&[("WT_SESSION", "")]), detect(&[]));
    }

    #[test]
    fn windows_defaults_preserve_explicit_restrictions() {
        for platform in [Platform::Windows, Platform::Other] {
            for (vars, expected) in [
                (
                    vec![("NO_COLOR", "1")],
                    TerminalCapabilities {
                        colors: ColorLevel::Monochrome,
                        glyphs: GlyphSet::Unicode,
                    },
                ),
                (
                    vec![("TERM", "dumb"), ("LANG", "C.UTF-8")],
                    TerminalCapabilities {
                        colors: ColorLevel::Monochrome,
                        glyphs: GlyphSet::Ascii,
                    },
                ),
                (
                    vec![
                        ("TERM", "xterm-256color"),
                        ("LC_ALL", "C"),
                        ("LANG", "C.UTF-8"),
                    ],
                    TerminalCapabilities {
                        colors: ColorLevel::Ansi256,
                        glyphs: GlyphSet::Ascii,
                    },
                ),
                (
                    vec![("TERM", "vt100")],
                    TerminalCapabilities {
                        colors: ColorLevel::Monochrome,
                        glyphs: GlyphSet::Unicode,
                    },
                ),
            ] {
                let mut vars = vars;
                vars.push(("WT_SESSION", "session"));
                assert_eq!(
                    detect_on(platform, &vars),
                    expected,
                    "{platform:?} {vars:?}"
                );
            }
        }
    }

    #[test]
    fn explicit_color_suppression_wins_over_richer_hints() {
        for value in ["1", "0", "false"] {
            assert_eq!(
                detect(&[
                    ("NO_COLOR", value),
                    ("TERM", "xterm-256color"),
                    ("COLORTERM", "truecolor")
                ])
                .colors,
                ColorLevel::Monochrome
            );
        }
        assert_eq!(
            detect(&[("NO_COLOR", ""), ("COLORTERM", "truecolor")]).colors,
            ColorLevel::TrueColor
        );
        assert_eq!(
            detect(&[
                ("TERM", "dumb"),
                ("COLORTERM", "truecolor"),
                ("LANG", "C.UTF-8")
            ]),
            TerminalCapabilities {
                colors: ColorLevel::Monochrome,
                glyphs: GlyphSet::Ascii
            }
        );
    }

    #[test]
    fn terminal_hints_select_each_color_depth() {
        for (term, level) in [
            ("", ColorLevel::Monochrome),
            ("vt100", ColorLevel::Monochrome),
            ("vt102", ColorLevel::Monochrome),
            ("xterm", ColorLevel::Ansi8),
            ("screen", ColorLevel::Ansi8),
            ("screen-16color", ColorLevel::Ansi16),
            ("linux", ColorLevel::Ansi16),
            ("tmux-256color", ColorLevel::Ansi256),
            ("xterm-direct", ColorLevel::TrueColor),
        ] {
            assert_eq!(detect(&[("TERM", term)]).colors, level, "{term}");
        }
        for hint in ["truecolor", "24bit", "TrueColor"] {
            assert_eq!(
                detect(&[("TERM", "xterm"), ("COLORTERM", hint)]).colors,
                ColorLevel::TrueColor
            );
        }
    }

    #[test]
    fn locale_precedence_uses_the_first_nonempty_value() {
        assert_eq!(
            detect(&[
                ("LC_ALL", "C"),
                ("LC_CTYPE", "C.UTF-8"),
                ("LANG", "en_US.UTF8")
            ])
            .glyphs,
            GlyphSet::Ascii
        );
        assert_eq!(
            detect(&[
                ("LC_ALL", ""),
                ("LC_CTYPE", "POSIX"),
                ("LANG", "en_US.UTF8")
            ])
            .glyphs,
            GlyphSet::Ascii
        );
        assert_eq!(
            detect(&[
                ("LC_ALL", ""),
                ("LC_CTYPE", ""),
                ("LANG", "en_US.UTF-8@modifier")
            ])
            .glyphs,
            GlyphSet::Unicode
        );
        for locale in ["C.UTF-8", "en_US.utf8", "UTF-8", "ja_JP.UTF8"] {
            assert_eq!(
                detect(&[("LC_CTYPE", locale), ("LANG", "C")]).glyphs,
                GlyphSet::Unicode
            );
        }
        for locale in ["C", "POSIX", "en_US.ISO8859-1", "", "notutf8"] {
            assert_eq!(detect(&[("LANG", locale)]).glyphs, GlyphSet::Ascii);
        }
    }
}
