//! Text buffers backed by ropey with undo/redo and multi-cursor.

use ide_core::{Cursor, IdeResult, LanguageId, language_from_path};
use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BufferId(Uuid);

impl BufferId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for BufferId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
enum Edit {
    Insert { at: usize, text: String },
    Delete { at: usize, text: String },
}

#[derive(Debug)]
pub struct Buffer {
    pub id: BufferId,
    pub path: Option<PathBuf>,
    pub language: LanguageId,
    rope: Rope,
    pub cursor: Cursor,
    pub extra_cursors: Vec<Cursor>,
    pub scroll_row: usize,
    pub dirty: bool,
    pub version: i32,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl Buffer {
    pub fn empty() -> Self {
        Self {
            id: BufferId::new(),
            path: None,
            language: LanguageId::Plain,
            rope: Rope::new(),
            cursor: Cursor::default(),
            extra_cursors: Vec::new(),
            scroll_row: 0,
            dirty: false,
            version: 0,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn from_path(path: impl AsRef<Path>) -> IdeResult<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)?;
        let mut buf = Self::from_text(text, Some(path.to_path_buf()));
        buf.language = language_from_path(path);
        buf.dirty = false;
        Ok(buf)
    }

    pub fn from_text(text: impl AsRef<str>, path: Option<PathBuf>) -> Self {
        let language = path
            .as_ref()
            .map(|p| language_from_path(p))
            .unwrap_or(LanguageId::Plain);
        let dirty = path.is_none();
        Self {
            id: BufferId::new(),
            path,
            language,
            rope: Rope::from_str(text.as_ref()),
            cursor: Cursor::default(),
            extra_cursors: Vec::new(),
            scroll_row: 0,
            dirty,
            version: 0,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn title(&self) -> String {
        match &self.path {
            Some(p) => p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("untitled")
                .to_string(),
            None => "untitled".into(),
        }
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines().max(1)
    }

    pub fn line(&self, row: usize) -> String {
        if row >= self.rope.len_lines() {
            return String::new();
        }
        let mut s = self.rope.line(row).to_string();
        if s.ends_with('\n') {
            s.pop();
            if s.ends_with('\r') {
                s.pop();
            }
        }
        s
    }

    pub fn line_len(&self, row: usize) -> usize {
        self.line(row).chars().count()
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn byte_offset(&self, cursor: Cursor) -> usize {
        let row = cursor.row.min(self.line_count().saturating_sub(1));
        let line_start = self.rope.line_to_byte(row);
        let line = self.line(row);
        let col = cursor.col.min(line.chars().count());
        let byte_col: usize = line.chars().take(col).map(|c| c.len_utf8()).sum();
        line_start + byte_col
    }

    pub fn cursor_from_byte(&self, byte: usize) -> Cursor {
        let byte = byte.min(self.rope.len_bytes());
        let row = self.rope.byte_to_line(byte);
        let line_start = self.rope.line_to_byte(row);
        let line = self.line(row);
        let mut col = 0;
        let mut used = 0;
        for ch in line.chars() {
            if used + ch.len_utf8() > byte.saturating_sub(line_start) {
                break;
            }
            used += ch.len_utf8();
            col += 1;
        }
        Cursor { row, col }
    }

    fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
        self.dirty = true;
    }

    pub fn all_cursors(&self) -> Vec<Cursor> {
        let mut c = vec![self.cursor];
        c.extend(self.extra_cursors.iter().copied());
        c.sort_by(|a, b| a.row.cmp(&b.row).then(a.col.cmp(&b.col)));
        c.dedup();
        c
    }

    pub fn add_cursor_below(&mut self) {
        let mut next = Cursor {
            row: self.cursor.row + 1,
            col: self.cursor.col,
        };
        if next.row < self.line_count() {
            next.col = next.col.min(self.line_len(next.row));
            if !self.extra_cursors.contains(&next) && next != self.cursor {
                self.extra_cursors.push(next);
            }
        }
    }

    pub fn clear_extra_cursors(&mut self) {
        self.extra_cursors.clear();
    }

    pub fn insert_char(&mut self, ch: char) {
        let mut s = String::new();
        s.push(ch);
        self.insert_str(&s);
    }

    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        // Multi-cursor: insert at each caret from bottom so offsets stay valid.
        let mut carets = self.all_cursors();
        carets.sort_by(|a, b| b.row.cmp(&a.row).then(b.col.cmp(&a.col)));
        let mut new_primary = self.cursor;
        for (i, caret) in carets.iter().enumerate() {
            let at = self.byte_offset(*caret);
            self.rope.insert(at, text);
            self.undo.push(Edit::Insert {
                at,
                text: text.to_string(),
            });
            let advanced = self.cursor_from_byte(at + text.len());
            if i == carets.len() - 1 {
                new_primary = advanced;
            }
        }
        self.redo.clear();
        self.bump_version();
        self.cursor = new_primary;
        self.extra_cursors.clear();
        self.clamp_cursor();
    }

    pub fn insert_newline(&mut self) {
        self.insert_str("\n");
    }

    pub fn backspace(&mut self) {
        let at = self.byte_offset(self.cursor);
        if at == 0 {
            return;
        }
        let prev = self.cursor_from_byte(at.saturating_sub(1));
        let start = self.byte_offset(prev);
        let text = self.rope.byte_slice(start..at).to_string();
        self.rope.remove(start..at);
        self.undo.push(Edit::Delete { at: start, text });
        self.redo.clear();
        self.bump_version();
        self.cursor = prev;
        self.extra_cursors.clear();
        self.clamp_cursor();
    }

    pub fn delete_forward(&mut self) {
        let at = self.byte_offset(self.cursor);
        if at >= self.rope.len_bytes() {
            return;
        }
        let Some(ch) = self.rope.byte_slice(at..).chars().next() else {
            return;
        };
        let end = at + ch.len_utf8();
        let text = self.rope.byte_slice(at..end).to_string();
        self.rope.remove(at..end);
        self.undo.push(Edit::Delete { at, text });
        self.redo.clear();
        self.bump_version();
    }

    pub fn replace_line_range(&mut self, row: usize, start_col: usize, end_col: usize, text: &str) {
        let line = self.line(row);
        let start_col = start_col.min(line.chars().count());
        let end_col = end_col.min(line.chars().count()).max(start_col);
        let line_start = self.rope.line_to_byte(row);
        let start_byte = line_start
            + line
                .chars()
                .take(start_col)
                .map(|c| c.len_utf8())
                .sum::<usize>();
        let end_byte = line_start
            + line
                .chars()
                .take(end_col)
                .map(|c| c.len_utf8())
                .sum::<usize>();
        if start_byte < end_byte {
            let deleted = self.rope.byte_slice(start_byte..end_byte).to_string();
            self.rope.remove(start_byte..end_byte);
            self.undo.push(Edit::Delete {
                at: start_byte,
                text: deleted,
            });
        }
        if !text.is_empty() {
            self.rope.insert(start_byte, text);
            self.undo.push(Edit::Insert {
                at: start_byte,
                text: text.to_string(),
            });
        }
        self.redo.clear();
        self.bump_version();
        self.cursor = Cursor {
            row,
            col: start_col + text.chars().count(),
        };
        self.extra_cursors.clear();
        self.clamp_cursor();
    }

    pub fn apply_text_edits(&mut self, edits: &[(u32, u32, u32, u32, String)]) {
        let mut ordered = edits.to_vec();
        ordered.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        for (sl, sc, el, ec, text) in ordered {
            if sl == el {
                self.replace_line_range(sl as usize, sc as usize, ec as usize, &text);
            } else {
                let start = self.byte_offset(Cursor {
                    row: sl as usize,
                    col: sc as usize,
                });
                let end = self.byte_offset(Cursor {
                    row: el as usize,
                    col: ec as usize,
                });
                if start < end && end <= self.rope.len_bytes() {
                    let deleted = self.rope.byte_slice(start..end).to_string();
                    self.rope.remove(start..end);
                    self.undo.push(Edit::Delete {
                        at: start,
                        text: deleted,
                    });
                }
                if !text.is_empty() {
                    self.rope.insert(start, &text);
                    self.undo.push(Edit::Insert {
                        at: start,
                        text: text.clone(),
                    });
                }
                self.redo.clear();
                self.bump_version();
                self.cursor = self.cursor_from_byte(start + text.len());
            }
        }
        self.extra_cursors.clear();
        self.clamp_cursor();
    }

    pub fn replace_all(&mut self, text: &str) {
        let old = self.rope.to_string();
        self.rope = Rope::from_str(text);
        self.undo.push(Edit::Delete { at: 0, text: old });
        self.undo.push(Edit::Insert {
            at: 0,
            text: text.to_string(),
        });
        self.redo.clear();
        self.bump_version();
        self.cursor = Cursor::default();
        self.extra_cursors.clear();
    }

    pub fn word_at_cursor(&self) -> String {
        let line = self.line(self.cursor.row);
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            return String::new();
        }
        let mut i = self.cursor.col.min(chars.len());
        if i > 0 && (i == chars.len() || !is_ident(chars[i.min(chars.len() - 1)])) {
            i = i.saturating_sub(1);
        }
        let mut start = i.min(chars.len().saturating_sub(1));
        let mut end = start;
        while start > 0 && is_ident(chars[start - 1]) {
            start -= 1;
        }
        while end < chars.len() && is_ident(chars[end]) {
            end += 1;
        }
        chars[start..end].iter().collect()
    }

    pub fn undo(&mut self) {
        let Some(edit) = self.undo.pop() else {
            return;
        };
        match &edit {
            Edit::Insert { at, text } => {
                let end = *at + text.len();
                self.rope.remove(*at..end);
                self.cursor = self.cursor_from_byte(*at);
            }
            Edit::Delete { at, text } => {
                self.rope.insert(*at, text);
                self.cursor = self.cursor_from_byte(*at + text.len());
            }
        }
        self.redo.push(edit);
        self.bump_version();
        self.clamp_cursor();
    }

    pub fn redo(&mut self) {
        let Some(edit) = self.redo.pop() else {
            return;
        };
        match &edit {
            Edit::Insert { at, text } => {
                self.rope.insert(*at, text);
                self.cursor = self.cursor_from_byte(*at + text.len());
            }
            Edit::Delete { at, text } => {
                let end = *at + text.len();
                self.rope.remove(*at..end);
                self.cursor = self.cursor_from_byte(*at);
            }
        }
        self.undo.push(edit);
        self.bump_version();
        self.clamp_cursor();
    }

    pub fn clamp_cursor(&mut self) {
        let max_row = self.line_count().saturating_sub(1);
        self.cursor.row = self.cursor.row.min(max_row);
        let max_col = self.line_len(self.cursor.row);
        self.cursor.col = self.cursor.col.min(max_col);
    }

    pub fn move_left(&mut self) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
            self.cursor.col = self.line_len(self.cursor.row);
        }
    }

    pub fn move_right(&mut self) {
        let len = self.line_len(self.cursor.row);
        if self.cursor.col < len {
            self.cursor.col += 1;
        } else if self.cursor.row + 1 < self.line_count() {
            self.cursor.row += 1;
            self.cursor.col = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor.row > 0 {
            self.cursor.row -= 1;
            self.clamp_cursor();
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor.row + 1 < self.line_count() {
            self.cursor.row += 1;
            self.clamp_cursor();
        }
    }

    pub fn move_line_start(&mut self) {
        self.cursor.col = 0;
    }

    pub fn move_line_end(&mut self) {
        self.cursor.col = self.line_len(self.cursor.row);
    }

    pub fn ensure_cursor_visible(&mut self, view_height: usize) {
        if view_height == 0 {
            return;
        }
        if self.cursor.row < self.scroll_row {
            self.scroll_row = self.cursor.row;
        } else if self.cursor.row >= self.scroll_row + view_height {
            self.scroll_row = self.cursor.row + 1 - view_height;
        }
    }

    pub fn save(&mut self) -> IdeResult<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| ide_core::IdeError::msg("buffer has no path"))?;
        std::fs::write(&path, self.text())?;
        self.dirty = false;
        Ok(())
    }

    pub fn set_path(&mut self, path: PathBuf) {
        self.language = language_from_path(&path);
        self.path = Some(path);
    }

    /// Find next occurrence of `query` after cursor. Case-insensitive.
    pub fn find_next(&mut self, query: &str) -> bool {
        if query.is_empty() {
            return false;
        }
        let q = query.to_ascii_lowercase();
        let start = self.byte_offset(self.cursor);
        let text = self.text();
        let lower = text.to_ascii_lowercase();
        let found = lower[start.saturating_add(1).min(lower.len())..]
            .find(&q)
            .map(|i| start.saturating_add(1) + i)
            .or_else(|| lower.find(&q));
        let Some(at) = found else {
            return false;
        };
        self.cursor = self.cursor_from_byte(at);
        self.clamp_cursor();
        true
    }

    pub fn find_prev(&mut self, query: &str) -> bool {
        if query.is_empty() {
            return false;
        }
        let q = query.to_ascii_lowercase();
        let start = self.byte_offset(self.cursor);
        let text = self.text();
        let lower = text.to_ascii_lowercase();
        let head = &lower[..start];
        let found = head.rfind(&q).or_else(|| lower.rfind(&q));
        let Some(at) = found else {
            return false;
        };
        self.cursor = self.cursor_from_byte(at);
        self.clamp_cursor();
        true
    }

    /// Replace the occurrence under/at cursor matching query, then find next.
    pub fn replace_one(&mut self, query: &str, replacement: &str) -> bool {
        if query.is_empty() {
            return false;
        }
        let at = self.byte_offset(self.cursor);
        let text = self.text();
        let slice = &text[at..];
        if !slice.to_ascii_lowercase().starts_with(&query.to_ascii_lowercase()) {
            if !self.find_next(query) {
                return false;
            }
        }
        let at = self.byte_offset(self.cursor);
        let end = at + query.len();
        if end > self.rope.len_bytes() {
            return false;
        }
        let deleted = self.rope.byte_slice(at..end).to_string();
        // verify case-insensitive match length using actual bytes of query in file
        let file_slice = text.get(at..).unwrap_or("");
        let mut matched = 0usize;
        let mut bytes = 0usize;
        for (qc, fc) in query.chars().zip(file_slice.chars()) {
            if qc.to_ascii_lowercase() != fc.to_ascii_lowercase() {
                break;
            }
            matched += 1;
            bytes += fc.len_utf8();
        }
        if matched != query.chars().count() {
            return false;
        }
        let end = at + bytes;
        let deleted = self.rope.byte_slice(at..end).to_string();
        self.rope.remove(at..end);
        self.undo.push(Edit::Delete { at, text: deleted });
        if !replacement.is_empty() {
            self.rope.insert(at, replacement);
            self.undo.push(Edit::Insert {
                at,
                text: replacement.to_string(),
            });
        }
        self.redo.clear();
        self.bump_version();
        self.cursor = self.cursor_from_byte(at + replacement.len());
        self.clamp_cursor();
        self.find_next(query);
        true
    }

    pub fn replace_all_matches(&mut self, query: &str, replacement: &str) -> usize {
        if query.is_empty() {
            return 0;
        }
        let mut count = 0;
        self.cursor = Cursor::default();
        while self.find_next(query) || (count == 0 && {
            // also try from start exactly
            let text = self.text().to_ascii_lowercase();
            text.starts_with(&query.to_ascii_lowercase())
        }) {
            // if first match at 0 without moving
            if count == 0 {
                let text = self.text();
                if text.to_ascii_lowercase().starts_with(&query.to_ascii_lowercase()) {
                    self.cursor = Cursor::default();
                }
            }
            if !self.replace_one(query, replacement) {
                break;
            }
            count += 1;
            if count > 10_000 {
                break;
            }
        }
        count
    }
}


fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[derive(Debug, Default)]
pub struct BufferSet {
    buffers: Vec<Buffer>,
    active: usize,
}

impl BufferSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }

    pub fn len(&self) -> usize {
        self.buffers.len()
    }

    pub fn active(&self) -> Option<&Buffer> {
        self.buffers.get(self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut Buffer> {
        self.buffers.get_mut(self.active)
    }

    pub fn get(&self, idx: usize) -> Option<&Buffer> {
        self.buffers.get(idx)
    }

    pub fn get_mut(&mut self, idx: usize) -> Option<&mut Buffer> {
        self.buffers.get_mut(idx)
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn set_active(&mut self, idx: usize) {
        if idx < self.buffers.len() {
            self.active = idx;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Buffer> {
        self.buffers.iter()
    }

    pub fn open(&mut self, buf: Buffer) -> usize {
        if let Some(path) = buf.path.clone() {
            if let Some((idx, _)) = self
                .buffers
                .iter()
                .enumerate()
                .find(|(_, b)| b.path.as_ref() == Some(&path))
            {
                self.active = idx;
                return idx;
            }
        }
        self.buffers.push(buf);
        self.active = self.buffers.len() - 1;
        self.active
    }

    pub fn close_active(&mut self) -> IdeResult<()> {
        if self.buffers.is_empty() {
            return Ok(());
        }
        if self.buffers[self.active].dirty {
            return Err(ide_core::IdeError::msg(
                "buffer is dirty; save with :w or discard with :q!",
            ));
        }
        self.buffers.remove(self.active);
        if self.active >= self.buffers.len() && self.active > 0 {
            self.active -= 1;
        }
        Ok(())
    }

    pub fn force_close_active(&mut self) {
        if self.buffers.is_empty() {
            return;
        }
        self.buffers.remove(self.active);
        if self.active >= self.buffers.len() && self.active > 0 {
            self.active -= 1;
        }
    }

    pub fn next_tab(&mut self) {
        if self.buffers.is_empty() {
            return;
        }
        self.active = (self.active + 1) % self.buffers.len();
    }

    pub fn prev_tab(&mut self) {
        if self.buffers.is_empty() {
            return;
        }
        self.active = if self.active == 0 {
            self.buffers.len() - 1
        } else {
            self.active - 1
        };
    }
}
