//! Minimal ANSI color palette. Whether colors are *on* is the host's call
//! (tty/`NO_COLOR` sniffing stays on the binary side); the renderers only
//! consume the resolved boolean.

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

    pub fn pick(color: bool) -> Self {
        if color {
            Palette::colored()
        } else {
            Palette::plain()
        }
    }
}
