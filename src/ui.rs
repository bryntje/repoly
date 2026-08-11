//! Terminal styling for **human-facing** CLI output.
//!
//! Agent/machine formats (`json`, `prompt`, `grok`, MCP) must not use this module
//! so they stay free of ANSI escapes.

use anstyle::{AnsiColor, Color, Effects, Style};
use clap::ValueEnum;
use std::env;
use std::io::{self, IsTerminal};

/// When to emit ANSI colors.
#[derive(Clone, Copy, Debug, Default, ValueEnum, PartialEq, Eq)]
pub enum ColorMode {
    /// Color when stdout is a TTY and `NO_COLOR` is unset.
    #[default]
    Auto,
    /// Always color (ignores TTY; still respects stripping only if disabled).
    Always,
    /// Never color.
    Never,
}

/// Resolved styling context for one CLI invocation.
#[derive(Clone, Debug)]
pub struct StyleCtx {
    color: bool,
    unicode: bool,
}

impl StyleCtx {
    pub fn resolve(mode: ColorMode) -> Self {
        let no_color = env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let force = env::var_os("CLICOLOR_FORCE").is_some_and(|v| v != "0");
        let ascii = env::var_os("REPOLY_ASCII").is_some_and(|v| v != "0");
        let tty = io::stdout().is_terminal();

        let color = match mode {
            ColorMode::Never => false,
            ColorMode::Always => true,
            ColorMode::Auto => force || (tty && !no_color),
        };

        // Prefer unicode symbols on color TTYs unless forced ASCII.
        let unicode = !ascii && (color || tty);

        Self { color, unicode }
    }

    /// Force a mode (tests).
    pub fn from_parts(color: bool, unicode: bool) -> Self {
        Self { color, unicode }
    }

    pub fn color_enabled(&self) -> bool {
        self.color
    }

    pub fn paint(&self, style: Style, text: &str) -> String {
        if !self.color {
            return text.to_string();
        }
        format!("{style}{text}{style:#}")
    }

    pub fn bold(&self, text: &str) -> String {
        self.paint(Style::new().effects(Effects::BOLD), text)
    }

    pub fn dim(&self, text: &str) -> String {
        self.paint(Style::new().effects(Effects::DIMMED), text)
    }

    pub fn green(&self, text: &str) -> String {
        self.paint(
            Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green))),
            text,
        )
    }

    pub fn yellow(&self, text: &str) -> String {
        self.paint(
            Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow))),
            text,
        )
    }

    pub fn red(&self, text: &str) -> String {
        self.paint(
            Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red))),
            text,
        )
    }

    pub fn cyan(&self, text: &str) -> String {
        self.paint(
            Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan))),
            text,
        )
    }

    pub fn blue(&self, text: &str) -> String {
        self.paint(
            Style::new().fg_color(Some(Color::Ansi(AnsiColor::Blue))),
            text,
        )
    }

    /// Command title, e.g. `repoly doctor`.
    pub fn header(&self, title: &str) -> String {
        self.bold(title)
    }

    /// Indented key/value meta line (`  workspace  innersync`).
    /// Pads the key *before* applying style so ANSI codes don't break alignment.
    pub fn meta_line(&self, key: &str, value: &str) -> String {
        let padded = format!("{key:<10}");
        format!("  {} {}", self.dim(&padded), value)
    }

    /// Horizontal rule under table headers.
    pub fn rule(&self, width: usize) -> String {
        let ch = if self.unicode { '─' } else { '-' };
        self.dim(&ch.to_string().repeat(width))
    }

    /// Severity badge for doctor/run/commit lines.
    pub fn badge_ok(&self) -> String {
        if self.unicode {
            self.green("✓")
        } else {
            self.green("ok  ")
        }
    }

    pub fn badge_warn(&self) -> String {
        if self.unicode {
            self.yellow("!")
        } else {
            self.yellow("warn")
        }
    }

    pub fn badge_err(&self) -> String {
        if self.unicode {
            self.red("✗")
        } else {
            self.red("err ")
        }
    }

    pub fn badge_info(&self) -> String {
        if self.unicode {
            self.cyan("i")
        } else {
            self.cyan("info")
        }
    }

    pub fn badge_skip(&self) -> String {
        if self.unicode {
            self.dim("·")
        } else {
            self.dim("skip")
        }
    }

    pub fn badge_fail(&self) -> String {
        if self.unicode {
            self.red("✗")
        } else {
            self.red("FAIL")
        }
    }

    /// Bracketed form used when unicode is off for alignment with legacy tests.
    pub fn badge_bracketed(&self, kind: BadgeKind) -> String {
        match kind {
            BadgeKind::Ok => {
                if self.unicode {
                    format!("  {} ", self.badge_ok())
                } else {
                    format!("  [{}] ", self.badge_ok())
                }
            }
            BadgeKind::Warn => {
                if self.unicode {
                    format!("  {} ", self.badge_warn())
                } else {
                    format!("  [{}] ", self.badge_warn())
                }
            }
            BadgeKind::Err => {
                if self.unicode {
                    format!("  {} ", self.badge_err())
                } else {
                    format!("  [{}] ", self.badge_err())
                }
            }
            BadgeKind::Info => {
                if self.unicode {
                    format!("  {} ", self.badge_info())
                } else {
                    format!("  [{}] ", self.badge_info())
                }
            }
            BadgeKind::Skip => {
                if self.unicode {
                    format!("  {} ", self.badge_skip())
                } else {
                    format!("  [{}] ", self.badge_skip())
                }
            }
            BadgeKind::Fail => {
                if self.unicode {
                    format!("  {} ", self.badge_fail())
                } else {
                    format!("  [{}] ", self.badge_fail())
                }
            }
        }
    }

    pub fn error_prefix(&self) -> String {
        self.red("error:")
    }

    pub fn warning_prefix(&self) -> String {
        self.yellow("warning:")
    }

    pub fn ok_prefix(&self) -> String {
        self.green("ok:")
    }

    pub fn section(&self, title: &str) -> String {
        if self.unicode {
            self.dim(&format!("── {title} ──"))
        } else {
            self.dim(&format!("-- {title} --"))
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum BadgeKind {
    Ok,
    Warn,
    Err,
    Info,
    Skip,
    Fail,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_has_no_ansi() {
        let s = StyleCtx::from_parts(false, false);
        let out = format!("{} {} {}", s.badge_ok(), s.green("x"), s.bold("y"));
        assert!(!out.contains('\u{1b}'), "expected no ANSI, got {out:?}");
    }

    #[test]
    fn always_has_ansi() {
        let s = StyleCtx::from_parts(true, true);
        assert!(s.green("x").contains('\u{1b}'));
        assert!(s.bold("y").contains('\u{1b}'));
    }
}
