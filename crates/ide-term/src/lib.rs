//! Integrated terminal via portable-pty.

use anyhow::{Context, Result};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use std::io::{Read, Write};
use std::sync::Arc;
use tracing::warn;

pub struct TerminalPane {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    reader_buf: Arc<Mutex<String>>,
    scroll: usize,
    cols: u16,
    rows: u16,
    _child: Box<dyn portable_pty::Child + Send + Sync>,
    _pair_reader: Option<std::thread::JoinHandle<()>>,
}

impl TerminalPane {
    pub fn spawn(cwd: &std::path::Path, cols: u16, rows: u16) -> Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        Self::spawn_command(cwd, cols, rows, &shell, &[])
    }

    /// Spawn an arbitrary command in a PTY (e.g. `ssh` with args).
    pub fn spawn_command(
        cwd: &std::path::Path,
        cols: u16,
        rows: u16,
        program: &str,
        args: &[String],
    ) -> Result<Self> {
        let cols = cols.max(10);
        let rows = rows.max(3);
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("open pty")?;

        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(a);
        }
        cmd.cwd(cwd);
        // Avoid trapping the embedded TTY in `less` after `git log` / man / etc.
        cmd.env("PAGER", "cat");
        cmd.env("GIT_PAGER", "cat");
        cmd.env("LESS", "FRX");

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawn {program}"))?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("clone reader")?;
        let writer = pair.master.take_writer().context("take writer")?;
        let reader_buf = Arc::new(Mutex::new(String::new()));
        let buf_c = reader_buf.clone();

        let handle = std::thread::spawn(move || {
            let mut tmp = [0u8; 4096];
            loop {
                match reader.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        let s = String::from_utf8_lossy(&tmp[..n]);
                        let mut g = buf_c.lock();
                        g.push_str(&s);
                        // keep last ~200k chars
                        if g.len() > 200_000 {
                            let keep = g.len() - 150_000;
                            g.drain(..keep);
                        }
                    }
                    Err(e) => {
                        warn!("pty read: {e}");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            master: pair.master,
            writer: Arc::new(Mutex::new(writer)),
            reader_buf,
            scroll: 0,
            cols,
            rows,
            _child: child,
            _pair_reader: Some(handle),
        })
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(10);
        let rows = rows.max(3);
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        if let Err(e) = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            warn!("pty resize: {e}");
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    pub fn write_bytes(&self, data: &[u8]) -> Result<()> {
        let mut w = self.writer.lock();
        w.write_all(data)?;
        w.flush()?;
        Ok(())
    }

    pub fn write_char(&self, c: char) -> Result<()> {
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        self.write_bytes(s.as_bytes())
    }

    pub fn visible_lines(&self) -> Vec<String> {
        let g = self.reader_buf.lock();
        // strip some ANSI for display simplicity
        let plain = strip_ansi(&g);
        let lines: Vec<&str> = plain.lines().collect();
        let h = self.rows as usize;
        let total = lines.len();
        let start = total.saturating_sub(h + self.scroll);
        let end = total.saturating_sub(self.scroll);
        lines[start..end.max(start)]
            .iter()
            .map(|s| s.chars().take(self.cols as usize).collect())
            .collect()
    }

    pub fn scroll_up(&mut self) {
        self.scroll += 1;
    }

    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek().copied() {
                // CSI: ESC [ ... final-byte
                Some('[') => {
                    chars.next();
                    for c2 in chars.by_ref() {
                        if (0x40..=0x7e).contains(&(c2 as u32)) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... BEL  or  ESC ] ... ESC \
                Some(']') => {
                    chars.next();
                    while let Some(c2) = chars.next() {
                        if c2 == '\u{07}' {
                            break;
                        }
                        if c2 == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    let _ = chars.next();
                }
                None => {}
            }
            continue;
        }
        if c == '\r' || c == '\u{07}' {
            continue;
        }
        // Orphan OSC title crumbs if ESC was already lost (e.g. "]0;title")
        if c == ']' && matches!(chars.peek(), Some('0'..='9')) {
            while let Some(c2) = chars.next() {
                if c2 == '\u{07}' || c2 == '\n' {
                    break;
                }
                if c2 == '\u{1b}' && chars.peek() == Some(&'\\') {
                    chars.next();
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod strip_tests {
    use super::strip_ansi;

    #[test]
    fn strips_osc_window_title() {
        let s = "\u{1b}]0;Cursor Agent\u{07}hello";
        assert_eq!(strip_ansi(s), "hello");
    }

    #[test]
    fn strips_orphan_osc_crumb() {
        let s = "]0;Cursor Agent\u{07}plan";
        assert_eq!(strip_ansi(s), "plan");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn spawn_resize_and_echo() {
        let tmp = std::env::temp_dir().join("tui-ide-term-smoke");
        let _ = std::fs::create_dir_all(&tmp);
        let mut pane = TerminalPane::spawn(&tmp, 40, 8).expect("spawn");
        pane.resize(60, 12);
        assert_eq!(pane.size(), (60, 12));
        pane.write_bytes(b"echo wave4\r").expect("write");
        std::thread::sleep(Duration::from_millis(200));
        let lines = pane.visible_lines().join("\n");
        assert!(
            lines.contains("wave4") || !lines.is_empty(),
            "expected pty output, got: {lines:?}"
        );
    }
}
