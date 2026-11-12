//! User themes: `~/.config/tui-ide/themes/<name>.toml` (DataRig-inspired).

use super::Palette;
use anyhow::{bail, Context, Result};
use parking_lot::Mutex;
use ratatui::style::Color;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct OwnedPalette {
    pub id: String,
    pub label: String,
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

impl OwnedPalette {
    pub fn from_builtin(p: &Palette) -> Self {
        Self {
            id: p.id.into(),
            label: p.label.into(),
            light: p.light,
            bg: p.bg,
            surface: p.surface,
            surface_2: p.surface_2,
            surface_3: p.surface_3,
            modal: p.modal,
            border_idle: p.border_idle,
            border_focus: p.border_focus,
            text: p.text,
            muted: p.muted,
            faint: p.faint,
            accent: p.accent,
            accent_warm: p.accent_warm,
            warn: p.warn,
            danger: p.danger,
            success: p.success,
            info: p.info,
            syn_keyword: p.syn_keyword,
            syn_string: p.syn_string,
            syn_number: p.syn_number,
            syn_comment: p.syn_comment,
            syn_function: p.syn_function,
            syn_ident: p.syn_ident,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ThemeFile {
    #[serde(default)]
    extends: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    light: Option<bool>,
    #[serde(default)]
    colors: ThemeColors,
}

#[derive(Debug, Default, Deserialize)]
struct ThemeColors {
    bg: Option<String>,
    surface: Option<String>,
    surface_2: Option<String>,
    surface_3: Option<String>,
    modal: Option<String>,
    border: Option<String>,
    border_idle: Option<String>,
    border_focus: Option<String>,
    accent: Option<String>,
    accent_warm: Option<String>,
    fg: Option<String>,
    text: Option<String>,
    fg_muted: Option<String>,
    muted: Option<String>,
    fg_dim: Option<String>,
    faint: Option<String>,
    warn: Option<String>,
    warning: Option<String>,
    danger: Option<String>,
    error: Option<String>,
    success: Option<String>,
    info: Option<String>,
    syn_keyword: Option<String>,
    syn_string: Option<String>,
    syn_number: Option<String>,
    syn_comment: Option<String>,
    syn_function: Option<String>,
    syn_ident: Option<String>,
    syn_identifier: Option<String>,
}

static CUSTOMS: Mutex<Vec<OwnedPalette>> = Mutex::new(Vec::new());
static ACTIVE: Mutex<Option<OwnedPalette>> = Mutex::new(None);

pub fn themes_dir() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("tui-ide");
    p.push("themes");
    Some(p)
}

pub fn reload_customs() {
    let mut out = Vec::new();
    let Some(dir) = themes_dir() else {
        *CUSTOMS.lock() = out;
        return;
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        *CUSTOMS.lock() = out;
        return;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if stem.is_empty() || !valid_theme_name(&stem) {
            continue;
        }
        if super::builtin_by_id(&stem).is_some() {
            continue; // never shadow built-ins
        }
        match load_theme_file(&path, &stem) {
            Ok(p) => out.push(p),
            Err(e) => eprintln!("db-studio theme {}: {e:#}", path.display()),
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    *CUSTOMS.lock() = out;
}

pub fn customs() -> Vec<OwnedPalette> {
    CUSTOMS.lock().clone()
}

pub fn find_custom(id: &str) -> Option<OwnedPalette> {
    CUSTOMS.lock().iter().find(|p| p.id == id).cloned()
}

pub fn set_active(p: OwnedPalette) {
    *ACTIVE.lock() = Some(p);
}

pub fn active() -> OwnedPalette {
    if let Some(a) = ACTIVE.lock().clone() {
        return a;
    }
    let p = super::builtin_by_id("dark")
        .or_else(|| super::builtin_by_id("black"))
        .expect("builtins");
    let owned = OwnedPalette::from_builtin(p);
    *ACTIVE.lock() = Some(owned.clone());
    owned
}

pub fn valid_theme_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty()
        && n.len() <= 64
        && n
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && super::builtin_by_id(n).is_none()
}

/// Write `themes/<name>.toml` extending `extends` (built-in id). Does not overwrite.
pub fn create_custom_theme(name: &str, extends: &str) -> Result<PathBuf> {
    let name = name.trim();
    if !valid_theme_name(name) {
        bail!("invalid theme name (letters/digits/-/_ , not a built-in)");
    }
    let base = super::builtin_by_id(extends).or_else(|| super::builtin_by_id("dark"));
    let Some(base) = base else {
        bail!("unknown extends theme");
    };
    let dir = themes_dir().context("no config dir")?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.toml"));
    if path.exists() {
        bail!("theme file already exists: {}", path.display());
    }
    let body = format!(
        r#"# DB Studio custom theme — edit colours, then : Theme: reload customs
# or restart. Use theme = "{name}" in config.toml
extends = "{ext}"
label = "{name}"

[colors]
# Uncomment to override (hex #rrggbb):
# accent = "{accent}"
# bg = "{bg}"
# fg = "{fg}"
# surface = "{surface}"
# border_focus = "{border}"
# syn_keyword = "{kw}"
"#,
        name = name,
        ext = base.id,
        accent = color_hex(base.accent),
        bg = color_hex(base.bg),
        fg = color_hex(base.text),
        surface = color_hex(base.surface),
        border = color_hex(base.border_focus),
        kw = color_hex(base.syn_keyword),
    );
    std::fs::write(&path, body)?;
    reload_customs();
    Ok(path)
}

fn load_theme_file(path: &Path, id: &str) -> Result<OwnedPalette> {
    let text = std::fs::read_to_string(path)?;
    let file: ThemeFile = toml::from_str(&text)?;
    let ext = file
        .extends
        .as_deref()
        .unwrap_or("dark")
        .trim()
        .to_ascii_lowercase();
    let base = super::builtin_by_id(&ext)
        .or_else(|| super::builtin_by_id("dark"))
        .context("extends")?;
    let mut p = OwnedPalette::from_builtin(base);
    p.id = id.to_string();
    p.label = file
        .label
        .unwrap_or_else(|| format!("{id} (custom)"));
    if let Some(l) = file.light {
        p.light = l;
    }
    let c = file.colors;
    apply_opt(&mut p.bg, c.bg.as_deref());
    apply_opt(&mut p.surface, c.surface.as_deref());
    apply_opt(&mut p.surface_2, c.surface_2.as_deref());
    apply_opt(&mut p.surface_3, c.surface_3.as_deref());
    apply_opt(&mut p.modal, c.modal.as_deref());
    apply_opt(
        &mut p.border_idle,
        c.border_idle.as_deref().or(c.border.as_deref()),
    );
    apply_opt(&mut p.border_focus, c.border_focus.as_deref());
    apply_opt(&mut p.accent, c.accent.as_deref());
    apply_opt(&mut p.accent_warm, c.accent_warm.as_deref());
    apply_opt(&mut p.text, c.text.as_deref().or(c.fg.as_deref()));
    apply_opt(
        &mut p.muted,
        c.muted.as_deref().or(c.fg_muted.as_deref()),
    );
    apply_opt(
        &mut p.faint,
        c.faint.as_deref().or(c.fg_dim.as_deref()),
    );
    apply_opt(&mut p.warn, c.warn.as_deref().or(c.warning.as_deref()));
    apply_opt(
        &mut p.danger,
        c.danger.as_deref().or(c.error.as_deref()),
    );
    apply_opt(&mut p.success, c.success.as_deref());
    apply_opt(&mut p.info, c.info.as_deref());
    apply_opt(&mut p.syn_keyword, c.syn_keyword.as_deref());
    apply_opt(&mut p.syn_string, c.syn_string.as_deref());
    apply_opt(&mut p.syn_number, c.syn_number.as_deref());
    apply_opt(&mut p.syn_comment, c.syn_comment.as_deref());
    apply_opt(&mut p.syn_function, c.syn_function.as_deref());
    apply_opt(
        &mut p.syn_ident,
        c.syn_ident.as_deref().or(c.syn_identifier.as_deref()),
    );
    Ok(p)
}

fn apply_opt(slot: &mut Color, raw: Option<&str>) {
    if let Some(s) = raw {
        if let Some(c) = parse_color(s) {
            *slot = c;
        }
    }
}

fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
    }
    match s.to_ascii_lowercase().as_str() {
        "black" => Some(Color::Black),
        "white" => Some(Color::White),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        _ => None,
    }
}

fn color_hex(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "#89b4fa".into(),
    }
}
