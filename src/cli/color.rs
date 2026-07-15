//! Minimal ANSI color handling: no dependency, honors `--no-color`,
//! the `NO_COLOR` convention (any non-empty value), and tty detection.

use std::io::IsTerminal;

/// ANSI escape sequences used by the human output. All fields are empty
/// strings when colors are disabled, so call sites format unconditionally.
pub struct Palette {
    pub red: &'static str,
    pub yellow: &'static str,
    pub cyan: &'static str,
    pub green: &'static str,
    pub bold: &'static str,
    pub dim: &'static str,
    pub reset: &'static str,
}

impl Palette {
    pub fn colored() -> Self {
        Palette {
            red: "\x1b[31m",
            yellow: "\x1b[33m",
            cyan: "\x1b[36m",
            green: "\x1b[32m",
            bold: "\x1b[1m",
            dim: "\x1b[2m",
            reset: "\x1b[0m",
        }
    }

    pub fn plain() -> Self {
        Palette {
            red: "",
            yellow: "",
            cyan: "",
            green: "",
            bold: "",
            dim: "",
            reset: "",
        }
    }

    /// Pick the palette for stdout given the `--no-color` flag.
    pub fn for_stdout(no_color_flag: bool) -> Self {
        if enabled(no_color_flag) {
            Palette::colored()
        } else {
            Palette::plain()
        }
    }
}

/// Colors are on iff: no `--no-color`, `NO_COLOR` env absent or empty
/// (https://no-color.org), and stdout is a terminal.
pub fn enabled(no_color_flag: bool) -> bool {
    !no_color_flag
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        && std::io::stdout().is_terminal()
}
