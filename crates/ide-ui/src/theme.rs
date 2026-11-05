//! tui-ide visual tokens — AMOLED black / white mono (host chrome).

use ratatui::style::{Color, Modifier, Style};
use std::sync::atomic::{AtomicU8, Ordering};

/// 0 = AMOLED black (default), 1 = white mono.
static MODE: AtomicU8 = AtomicU8::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Black,
    White,
}

pub fn set_mode(mode: ThemeMode) {
    MODE.store(
        match mode {
            ThemeMode::Black => 0,
            ThemeMode::White => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn set_mode_name(name: &str) {
    let n = name.trim().to_ascii_lowercase();
    set_mode(match n.as_str() {
        "white" | "light" | "bone" => ThemeMode::White,
        _ => ThemeMode::Black,
    });
}

fn white() -> bool {
    MODE.load(Ordering::Relaxed) == 1
}

pub fn bg() -> Color {
    if white() {
        Color::Rgb(255, 255, 255)
    } else {
        Color::Rgb(0, 0, 0)
    }
}
pub fn surface() -> Color {
    if white() {
        Color::Rgb(245, 245, 245)
    } else {
        Color::Rgb(10, 10, 10)
    }
}
pub fn surface_2() -> Color {
    if white() {
        Color::Rgb(235, 235, 235)
    } else {
        Color::Rgb(18, 18, 18)
    }
}
/// Elevated modal / dialog plate (reads above chrome).
pub fn modal() -> Color {
    if white() {
        Color::Rgb(255, 255, 255)
    } else {
        Color::Rgb(32, 32, 32)
    }
}
/// Scrim behind modals (dims the studio chrome).
pub fn scrim() -> Color {
    if white() {
        Color::Rgb(220, 220, 220)
    } else {
        Color::Rgb(0, 0, 0)
    }
}
pub fn border_idle() -> Color {
    if white() {
        Color::Rgb(210, 210, 210)
    } else {
        Color::Rgb(42, 42, 42)
    }
}
pub fn border_focus() -> Color {
    if white() {
        Color::Rgb(40, 40, 40)
    } else {
        Color::Rgb(220, 220, 220)
    }
}
pub fn text() -> Color {
    if white() {
        Color::Rgb(16, 16, 16)
    } else {
        Color::Rgb(230, 230, 230)
    }
}
pub fn muted() -> Color {
    if white() {
        Color::Rgb(100, 100, 100)
    } else {
        Color::Rgb(140, 140, 140)
    }
}
pub fn faint() -> Color {
    if white() {
        Color::Rgb(150, 150, 150)
    } else {
        Color::Rgb(90, 90, 90)
    }
}
pub fn accent() -> Color {
    text()
}
pub fn warn() -> Color {
    muted()
}
pub fn error_c() -> Color {
    text()
}
pub fn info() -> Color {
    muted()
}
pub fn code_pill() -> Color {
    text()
}
pub fn db_pill() -> Color {
    muted()
}

pub fn style_text() -> Style {
    Style::default().fg(text())
}
pub fn style_muted() -> Style {
    Style::default().fg(muted())
}
pub fn style_faint() -> Style {
    Style::default().fg(faint())
}
pub fn style_accent() -> Style {
    Style::default().fg(accent())
}
pub fn style_selected() -> Style {
    if white() {
        Style::default()
            .fg(Color::White)
            .bg(text())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(text())
            .add_modifier(Modifier::BOLD)
    }
}
pub fn style_pill_on(bg_c: Color) -> Style {
    let fg = if white() { Color::White } else { Color::Black };
    Style::default()
        .fg(fg)
        .bg(bg_c)
        .add_modifier(Modifier::BOLD)
}
pub fn style_pill_off() -> Style {
    Style::default().fg(muted()).bg(surface_2())
}
pub fn border(focused: bool) -> Color {
    if focused {
        border_focus()
    } else {
        border_idle()
    }
}
