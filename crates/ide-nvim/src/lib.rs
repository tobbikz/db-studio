//! Neovim embedder: UI protocol frontend so AstroNvim / Neovim plugins run for real.
//!
//! Architecture: `nvim --embed` + `nvim_ui_attach` (`ext_linegrid`). We paint the
//! grid in ratatui; Database mode stays native in tui-ide.

mod grid;
mod launch;

pub use grid::{CellAttr, NvimCell, NvimGrid};
pub use launch::{NvimLaunchConfig, detect_astronvim, resolve_nvim_bin};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use grid::GridInner;
use nvim_rs::compat::tokio::Compat;
use nvim_rs::create::tokio as create;
use nvim_rs::{Handler, Neovim, UiAttachOptions, Value};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::{ChildStdin, Command};
use tokio::task::JoinHandle;
use tracing::{debug, info};

/// Listed Neovim buffer for host chrome tabs.
#[derive(Debug, Clone)]
pub struct NvimBufInfo {
    pub id: i64,
    pub name: String,
    pub current: bool,
}

/// Shared session handle used by the TUI.
pub struct NvimSession {
    nvim: Neovim<Compat<ChildStdin>>,
    grid: Arc<Mutex<GridInner>>,
    width: u64,
    height: u64,
    /// When true, next `resize` always calls `ui_try_resize` even if dims match.
    force_resize: bool,
    pub launch: NvimLaunchConfig,
    _io: JoinHandle<Result<(), Box<nvim_rs::error::LoopError>>>,
    child: tokio::process::Child,
}

#[derive(Clone)]
struct NvimHandler {
    grid: Arc<Mutex<GridInner>>,
}

#[async_trait]
impl Handler for NvimHandler {
    type Writer = Compat<ChildStdin>;

    async fn handle_notify(
        &self,
        name: String,
        args: Vec<Value>,
        _neovim: Neovim<Compat<ChildStdin>>,
    ) {
        if name != "redraw" {
            debug!("nvim notify: {name}");
            return;
        }
        // redraw args: array of [event_name, [params...], [params...], ...]
        let mut g = self.grid.lock();
        for batch in args {
            let Value::Array(ev) = batch else {
                continue;
            };
            if ev.is_empty() {
                continue;
            }
            let Some(name) = ev[0].as_str() else {
                continue;
            };
            for tuple in ev.iter().skip(1) {
                let Value::Array(params) = tuple else {
                    continue;
                };
                g.apply_event(name, params);
            }
            // Events with no param tuples (e.g. flush)
            if ev.len() == 1 {
                g.apply_event(name, &[]);
            }
        }
    }
}

impl NvimSession {
    pub async fn start(cwd: &Path, cols: u16, rows: u16, launch: NvimLaunchConfig) -> Result<Self> {
        let bin = resolve_nvim_bin(&launch)?;
        let cols = cols.max(20) as u64;
        let rows = rows.max(5) as u64;

        let grid = Arc::new(Mutex::new(GridInner::new(cols as usize, rows as usize)));
        let handler = NvimHandler {
            grid: grid.clone(),
        };

        let mut cmd = Command::new(&bin);
        cmd.arg("--embed");
        // Hide Neovim intro ("Nvim is open source…", Uganda, :help links).
        cmd.args(["--cmd", "set shortmess+=I"]);
        for a in &launch.extra_args {
            cmd.arg(a);
        }
        cmd.current_dir(cwd);
        // Leave stderr inherited so a full pipe buffer cannot stall nvim.
        cmd.stderr(Stdio::inherit());

        if let Some(app) = &launch.appname {
            cmd.env("NVIM_APPNAME", app);
            info!("starting nvim with NVIM_APPNAME={app}");
        }
        for (k, v) in &launch.env {
            cmd.env(k, v);
        }
        cmd.env_remove("VIMINIT");

        let (nvim, io_handler, child) = create::new_child_cmd(&mut cmd, handler)
            .await
            .map_err(|e| anyhow::anyhow!("spawn nvim --embed: {e}"))?;

        let mut opts = UiAttachOptions::new();
        opts.set_rgb(true);
        opts.set_linegrid_external(true);

        nvim.ui_attach(cols as i64, rows as i64, &opts)
            .await
            .map_err(|e| anyhow::anyhow!("ui_attach: {e}"))?;

        // Line numbers / cursorline; cwd = workspace. Astro (if present) opens neo-tree itself.
        let is_astro =
            launch.astronvim_detected || launch.appname.as_deref() == Some("astronvim");
        let boot = if is_astro {
            // laststatus/tabline stay on so Astro heirline looks like an IDE inside the grid.
            format!(
                "silent! set shortmess+=I mouse=a number relativenumber \
                 signcolumn=yes cursorline laststatus=3 showtabline=0 \
                 cmdheight=0 noshowmode noruler \
                 | silent! lcd {} \
                 | lua vim.defer_fn(function() pcall(function() \
                     require('neo-tree.command').execute({{action='focus',source='filesystem',\
                     position='left',reveal=true}}) end) end, 120)",
                path_escape(cwd)
            )
        } else {
            format!(
                "silent! set shortmess+=I mouse=a number relativenumber \
                 signcolumn=yes cursorline laststatus=0 showmode ruler \
                 | silent! lcd {} | silent! enew",
                path_escape(cwd)
            )
        };
        let _ = nvim.command(&boot).await;

        info!(
            "nvim UI attached {}x{} (astro={})",
            cols, rows, is_astro
        );

        Ok(Self {
            nvim,
            grid,
            width: cols,
            height: rows,
            force_resize: false,
            launch,
            _io: io_handler,
            child,
        })
    }

    pub fn force_next_resize(&mut self) {
        self.force_resize = true;
    }

    /// Fast attach with `-u NONE` (no user config / Astro) for tests and CI.
    pub async fn start_minimal(cwd: &Path, cols: u16, rows: u16) -> Result<Self> {
        let mut launch = NvimLaunchConfig::default();
        launch.extra_args = vec!["-u".into(), "NONE".into()];
        Self::start(cwd, cols, rows, launch).await
    }

    pub fn snapshot(&self) -> NvimGrid {
        self.grid.lock().snapshot()
    }

    pub async fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        let cols = cols.max(20) as u64;
        let rows = rows.max(5) as u64;
        if !self.force_resize && cols == self.width && rows == self.height {
            return Ok(());
        }
        self.force_resize = false;
        self.grid.lock().resize(cols as usize, rows as usize);
        self.nvim
            .ui_try_resize(cols as i64, rows as i64)
            .await
            .map_err(|e| anyhow::anyhow!("ui_try_resize: {e}"))?;
        self.width = cols;
        self.height = rows;
        Ok(())
    }

    pub async fn input(&self, keys: &str) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        self.nvim
            .input(keys)
            .await
            .map_err(|e| anyhow::anyhow!("nvim_input: {e}"))?;
        Ok(())
    }

    pub async fn command(&self, cmd: &str) -> Result<()> {
        self.nvim
            .command(cmd)
            .await
            .map_err(|e| anyhow::anyhow!("nvim_command: {e}"))?;
        Ok(())
    }

    pub async fn open_file(&self, path: &Path) -> Result<()> {
        let p = path_escape(path);
        self.command(&format!("edit {p}")).await
    }

    /// Open file and jump to 1-based line/column (Wave 5 search / git dirty jump).
    pub async fn open_file_at(&self, path: &Path, line: usize, col: usize) -> Result<()> {
        self.open_file(path).await?;
        let line = line.max(1);
        let col = col.max(1);
        self.command(&format!("call cursor({line}, {col})")).await
    }

    /// Count listed buffers with `bufmodified` set.
    pub async fn dirty_buf_count(&self) -> Result<usize> {
        let v = self
            .nvim
            .eval("len(filter(getbufinfo({'bufmodified':1}), 'v:val.listed'))")
            .await
            .map_err(|e| anyhow::anyhow!("dirty_buf_count: {e}"))?;
        Ok(v.as_i64().unwrap_or(0).max(0) as usize)
    }

    /// Read + clear `g:tui_ide_goto` (neo-tree DB/SSH/AI tabs → host products).
    pub async fn take_tui_ide_goto(&self) -> Result<Option<String>> {
        let v = self
            .nvim
            .eval("get(g:, 'tui_ide_goto', v:null)")
            .await
            .map_err(|e| anyhow::anyhow!("tui_ide_goto: {e}"))?;
        let s = match v {
            Value::Nil => return Ok(None),
            Value::Boolean(false) => return Ok(None),
            Value::String(s) => s.as_str().unwrap_or("").to_string(),
            other => {
                let t = other.to_string();
                if t == "nil" || t.is_empty() {
                    return Ok(None);
                }
                t
            }
        };
        let s = s.trim().to_string();
        if s.is_empty() {
            return Ok(None);
        }
        let _ = self.nvim.command("let g:tui_ide_goto = v:null").await;
        Ok(Some(s))
    }

    /// Workspace diagnostic counts (errors, warnings) via Lua — no host LSP.
    pub async fn diagnostic_counts(&self) -> Result<(u64, u64)> {
        let v = self
            .nvim
            .exec_lua(
                r#"
local errs = #vim.diagnostic.get(nil, { severity = vim.diagnostic.severity.ERROR })
local warns = #vim.diagnostic.get(nil, { severity = vim.diagnostic.severity.WARN })
return {errs, warns}
"#,
                vec![],
            )
            .await
            .map_err(|e| anyhow::anyhow!("diagnostic_counts: {e}"))?;
        match v {
            Value::Array(arr) if arr.len() >= 2 => {
                let e = arr[0].as_u64().or_else(|| arr[0].as_i64().map(|i| i as u64)).unwrap_or(0);
                let w = arr[1].as_u64().or_else(|| arr[1].as_i64().map(|i| i as u64)).unwrap_or(0);
                Ok((e, w))
            }
            _ => Ok((0, 0)),
        }
    }

    pub async fn write_all(&self) -> Result<()> {
        self.command("silent! wall").await
    }

    pub async fn quit_after_write(&self) -> Result<()> {
        let _ = self.write_all().await;
        self.command("qa").await
    }

    pub async fn lcd(&self, path: &Path) -> Result<()> {
        let p = path_escape(path);
        self.command(&format!("silent! lcd {p}")).await
    }

    pub fn mode(&self) -> String {
        self.grid.lock().snapshot().mode
    }

    /// Live `mode()` via RPC (UI `mode_change` can lag one key behind Esc).
    pub async fn mode_live(&self) -> String {
        let m = match self.nvim.eval("mode()").await {
            Ok(v) => match v {
                Value::String(s) => s.as_str().unwrap_or("").to_string(),
                other => other
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
            },
            Err(_) => return self.mode(),
        };
        if !m.is_empty() {
            self.grid.lock().set_mode(m.clone());
            m
        } else {
            self.mode()
        }
    }

    pub async fn paste_text(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        // phase -1 = single call (complete paste)
        self.nvim
            .paste(text, false, -1)
            .await
            .map_err(|e| anyhow::anyhow!("nvim_paste: {e}"))?;
        Ok(())
    }

    pub async fn input_mouse(
        &self,
        button: &str,
        action: &str,
        modifier: &str,
        row: u16,
        col: u16,
    ) -> Result<()> {
        self.nvim
            .input_mouse(button, action, modifier, 0, row as i64, col as i64)
            .await
            .map_err(|e| anyhow::anyhow!("nvim_input_mouse: {e}"))?;
        Ok(())
    }

    pub fn is_astronvim(&self) -> bool {
        self.launch.astronvim_detected || self.launch.appname.as_deref() == Some("astronvim")
    }

    pub fn code_help_text() -> &'static str {
        "══ Code mode help (Wave 1–9 / IDE v1.0) ══\n\
         \n\
         Host chords (not sent to Neovim)\n\
           Ctrl-Shift-H     Home dashboard\n\
           Ctrl-Shift-D     Database studio (restores Code surface on return)\n\
           Ctrl-Shift-P     Host command palette\n\
           Ctrl-Q           Quit (prompts if dirty Neovim buffers)\n\
           Ctrl-B           Toggle host file tree (Wave 3)\n\
           Ctrl-`           Toggle host terminal (Wave 4)\n\
           Ctrl-Shift-F     Project search (Wave 5)\n\
           Ctrl-Shift-G     Git panel (Wave 6)\n\
           Alt-1…9          Switch host buffer tab (Wave 2)\n\
         \n\
         Editor surface\n\
           e / Enter (Home) open Neovim grid\n\
           keys             forwarded to Neovim / AstroNvim\n\
           mouse click      cursor / select (nvim_input_mouse)\n\
           scroll wheel     scroll Neovim window\n\
           bracketed paste  nvim_paste into buffer\n\
           Open File / tree Enter → :edit in Neovim when attached\n\
           buffer tabs      host chrome lists Neovim buffers\n\
         \n\
         Terminal (Wave 4)\n\
           Ctrl-`           open / focus / close host PTY pane\n\
           keys             when Terminal focused → PTY (not Neovim)\n\
           palette          Terminal\n\
         \n\
         Search / Git / Debug (Waves 5–7)\n\
           Ctrl-Shift-F     search → Enter opens file:line in Neovim\n\
           Ctrl-Shift-G     git status · stage/unstage · Enter opens file\n\
           palette Debug    DAP (lldb-dap) · F5/F9/F10/F11 when Debug focused\n\
         \n\
         Clipboard (Wave 2)\n\
           Yank → host / Paste host → editor (palette)\n\
         \n\
         Quit / workspace (Wave 3)\n\
           y / Enter        :wall then leave\n\
           !                discard dirty (:qa!)\n\
           n / Esc          cancel\n\
         \n\
         Status\n\
           CODE · Neovim|AstroNvim · mode · E#/W# diagnostics (from nvim)\n\
         \n\
         Fallback\n\
           If nvim missing → native rope editor + LSP\n"
    }

    /// Listed, loaded buffers for host chrome tabs (filename only).
    pub async fn list_buf_infos(&self) -> Result<Vec<NvimBufInfo>> {
        let bufs = self
            .nvim
            .list_bufs()
            .await
            .map_err(|e| anyhow::anyhow!("nvim_list_bufs: {e}"))?;
        let cur = self
            .nvim
            .get_current_buf()
            .await
            .map_err(|e| anyhow::anyhow!("nvim_get_current_buf: {e}"))?;
        let cur_id = cur
            .get_number()
            .await
            .map_err(|e| anyhow::anyhow!("nvim_buf_get_number: {e}"))?;
        let mut out = Vec::new();
        for b in bufs {
            let loaded = b
                .is_loaded()
                .await
                .map_err(|e| anyhow::anyhow!("nvim_buf_is_loaded: {e}"))?;
            if !loaded {
                continue;
            }
            let id = b
                .get_number()
                .await
                .map_err(|e| anyhow::anyhow!("nvim_buf_get_number: {e}"))?;
            // Skip unlisted / help-like internals when buflisted is false.
            let listed = b
                .get_option("buflisted")
                .await
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if !listed {
                continue;
            }
            let name = b
                .get_name()
                .await
                .unwrap_or_default();
            let short = if name.is_empty() {
                "[No Name]".into()
            } else {
                Path::new(&name)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(name.as_str())
                    .to_string()
            };
            out.push(NvimBufInfo {
                id,
                name: short,
                current: id == cur_id,
            });
        }
        Ok(out)
    }

    pub async fn set_current_buf_id(&self, id: i64) -> Result<()> {
        let bufs = self
            .nvim
            .list_bufs()
            .await
            .map_err(|e| anyhow::anyhow!("nvim_list_bufs: {e}"))?;
        for b in bufs {
            let n = b.get_number().await.unwrap_or(-1);
            if n == id {
                self.nvim
                    .set_current_buf(&b)
                    .await
                    .map_err(|e| anyhow::anyhow!("nvim_set_current_buf: {e}"))?;
                return Ok(());
            }
        }
        bail!("buffer {id} not found")
    }

    /// Unnamed register (`"`) text for host clipboard yank.
    pub async fn get_unnamed_register(&self) -> Result<String> {
        let v = self
            .nvim
            .eval(r#"getreg('"')"#)
            .await
            .map_err(|e| anyhow::anyhow!("nvim_eval getreg: {e}"))?;
        Ok(match v {
            Value::String(s) => s
                .as_str()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            Value::Array(arr) => arr
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
                .join("\n"),
            other => other.to_string(),
        })
    }

    /// Open a scratch buffer with Code Wave 1 help (when Neovim is attached).
    pub async fn show_code_help(&self) -> Result<()> {
        let lines: Vec<String> = Self::code_help_text()
            .lines()
            .map(|s| s.to_string())
            .collect();
        let buf = self
            .nvim
            .create_buf(false, true)
            .await
            .map_err(|e| anyhow::anyhow!("nvim_create_buf: {e}"))?;
        buf.set_lines(0, -1, false, lines)
            .await
            .map_err(|e| anyhow::anyhow!("nvim_buf_set_lines: {e}"))?;
        self.nvim
            .set_current_buf(&buf)
            .await
            .map_err(|e| anyhow::anyhow!("nvim_set_current_buf: {e}"))?;
        let _ = self
            .command("silent! setlocal buftype=nofile bufhidden=wipe noswapfile nomodifiable")
            .await;
        Ok(())
    }

    pub async fn quit(&self) {
        let _ = self.nvim.quit_no_save().await;
    }
}

impl Drop for NvimSession {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn path_escape(path: &Path) -> String {
    let s = path.display().to_string();
    let mut out = String::new();
    for c in s.chars() {
        if matches!(c, ' ' | '\\' | '|' | '"' | '\'') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Map a crossterm KeyEvent into Neovim `nvim_input` key notation.
pub fn key_to_nvim(code: crossterm::event::KeyCode, mods: crossterm::event::KeyModifiers) -> Option<String> {
    use crossterm::event::{KeyCode, KeyModifiers};
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let shift = mods.contains(KeyModifiers::SHIFT);

    match code {
        KeyCode::Char(c) => {
            if ctrl {
                if c.is_ascii_alphabetic() {
                    return Some(format!("<C-{}>", c.to_ascii_lowercase()));
                }
                return match c {
                    '`' => Some("<C-`>".into()),
                    '\\' => Some("<C-\\\\>".into()),
                    '[' => Some("<C-[>".into()),
                    ']' => Some("<C-]>".into()),
                    '.' => Some("<C-.>".into()),
                    '/' => Some("<C-/>".into()),
                    _ => Some(format!("<C-{c}>")),
                };
            }
            if alt {
                return Some(format!("<M-{c}>"));
            }
            if c == '<' {
                return Some("<lt>".into());
            }
            Some(c.to_string())
        }
        KeyCode::Enter => Some("<CR>".into()),
        KeyCode::Esc => Some("<Esc>".into()),
        KeyCode::Backspace => Some("<BS>".into()),
        KeyCode::Tab if shift => Some("<S-Tab>".into()),
        KeyCode::Tab => Some("<Tab>".into()),
        KeyCode::Delete => Some("<Del>".into()),
        KeyCode::Insert => Some("<Insert>".into()),
        KeyCode::Home => Some("<Home>".into()),
        KeyCode::End => Some("<End>".into()),
        KeyCode::PageUp => Some("<PageUp>".into()),
        KeyCode::PageDown => Some("<PageDown>".into()),
        KeyCode::Up => Some("<Up>".into()),
        KeyCode::Down => Some("<Down>".into()),
        KeyCode::Left => Some("<Left>".into()),
        KeyCode::Right => Some("<Right>".into()),
        KeyCode::F(n) => Some(format!("<F{n}>")),
        _ => None,
    }
}

/// Host keys that tui-ide intercepts and must NOT forward to Neovim.
pub fn is_host_chord(code: crossterm::event::KeyCode, mods: crossterm::event::KeyModifiers) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    // Super preferred; Control is the reliable fallback when WM eats Super.
    let ctrl = mods.contains(KeyModifiers::CONTROL) || mods.contains(KeyModifiers::SUPER);
    let shift = mods.contains(KeyModifiers::SHIFT);
    if ctrl && shift && matches!(code, KeyCode::Char('d') | KeyCode::Char('D')) {
        return true;
    }
    if ctrl && shift && matches!(code, KeyCode::Char('p') | KeyCode::Char('P')) {
        return true;
    }
    if ctrl && shift && matches!(code, KeyCode::Char('h') | KeyCode::Char('H')) {
        return true;
    }
    // Project search (Wave 5)
    if ctrl && shift && matches!(code, KeyCode::Char('f') | KeyCode::Char('F')) {
        return true;
    }
    // Git panel (Wave 6)
    if ctrl && shift && matches!(code, KeyCode::Char('g') | KeyCode::Char('G')) {
        return true;
    }
    // Sessions (SSH/RDP)
    if ctrl && shift && matches!(code, KeyCode::Char('s') | KeyCode::Char('S')) {
        return true;
    }
    // Agents (Cursor / Claude / OpenCode CLIs)
    if ctrl && shift && matches!(code, KeyCode::Char('a') | KeyCode::Char('A')) {
        return true;
    }
    // Outline pane toggle
    if ctrl && shift && matches!(code, KeyCode::Char('o') | KeyCode::Char('O')) {
        return true;
    }
    // Product ribbon Ctrl-1…5
    if ctrl
        && !shift
        && matches!(
            code,
            KeyCode::Char('1')
                | KeyCode::Char('2')
                | KeyCode::Char('3')
                | KeyCode::Char('4')
                | KeyCode::Char('5')
        )
    {
        return true;
    }
    // Quit host (Neovim has its own :qa)
    if ctrl && matches!(code, KeyCode::Char('q') | KeyCode::Char('Q')) {
        return true;
    }
    // Host file tree toggle (Wave 3)
    if ctrl && !shift && matches!(code, KeyCode::Char('b') | KeyCode::Char('B')) {
        return true;
    }
    // Host terminal toggle (Wave 4)
    if ctrl && !shift && matches!(code, KeyCode::Char('`')) {
        return true;
    }
    false
}

pub fn which_nvim() -> Result<PathBuf> {
    let path = std::env::var_os("PATH").context("PATH unset")?;
    for dir in std::env::split_paths(&path) {
        let c = dir.join("nvim");
        if c.is_file() {
            return Ok(c);
        }
    }
    bail!("nvim not found in PATH — install Neovim to use Code mode with AstroNvim")
}

#[cfg(test)]
mod host_chord_tests {
    use super::is_host_chord;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn wave4_5_6_host_chords() {
        assert!(is_host_chord(
            KeyCode::Char('`'),
            KeyModifiers::CONTROL
        ));
        assert!(is_host_chord(
            KeyCode::Char('f'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        ));
        assert!(is_host_chord(
            KeyCode::Char('g'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        ));
        assert!(is_host_chord(
            KeyCode::Char('s'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        ));
        assert!(is_host_chord(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        ));
        assert!(is_host_chord(
            KeyCode::Char('A'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        ));
        assert!(!is_host_chord(
            KeyCode::Char('s'),
            KeyModifiers::CONTROL
        ));
        assert!(!is_host_chord(KeyCode::Char('j'), KeyModifiers::empty()));
    }
}
