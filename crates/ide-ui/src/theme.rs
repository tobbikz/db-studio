//! Visual tokens — DataRig-inspired built-in themes (truecolor) + custom TOML.

#[path = "theme_custom.rs"]
mod theme_custom;

pub use theme_custom::{create_custom_theme, customs, reload_customs, themes_dir, OwnedPalette};

use ratatui::style::{Color, Modifier, Style};
use std::sync::atomic::{AtomicBool, Ordering};
use theme_custom::{active, find_custom, set_active};

/// Nerd Font glyphs in the Database studio (default on; ASCII fallback when off).
static ICONS: AtomicBool = AtomicBool::new(true);

pub fn set_icons(on: bool) {
    ICONS.store(on, Ordering::Relaxed);
}

pub fn icons_on() -> bool {
    ICONS.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub id: &'static str,
    pub label: &'static str,
    pub light: bool,
    pub bg: Color,
    pub surface: Color,
    pub surface_2: Color,
    pub surface_3: Color,
    pub modal: Color,
    pub border_idle: Color,
    pub border_focus: Color,
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    pub accent: Color,
    pub accent_warm: Color,
    pub warn: Color,
    pub danger: Color,
    pub success: Color,
    pub info: Color,
    pub syn_keyword: Color,
    pub syn_string: Color,
    pub syn_number: Color,
    pub syn_comment: Color,
    pub syn_function: Color,
    pub syn_ident: Color,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// Built-ins (DataRig-inspired names + AMOLED mono).
pub static PALETTES: &[Palette] = &[
    // 0 — AMOLED mono (legacy default)
    Palette {
        id: "black",
        label: "AMOLED mono",
        light: false,
        bg: rgb(0, 0, 0),
        surface: rgb(10, 10, 10),
        surface_2: rgb(18, 18, 18),
        surface_3: rgb(26, 26, 26),
        modal: rgb(22, 22, 22),
        border_idle: rgb(42, 42, 42),
        border_focus: rgb(200, 200, 200),
        text: rgb(230, 230, 230),
        muted: rgb(140, 140, 140),
        faint: rgb(90, 90, 90),
        accent: rgb(230, 230, 230),
        accent_warm: rgb(200, 200, 200),
        warn: rgb(180, 180, 180),
        danger: rgb(239, 68, 68),
        success: rgb(34, 197, 94),
        info: rgb(140, 140, 140),
        syn_keyword: rgb(230, 230, 230),
        syn_string: rgb(160, 160, 160),
        syn_number: rgb(210, 210, 210),
        syn_comment: rgb(90, 90, 90),
        syn_function: rgb(200, 200, 200),
        syn_ident: rgb(180, 180, 180),
    },
    // 1 — AMOLED light mono (pure white / black)
    Palette {
        id: "white",
        label: "AMOLED light mono",
        light: true,
        bg: rgb(255, 255, 255),
        surface: rgb(250, 250, 250),
        surface_2: rgb(242, 242, 242),
        surface_3: rgb(230, 230, 230),
        modal: rgb(255, 255, 255),
        border_idle: rgb(210, 210, 210),
        border_focus: rgb(40, 40, 40),
        text: rgb(16, 16, 16),
        muted: rgb(100, 100, 100),
        faint: rgb(150, 150, 150),
        accent: rgb(16, 16, 16),
        accent_warm: rgb(60, 60, 60),
        warn: rgb(80, 80, 80),
        danger: rgb(220, 38, 38),
        success: rgb(22, 163, 74),
        info: rgb(100, 100, 100),
        syn_keyword: rgb(16, 16, 16),
        syn_string: rgb(80, 80, 80),
        syn_number: rgb(40, 40, 40),
        syn_comment: rgb(150, 150, 150),
        syn_function: rgb(40, 40, 40),
        syn_ident: rgb(30, 30, 30),
    },
    // 2 — DataRig-like dark (subtle blue accent)
    Palette {
        id: "dark",
        label: "Dark",
        light: false,
        bg: rgb(16, 18, 22),
        surface: rgb(24, 26, 32),
        surface_2: rgb(32, 35, 42),
        surface_3: rgb(40, 44, 52),
        modal: rgb(28, 31, 38),
        border_idle: rgb(48, 52, 62),
        border_focus: rgb(137, 180, 250),
        text: rgb(205, 214, 244),
        muted: rgb(147, 153, 178),
        faint: rgb(88, 91, 112),
        accent: rgb(137, 180, 250),
        accent_warm: rgb(249, 226, 175),
        warn: rgb(249, 226, 175),
        danger: rgb(243, 139, 168),
        success: rgb(166, 227, 161),
        info: rgb(137, 220, 235),
        syn_keyword: rgb(203, 166, 247),
        syn_string: rgb(166, 227, 161),
        syn_number: rgb(250, 179, 135),
        syn_comment: rgb(108, 112, 134),
        syn_function: rgb(137, 180, 250),
        syn_ident: rgb(205, 214, 244),
    },
    // 2 — Nord
    Palette {
        id: "nord",
        label: "Nord",
        light: false,
        bg: rgb(46, 52, 64),
        surface: rgb(59, 66, 82),
        surface_2: rgb(67, 76, 94),
        surface_3: rgb(76, 86, 106),
        modal: rgb(59, 66, 82),
        border_idle: rgb(76, 86, 106),
        border_focus: rgb(136, 192, 208),
        text: rgb(236, 239, 244),
        muted: rgb(216, 222, 233),
        faint: rgb(129, 161, 193),
        accent: rgb(136, 192, 208),
        accent_warm: rgb(235, 203, 139),
        warn: rgb(235, 203, 139),
        danger: rgb(191, 97, 106),
        success: rgb(163, 190, 140),
        info: rgb(129, 161, 193),
        syn_keyword: rgb(129, 161, 193),
        syn_string: rgb(163, 190, 140),
        syn_number: rgb(180, 142, 173),
        syn_comment: rgb(97, 110, 136),
        syn_function: rgb(136, 192, 208),
        syn_ident: rgb(216, 222, 233),
    },
    // 3 — Catppuccin Mocha
    Palette {
        id: "catppuccin-mocha",
        label: "Catppuccin Mocha",
        light: false,
        bg: rgb(30, 30, 46),
        surface: rgb(49, 50, 68),
        surface_2: rgb(69, 71, 90),
        surface_3: rgb(88, 91, 112),
        modal: rgb(49, 50, 68),
        border_idle: rgb(69, 71, 90),
        border_focus: rgb(137, 180, 250),
        text: rgb(205, 214, 244),
        muted: rgb(186, 194, 222),
        faint: rgb(108, 112, 134),
        accent: rgb(137, 180, 250),
        accent_warm: rgb(250, 179, 135),
        warn: rgb(249, 226, 175),
        danger: rgb(243, 139, 168),
        success: rgb(166, 227, 161),
        info: rgb(148, 226, 213),
        syn_keyword: rgb(203, 166, 247),
        syn_string: rgb(166, 227, 161),
        syn_number: rgb(250, 179, 135),
        syn_comment: rgb(108, 112, 134),
        syn_function: rgb(137, 180, 250),
        syn_ident: rgb(205, 214, 244),
    },
    // 4 — Tokyo Night
    Palette {
        id: "tokyo-night",
        label: "Tokyo Night",
        light: false,
        bg: rgb(26, 27, 38),
        surface: rgb(36, 40, 59),
        surface_2: rgb(41, 46, 66),
        surface_3: rgb(59, 66, 97),
        modal: rgb(36, 40, 59),
        border_idle: rgb(59, 66, 97),
        border_focus: rgb(122, 162, 247),
        text: rgb(192, 202, 245),
        muted: rgb(169, 177, 214),
        faint: rgb(86, 95, 137),
        accent: rgb(122, 162, 247),
        accent_warm: rgb(224, 175, 104),
        warn: rgb(224, 175, 104),
        danger: rgb(247, 118, 142),
        success: rgb(158, 206, 106),
        info: rgb(125, 207, 255),
        syn_keyword: rgb(187, 154, 247),
        syn_string: rgb(158, 206, 106),
        syn_number: rgb(255, 158, 100),
        syn_comment: rgb(86, 95, 137),
        syn_function: rgb(122, 162, 247),
        syn_ident: rgb(192, 202, 245),
    },
    // 5 — Dracula
    Palette {
        id: "dracula",
        label: "Dracula",
        light: false,
        bg: rgb(40, 42, 54),
        surface: rgb(68, 71, 90),
        surface_2: rgb(98, 114, 164),
        surface_3: rgb(68, 71, 90),
        modal: rgb(68, 71, 90),
        border_idle: rgb(98, 114, 164),
        border_focus: rgb(189, 147, 249),
        text: rgb(248, 248, 242),
        muted: rgb(189, 147, 249),
        faint: rgb(98, 114, 164),
        accent: rgb(139, 233, 253),
        accent_warm: rgb(241, 250, 140),
        warn: rgb(255, 184, 108),
        danger: rgb(255, 85, 85),
        success: rgb(80, 250, 123),
        info: rgb(139, 233, 253),
        syn_keyword: rgb(255, 121, 198),
        syn_string: rgb(241, 250, 140),
        syn_number: rgb(189, 147, 249),
        syn_comment: rgb(98, 114, 164),
        syn_function: rgb(80, 250, 123),
        syn_ident: rgb(248, 248, 242),
    },
    // 6 — Light / bone
    Palette {
        id: "light",
        label: "Light",
        light: true,
        bg: rgb(250, 250, 250),
        surface: rgb(245, 245, 245),
        surface_2: rgb(235, 235, 235),
        surface_3: rgb(225, 225, 225),
        modal: rgb(255, 255, 255),
        border_idle: rgb(210, 210, 210),
        border_focus: rgb(30, 102, 245),
        text: rgb(24, 24, 27),
        muted: rgb(82, 82, 91),
        faint: rgb(161, 161, 170),
        accent: rgb(30, 102, 245),
        accent_warm: rgb(180, 83, 9),
        warn: rgb(161, 98, 7),
        danger: rgb(220, 38, 38),
        success: rgb(22, 163, 74),
        info: rgb(37, 99, 235),
        syn_keyword: rgb(109, 40, 217),
        syn_string: rgb(21, 128, 61),
        syn_number: rgb(194, 65, 12),
        syn_comment: rgb(161, 161, 170),
        syn_function: rgb(37, 99, 235),
        syn_ident: rgb(39, 39, 42),
    },
    // 7 — High contrast
    Palette {
        id: "high-contrast",
        label: "High contrast",
        light: false,
        bg: rgb(0, 0, 0),
        surface: rgb(0, 0, 0),
        surface_2: rgb(20, 20, 20),
        surface_3: rgb(40, 40, 40),
        modal: rgb(0, 0, 0),
        border_idle: rgb(120, 120, 120),
        border_focus: rgb(255, 255, 255),
        text: rgb(255, 255, 255),
        muted: rgb(200, 200, 200),
        faint: rgb(140, 140, 140),
        accent: rgb(0, 255, 255),
        accent_warm: rgb(255, 255, 0),
        warn: rgb(255, 255, 0),
        danger: rgb(255, 0, 0),
        success: rgb(0, 255, 0),
        info: rgb(0, 200, 255),
        syn_keyword: rgb(255, 255, 255),
        syn_string: rgb(0, 255, 0),
        syn_number: rgb(255, 255, 0),
        syn_comment: rgb(140, 140, 140),
        syn_function: rgb(0, 255, 255),
        syn_ident: rgb(255, 255, 255),
    },
];

/// Legacy alias kept for call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Black,
    White,
}

pub fn set_mode(mode: ThemeMode) {
    set_mode_name(match mode {
        ThemeMode::Black => "black",
        ThemeMode::White => "white",
    });
}

pub fn builtin_by_id(id: &str) -> Option<&'static Palette> {
    PALETTES.iter().find(|p| p.id == id)
}

pub fn resolve_theme_id(name: &str) -> String {
    let n = name.trim().to_ascii_lowercase();
    match n.as_str() {
        "amoled-light" | "amoled_light" | "light-mono" | "light_mono" => "white".into(),
        "bone" => "light".into(),
        "mocha" | "catppuccin" => "catppuccin-mocha".into(),
        "tokyo" | "tokyonight" => "tokyo-night".into(),
        "hc" | "contrast" => "high-contrast".into(),
        other => other.to_string(),
    }
}

pub fn set_mode_name(name: &str) {
    let idx = resolve_theme_id(name);
    if let Some(p) = builtin_by_id(&idx) {
        set_active(OwnedPalette::from_builtin(p));
        return;
    }
    if let Some(c) = find_custom(&idx) {
        set_active(c);
        return;
    }
    // Fallback dark
    if let Some(p) = builtin_by_id("dark") {
        set_active(OwnedPalette::from_builtin(p));
    }
}

pub fn current_id() -> String {
    active().id
}

pub fn current_label() -> String {
    active().label
}

/// Built-ins + custom themes for palette / docs.
pub fn list_themes() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = PALETTES
        .iter()
        .map(|p| (p.id.to_string(), p.label.to_string()))
        .collect();
    for c in customs() {
        v.push((c.id, c.label));
    }
    v
}

pub fn cycle_theme(forward: bool) -> String {
    let all = list_themes();
    if all.is_empty() {
        return "dark".into();
    }
    let cur = current_id();
    let i = all.iter().position(|(id, _)| id == &cur).unwrap_or(0);
    let next = if forward {
        (i + 1) % all.len()
    } else {
        i.checked_sub(1).unwrap_or(all.len() - 1)
    };
    let id = all[next].0.clone();
    set_mode_name(&id);
    id
}

fn light() -> bool {
    active().light
}

pub fn bg() -> Color {
    active().bg
}
pub fn surface() -> Color {
    active().surface
}
pub fn surface_2() -> Color {
    active().surface_2
}
pub fn surface_3() -> Color {
    active().surface_3
}
pub fn modal() -> Color {
    active().modal
}
pub fn scrim() -> Color {
    if light() {
        rgb(220, 220, 220)
    } else {
        rgb(0, 0, 0)
    }
}
pub fn border_idle() -> Color {
    active().border_idle
}
pub fn border_focus() -> Color {
    active().border_focus
}
pub fn text() -> Color {
    active().text
}
pub fn muted() -> Color {
    active().muted
}
pub fn faint() -> Color {
    active().faint
}
pub fn accent() -> Color {
    active().accent
}
pub fn accent_warm() -> Color {
    active().accent_warm
}
pub fn warn() -> Color {
    active().warn
}
pub fn error_c() -> Color {
    active().danger
}
pub fn info() -> Color {
    active().info
}
pub fn success() -> Color {
    active().success
}
pub fn danger() -> Color {
    active().danger
}

pub fn style_ok() -> Style {
    Style::default()
        .fg(success())
        .add_modifier(Modifier::BOLD)
}
pub fn style_err() -> Style {
    Style::default()
        .fg(danger())
        .add_modifier(Modifier::BOLD)
}
pub fn style_ok_badge() -> Style {
    Style::default()
        .fg(contrast_on(success()))
        .bg(success())
        .add_modifier(Modifier::BOLD)
}
pub fn style_err_badge() -> Style {
    Style::default()
        .fg(contrast_on(danger()))
        .bg(danger())
        .add_modifier(Modifier::BOLD)
}
pub fn code_pill() -> Color {
    accent()
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
pub fn style_warn() -> Style {
    Style::default().fg(warn()).add_modifier(Modifier::BOLD)
}
pub fn style_title(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(muted())
    }
}
pub fn style_selected() -> Style {
    // Soft row selection (Explorer / lists) — never a full reverse-video wash.
    Style::default()
        .fg(text())
        .bg(surface_3())
        .add_modifier(Modifier::BOLD)
}

/// Strong inverted chip (badges / running) — not for list rows.
pub fn style_selected_strong() -> Style {
    Style::default()
        .fg(contrast_on(accent()))
        .bg(accent())
        .add_modifier(Modifier::BOLD)
}
pub fn style_pill_on(bg_c: Color) -> Style {
    Style::default()
        .fg(contrast_on(bg_c))
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

/// Pick readable fg for a filled chip (light/dark heuristic).
fn contrast_on(bg_c: Color) -> Color {
    match bg_c {
        Color::Rgb(r, g, b) => {
            let lum = (u16::from(r) * 299 + u16::from(g) * 587 + u16::from(b) * 114) / 1000;
            if lum > 140 {
                Color::Rgb(16, 16, 16)
            } else {
                Color::Rgb(245, 245, 245)
            }
        }
        _ => {
            if light() {
                Color::White
            } else {
                Color::Black
            }
        }
    }
}

pub fn syn_keyword() -> Color {
    active().syn_keyword
}
pub fn syn_string() -> Color {
    active().syn_string
}
pub fn syn_number() -> Color {
    active().syn_number
}
pub fn syn_comment() -> Color {
    active().syn_comment
}
pub fn syn_function() -> Color {
    active().syn_function
}
pub fn syn_ident() -> Color {
    active().syn_ident
}
