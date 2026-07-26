//! Color *enablement* — the OS-flavored half of color handling: `--no-color`,
//! the `NO_COLOR` convention (any non-empty value), and tty detection. The
//! palette itself lives in the driver (`reactant::driver::palette`), which
//! only consumes the resolved boolean.

use std::io::IsTerminal;

/// Colors are on iff: no `--no-color`, `NO_COLOR` env absent or empty
/// (https://no-color.org), and stdout is a terminal.
pub fn enabled(no_color_flag: bool) -> bool {
    !no_color_flag
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        && std::io::stdout().is_terminal()
}
