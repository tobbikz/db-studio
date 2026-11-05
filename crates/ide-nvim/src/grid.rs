//! Neovim screen grid state (ext_linegrid).

use rmpv::Value;

#[derive(Debug, Clone, Copy, Default)]
pub struct CellAttr {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
    pub fg: Option<(u8, u8, u8)>,
    pub bg: Option<(u8, u8, u8)>,
}

#[derive(Debug, Clone)]
pub struct NvimCell {
    pub text: String,
    pub attr_id: u64,
}

impl Default for NvimCell {
    fn default() -> Self {
        Self {
            text: " ".into(),
            attr_id: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NvimGrid {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<NvimCell>, // row-major
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub attrs: Vec<CellAttr>, // index = hl_id
    pub dirty: bool,
    /// Last `mode_change` name from Neovim UI (n/i/v/c/…).
    pub mode: String,
}

impl NvimGrid {
    pub fn cell(&self, row: usize, col: usize) -> Option<&NvimCell> {
        if row >= self.height || col >= self.width {
            return None;
        }
        self.cells.get(row * self.width + col)
    }

    pub fn attr(&self, id: u64) -> CellAttr {
        self.attrs
            .get(id as usize)
            .copied()
            .unwrap_or_default()
    }
}

pub(crate) struct GridInner {
    width: usize,
    height: usize,
    cells: Vec<NvimCell>,
    cursor_row: usize,
    cursor_col: usize,
    attrs: Vec<CellAttr>,
    dirty: bool,
    mode: String,
}

impl GridInner {
    pub fn new(width: usize, height: usize) -> Self {
        let n = width.saturating_mul(height).max(1);
        Self {
            width,
            height,
            cells: vec![NvimCell::default(); n],
            cursor_row: 0,
            cursor_col: 0,
            attrs: vec![CellAttr::default()],
            dirty: true,
            mode: "n".into(),
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.cells = vec![NvimCell::default(); self.width * self.height];
        self.dirty = true;
    }

    pub fn snapshot(&self) -> NvimGrid {
        NvimGrid {
            width: self.width,
            height: self.height,
            cells: self.cells.clone(),
            cursor_row: self.cursor_row,
            cursor_col: self.cursor_col,
            attrs: self.attrs.clone(),
            dirty: self.dirty,
            mode: self.mode.clone(),
        }
    }

    pub fn set_mode(&mut self, mode: impl Into<String>) {
        self.mode = mode.into();
        self.dirty = true;
    }

    pub fn apply_event(&mut self, name: &str, params: &[Value]) {
        match name {
            "grid_resize" => {
                let grid = params.first().and_then(|v| v.as_u64()).unwrap_or(1);
                if grid != 1 {
                    return;
                }
                let w = params.get(1).and_then(|v| v.as_u64()).unwrap_or(80) as usize;
                let h = params.get(2).and_then(|v| v.as_u64()).unwrap_or(24) as usize;
                self.resize(w, h);
            }
            "grid_clear" => {
                let grid = params.first().and_then(|v| v.as_u64()).unwrap_or(1);
                if grid != 1 {
                    return;
                }
                for c in &mut self.cells {
                    *c = NvimCell::default();
                }
                self.dirty = true;
            }
            "grid_cursor_goto" => {
                let grid = params.first().and_then(|v| v.as_u64()).unwrap_or(1);
                if grid != 1 {
                    return;
                }
                self.cursor_row = params.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                self.cursor_col = params.get(2).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                self.dirty = true;
            }
            "grid_line" => {
                let grid = params.first().and_then(|v| v.as_u64()).unwrap_or(1);
                if grid != 1 {
                    return;
                }
                let row = params.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let col_start = params.get(2).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let cells = params.get(3).and_then(|v| v.as_array());
                let Some(cells) = cells else {
                    return;
                };
                let mut col = col_start;
                let mut hl: u64 = 0;
                for cell in cells {
                    let Value::Array(parts) = cell else {
                        continue;
                    };
                    let text = parts
                        .first()
                        .and_then(|v| v.as_str())
                        .unwrap_or(" ")
                        .to_string();
                    if let Some(h) = parts.get(1).and_then(|v| v.as_u64()) {
                        hl = h;
                    }
                    let repeat = parts.get(2).and_then(|v| v.as_u64()).unwrap_or(1) as usize;
                    for _ in 0..repeat {
                        if row < self.height && col < self.width {
                            let idx = row * self.width + col;
                            self.cells[idx] = NvimCell {
                                text: text.clone(),
                                attr_id: hl,
                            };
                        }
                        col += 1;
                    }
                }
                self.dirty = true;
            }
            "grid_scroll" => {
                let grid = params.first().and_then(|v| v.as_u64()).unwrap_or(1);
                if grid != 1 {
                    return;
                }
                // [grid, top, bot, left, right, rows, cols]
                let top = params.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let bot = params.get(2).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let left = params.get(3).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let right = params.get(4).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let rows = params.get(5).and_then(|v| v.as_i64()).unwrap_or(0);
                // cols unused for now
                if rows == 0 || self.width == 0 {
                    return;
                }
                let height = bot.saturating_sub(top);
                let width = right.saturating_sub(left);
                if height == 0 || width == 0 {
                    return;
                }
                let mut tmp = vec![NvimCell::default(); height * width];
                for r in 0..height {
                    for c in 0..width {
                        let src_r = top + r;
                        let src_c = left + c;
                        if src_r < self.height && src_c < self.width {
                            tmp[r * width + c] = self.cells[src_r * self.width + src_c].clone();
                        }
                    }
                }
                let shift = rows.unsigned_abs() as usize;
                if rows > 0 {
                    // scroll up: content moves up
                    for r in 0..height {
                        let src = r + shift;
                        for c in 0..width {
                            let dest_r = top + r;
                            let dest_c = left + c;
                            if dest_r < self.height && dest_c < self.width {
                                let cell = if src < height {
                                    tmp[src * width + c].clone()
                                } else {
                                    NvimCell::default()
                                };
                                self.cells[dest_r * self.width + dest_c] = cell;
                            }
                        }
                    }
                } else {
                    for r in (0..height).rev() {
                        let src = r as isize - shift as isize;
                        for c in 0..width {
                            let dest_r = top + r;
                            let dest_c = left + c;
                            if dest_r < self.height && dest_c < self.width {
                                let cell = if src >= 0 {
                                    tmp[src as usize * width + c].clone()
                                } else {
                                    NvimCell::default()
                                };
                                self.cells[dest_r * self.width + dest_c] = cell;
                            }
                        }
                    }
                }
                self.dirty = true;
            }
            "hl_attr_define" => {
                let id = params.first().and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let rgb = params.get(1).and_then(|v| v.as_map());
                let mut attr = CellAttr::default();
                if let Some(map) = rgb {
                    for (k, v) in map {
                        let key = k.as_str().unwrap_or("");
                        match key {
                            "foreground" => {
                                if let Some(n) = v.as_u64() {
                                    attr.fg = Some(rgb_u64(n));
                                }
                            }
                            "background" => {
                                if let Some(n) = v.as_u64() {
                                    attr.bg = Some(rgb_u64(n));
                                }
                            }
                            "bold" => attr.bold = v.as_bool().unwrap_or(false),
                            "italic" => attr.italic = v.as_bool().unwrap_or(false),
                            "underline" => attr.underline = v.as_bool().unwrap_or(false),
                            "reverse" => attr.reverse = v.as_bool().unwrap_or(false),
                            _ => {}
                        }
                    }
                }
                if id >= self.attrs.len() {
                    self.attrs.resize(id + 1, CellAttr::default());
                }
                self.attrs[id] = attr;
            }
            "mode_change" => {
                if let Some(m) = params.first().and_then(|v| v.as_str()) {
                    self.mode = m.to_string();
                    self.dirty = true;
                }
            }
            "flush" | "default_colors_set" | "hl_group_set" | "mode_info_set"
            | "grid_destroy" | "win_viewport" | "tabline_update" | "msg_show" | "msg_clear"
            | "cmdline_show" | "cmdline_hide" | "mouse_on" | "mouse_off" | "busy_start"
            | "busy_stop" | "option_set" | "update_menu" | "set_icon" | "set_title" => {
                if name == "flush" {
                    self.dirty = true;
                }
            }
            _ => {}
        }
    }
}

fn rgb_u64(n: u64) -> (u8, u8, u8) {
    let r = ((n >> 16) & 0xff) as u8;
    let g = ((n >> 8) & 0xff) as u8;
    let b = (n & 0xff) as u8;
    (r, g, b)
}
