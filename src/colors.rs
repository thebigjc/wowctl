//! Color output utilities.

use owo_colors::OwoColorize;
use std::sync::atomic::{AtomicBool, Ordering};

static COLORS_ENABLED: AtomicBool = AtomicBool::new(true);

/// Sets whether colors should be used globally.
pub fn set_colors_enabled(enabled: bool) {
    COLORS_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Returns whether colors are currently enabled.
pub fn colors_enabled() -> bool {
    COLORS_ENABLED.load(Ordering::Relaxed)
}

/// Extension trait for conditional coloring.
pub trait ColorExt {
    fn color_cyan(&self) -> String;
    fn color_green(&self) -> String;
    fn color_red(&self) -> String;
    fn color_yellow(&self) -> String;
    fn color_blue(&self) -> String;
    fn color_dimmed(&self) -> String;
    fn color_bold(&self) -> String;
}

impl<T: ToString> ColorExt for T {
    fn color_cyan(&self) -> String {
        let s = self.to_string();
        if colors_enabled() {
            format!("{}", s.cyan())
        } else {
            s
        }
    }

    fn color_green(&self) -> String {
        let s = self.to_string();
        if colors_enabled() {
            format!("{}", s.green())
        } else {
            s
        }
    }

    fn color_red(&self) -> String {
        let s = self.to_string();
        if colors_enabled() {
            format!("{}", s.red())
        } else {
            s
        }
    }

    fn color_yellow(&self) -> String {
        let s = self.to_string();
        if colors_enabled() {
            format!("{}", s.yellow())
        } else {
            s
        }
    }

    fn color_blue(&self) -> String {
        let s = self.to_string();
        if colors_enabled() {
            format!("{}", s.blue())
        } else {
            s
        }
    }

    fn color_dimmed(&self) -> String {
        let s = self.to_string();
        if colors_enabled() {
            format!("{}", s.dimmed())
        } else {
            s
        }
    }

    fn color_bold(&self) -> String {
        let s = self.to_string();
        if colors_enabled() {
            format!("{}", s.bold())
        } else {
            s
        }
    }
}
