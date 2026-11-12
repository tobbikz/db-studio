//! Application loop: input, render, LSP, git, search, terminal, DAP.

use anyhow::{Context, Result};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
    KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ide_buffer::{Buffer, BufferSet};
use ide_config::IdeConfig;
use ide_core::{
    BottomPanel, CodeSurface, Cursor, DbSurface, Focus, Mode, ProductMode, Sidebar,
    TableFilterField,
};
use ide_db::{
    default_brain_source, query_param_prompt_seed, sql_has_query_params, substitute_query_params,
    AlterColumnKind, DataSource, DbDriver, DbNodeKind, DbWorkspace, PostgresMaintenance,
};
use ide_dap::{find_adapter, DapClient, DapEvent};
use ide_nvim::{self, NvimSession, detect_astronvim, key_to_nvim};
use ide_plugins::PluginHost;
use ide_fs::{list_workspace_files, Workspace};
use ide_git::GitState;
use ide_lsp::{DiagnosticItem, LspManager};
use ide_search::{search_project, SearchState};
use ide_syntax::Highlighter;
use ide_term::TerminalPane;
use ide_sessions::{ConnectOutcome, SessionManager};
use ide_agents::AgentManager;
use ide_ui::{
    home_recent_slots, render, Action, CreateObjectForm, CreateObjectKind, DbConnectForm,
    HomeRecentSlot, Keymap, Palette, PaletteMode, RenderCtx, StatusMessage, UiState,
};
use lsp_types::{
    AnnotatedTextEdit, CodeActionOrCommand, Diagnostic, DocumentChangeOperation, DocumentChanges,
    OneOf, TextEdit, Uri, WorkspaceEdit,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};

pub struct App {
    workspace: Workspace,
    buffers: BufferSet,
    db: DbWorkspace,
    ui: UiState,
    highlighter: Highlighter,
    lsp: LspManager,
    git: Option<GitState>,
    search: SearchState,
    terminal: Option<TerminalPane>,
    sessions: SessionManager,
    agents: AgentManager,
    dap: Option<DapClient>,
    config: IdeConfig,
    should_quit: bool,
    debug_log: String,
    debug_thread_id: i64,
    plugins: PluginHost,
    db_sql_undo_stack: Vec<String>,
    /// Neovim UI backend for Code mode (AstroNvim / user config).
    nvim: Option<NvimSession>,
    nvim_label: String,
    /// True after embed failed — native Code editor is the daily path.
    nvim_fallback: bool,
    /// Restore this Code surface when leaving Database (Wave 2 focus).
    last_code_surface: CodeSurface,
    /// Host chrome rows above the Neovim grid (header [+ tabs]).
    nvim_chrome_top: u16,
    /// Cached buffer tabs for host chrome.
    nvim_bufs: Vec<ide_nvim::NvimBufInfo>,
    /// Ticks between buffer-tab refreshes.
    nvim_tab_tick: u32,
    /// Cached Neovim diagnostic counts (errors, warnings).
    nvim_diag: (u64, u64),
    /// Host columns left of the Neovim grid (file tree).
    nvim_chrome_left: u16,
    /// Pending workspace path after ConfirmQuit (switch folder).
    pending_workspace_path: Option<PathBuf>,
    /// After entering Editor, force a blank buffer (no stock NVIM intro).
    pending_nvim_blank: bool,
    /// After Esc leaves insert/visual, next Esc / 1–4 goes to Code panes (mode RPC lag).
    nvim_esc_armed: bool,
    /// Run SQL concurrently with the event loop (Esc cancel).
    pending_db_query: bool,
    /// Ticks since last connection-health sweep (~33ms each).
    health_tick_counter: u32,
    /// Ticks since last Health · Activity auto-refresh (~33ms each).
    health_activity_tick: u32,
    /// Theme id before palette live-preview (Esc restores).
    theme_before_preview: Option<String>,
}

impl App {
    pub fn new(root: PathBuf) -> Result<Self> {
        let config = IdeConfig::load();
        ide_ui::theme::reload_customs();
        ide_ui::theme::set_mode_name(&config.theme.name);
        let workspace = Workspace::open(&root).context("open workspace")?;
        let lsp = LspManager::new(workspace.root.clone(), config.clone());
        let mut app = Self {
            workspace,
            buffers: BufferSet::new(),
            db: DbWorkspace::load(),
            ui: UiState::default(),
            highlighter: Highlighter::new(),
            lsp,
            git: None,
            search: SearchState::default(),
            terminal: None,
            sessions: SessionManager::load(),
            agents: AgentManager::load(),
            dap: None,
            config,
            should_quit: false,
            debug_log: String::new(),
            debug_thread_id: 1,
            plugins: PluginHost::load(),
            db_sql_undo_stack: Vec::new(),
            nvim: None,
            nvim_label: "Neovim".into(),
            nvim_fallback: false,
            last_code_surface: CodeSurface::Home,
            nvim_chrome_top: 1,
            nvim_bufs: Vec::new(),
            nvim_tab_tick: 0,
            nvim_diag: (0, 0),
            nvim_chrome_left: 0,
            pending_workspace_path: None,
            pending_nvim_blank: false,
            nvim_esc_armed: false,
            pending_db_query: false,
            health_tick_counter: 0,
            health_activity_tick: 0,
            theme_before_preview: None,
        };
        app.config.remember_project(&app.workspace.root);
        app.db.query_timeout_secs = app.config.database.query_timeout_secs;
        app.db.select_limit = app.config.database.select_limit;
        app.apply_studio_layout_from_config();
        app.refresh_queries_dir();
        Ok(app)
    }

    fn apply_studio_layout_from_config(&mut self) {
        let db = &self.config.database;
        self.ui.explorer_width = db.explorer_width.clamp(14, 100);
        self.ui.studio_left_width = db.studio_left_width.clamp(28, 120);
        self.ui.results_pct = db.results_pct.clamp(25, 70);
    }

    fn persist_studio_layout(&mut self) {
        self.config.database.explorer_width = self.ui.explorer_width;
        self.config.database.studio_left_width = self.ui.studio_left_width;
        self.config.database.results_pct = self.ui.results_pct;
        self.config.database.select_limit = self.db.select_limit;
        if let Err(e) = self.config.save() {
            self.set_status(format!("layout save: {e:#}"), true);
        }
    }

    fn cycle_select_limit(&mut self) {
        self.db.select_limit = ide_db::next_select_limit_preset(self.db.select_limit);
        self.config.database.select_limit = self.db.select_limit;
        let _ = self.config.save();
        let msg = if self.db.select_limit == 0 {
            "SELECT limit · off (no auto LIMIT)".to_string()
        } else {
            format!(
                "SELECT limit · {} · Alt-L cycle · applies when SQL has no LIMIT",
                self.db.select_limit
            )
        };
        self.set_status(msg, false);
    }

    /// Ctrl+arrows — resize only the focused pane's divider.
    fn resize_focused_pane(&mut self, code: KeyCode) {
        match self.ui.focus {
            // Explorer: ← shrink / → grow its width (auto-grows left stack).
            Focus::Database => match code {
                KeyCode::Right => {
                    let next = (self.ui.explorer_width + 2).min(100);
                    let need_left = next.saturating_add(12);
                    if need_left > self.ui.studio_left_width {
                        self.ui.studio_left_width = need_left.min(120);
                    }
                    self.ui.explorer_width = next
                        .min(self.ui.studio_left_width.saturating_sub(12).max(14));
                    self.persist_studio_layout();
                    self.set_status(
                        format!("explorer · {} cols (saved)", self.ui.explorer_width),
                        false,
                    );
                }
                KeyCode::Left => {
                    self.ui.explorer_width =
                        self.ui.explorer_width.saturating_sub(2).max(14);
                    self.persist_studio_layout();
                    self.set_status(
                        format!("explorer · {} cols (saved)", self.ui.explorer_width),
                        false,
                    );
                }
                KeyCode::Down => {
                    self.ui.results_pct = (self.ui.results_pct + 5).min(70);
                    self.persist_studio_layout();
                    self.set_status(
                        format!("results · {}% (saved)", self.ui.results_pct),
                        false,
                    );
                }
                KeyCode::Up => {
                    self.ui.results_pct =
                        self.ui.results_pct.saturating_sub(5).max(25);
                    self.persist_studio_layout();
                    self.set_status(
                        format!("results · {}% (saved)", self.ui.results_pct),
                        false,
                    );
                }
                _ => {}
            },
            // Structure: ← more explorer / → more structure (move shared split).
            Focus::Structure => match code {
                KeyCode::Left => {
                    let next = (self.ui.explorer_width + 2).min(100);
                    let need_left = next.saturating_add(12);
                    if need_left > self.ui.studio_left_width {
                        self.ui.studio_left_width = need_left.min(120);
                    }
                    self.ui.explorer_width = next
                        .min(self.ui.studio_left_width.saturating_sub(12).max(14));
                    self.persist_studio_layout();
                    self.set_status(
                        format!("structure ← explorer · {} cols", self.ui.explorer_width),
                        false,
                    );
                }
                KeyCode::Right => {
                    self.ui.explorer_width =
                        self.ui.explorer_width.saturating_sub(2).max(14);
                    self.persist_studio_layout();
                    self.set_status(
                        format!("structure → · explorer {} cols", self.ui.explorer_width),
                        false,
                    );
                }
                KeyCode::Down => {
                    self.ui.results_pct = (self.ui.results_pct + 5).min(70);
                    self.persist_studio_layout();
                    self.set_status(
                        format!("results · {}% (saved)", self.ui.results_pct),
                        false,
                    );
                }
                KeyCode::Up => {
                    self.ui.results_pct =
                        self.ui.results_pct.saturating_sub(5).max(25);
                    self.persist_studio_layout();
                    self.set_status(
                        format!("results · {}% (saved)", self.ui.results_pct),
                        false,
                    );
                }
                _ => {}
            },
            // Console: ← grow console (shrink left) / → grow left stack.
            Focus::Editor => match code {
                KeyCode::Left => {
                    self.ui.studio_left_width =
                        self.ui.studio_left_width.saturating_sub(2).max(28);
                    let max_ex = self.ui.studio_left_width.saturating_sub(12).max(14);
                    if self.ui.explorer_width > max_ex {
                        self.ui.explorer_width = max_ex;
                    }
                    self.persist_studio_layout();
                    self.set_status(
                        format!("console ← · left {} cols", self.ui.studio_left_width),
                        false,
                    );
                }
                KeyCode::Right => {
                    self.ui.studio_left_width =
                        (self.ui.studio_left_width + 2).min(120);
                    self.persist_studio_layout();
                    self.set_status(
                        format!("console → · left {} cols", self.ui.studio_left_width),
                        false,
                    );
                }
                KeyCode::Down => {
                    self.ui.results_pct = (self.ui.results_pct + 5).min(70);
                    self.persist_studio_layout();
                    self.set_status(
                        format!("results · {}% (saved)", self.ui.results_pct),
                        false,
                    );
                }
                KeyCode::Up => {
                    self.ui.results_pct =
                        self.ui.results_pct.saturating_sub(5).max(25);
                    self.persist_studio_layout();
                    self.set_status(
                        format!("results · {}% (saved)", self.ui.results_pct),
                        false,
                    );
                }
                _ => {}
            },
            // Results: ↑ shrink / ↓ grow height only.
            Focus::Results => match code {
                KeyCode::Down => {
                    self.ui.results_pct = (self.ui.results_pct + 5).min(70);
                    self.persist_studio_layout();
                    self.set_status(
                        format!("results · {}% (saved)", self.ui.results_pct),
                        false,
                    );
                }
                KeyCode::Up => {
                    self.ui.results_pct =
                        self.ui.results_pct.saturating_sub(5).max(25);
                    self.persist_studio_layout();
                    self.set_status(
                        format!("results · {}% (saved)", self.ui.results_pct),
                        false,
                    );
                }
                KeyCode::Left | KeyCode::Right => {
                    self.set_status(
                        "results · Ctrl+↑↓ height (←→ for Explorer/Console)",
                        false,
                    );
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn refresh_queries_dir(&mut self) {
        self.db
            .set_queries_dir(self.workspace.root.join("queries"));
    }

    fn start_open_folder(&mut self) {
        self.ui.mode = Mode::OpenFolder;
        self.ui.open_folder_input = self.workspace.root.display().to_string();
    }

    fn open_projects_palette(&mut self) {
        self.ui.mode = Mode::Palette;
        self.ui.palette = Palette::open_projects(&self.config.recent_projects);
    }

    /// Home W: pick a DB connection workspace → open Studio.
    fn open_db_workspaces_palette(&mut self) {
        if self.db.sources.is_empty() {
            self.set_status("no workspaces — A to add", false);
            return;
        }
        let items: Vec<(String, String)> = self
            .db
            .sources
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let where_ = match s.driver {
                    ide_db::DbDriver::Sqlite => {
                        if s.file.is_empty() {
                            "sqlite".into()
                        } else {
                            s.file.clone()
                        }
                    }
                    _ => {
                        if s.database.is_empty() {
                            format!("{}:{}", s.host, s.port)
                        } else {
                            format!("{}:{}/{}", s.host, s.port, s.database)
                        }
                    }
                };
                (
                    format!("{} · {} · {where_}", s.name, s.driver.label()),
                    format!("workspace:{}", i + 1),
                )
            })
            .collect();
        self.ui.mode = Mode::Palette;
        self.ui.palette = Palette::open_workspaces(items);
    }

    async fn switch_workspace(&mut self, path: PathBuf) -> Result<()> {
        if let Some(nvim) = self.nvim.as_ref() {
            let dirty = nvim.dirty_buf_count().await.unwrap_or(0);
            if dirty > 0 {
                self.pending_workspace_path = Some(path);
                self.ui.confirm_quit_label = format!(
                    "{dirty} dirty Neovim buffer(s). y=save & open · !=discard · n=cancel"
                );
                self.ui.mode = Mode::ConfirmQuit;
                return Ok(());
            }
        }
        self.switch_workspace_force(path, false).await
    }

    async fn switch_workspace_force(&mut self, path: PathBuf, write_first: bool) -> Result<()> {
        if let Some(nvim) = self.nvim.as_ref() {
            if write_first {
                let _ = nvim.write_all().await;
            }
        }
        if let Some(nvim) = self.nvim.take() {
            nvim.quit().await;
        }
        let ws = Workspace::open(&path).context("open folder")?;
        let root = ws.root.clone();
        self.workspace = ws;
        self.buffers = BufferSet::new();
        self.ui.split = Default::default();
        self.ui.code_surface = CodeSurface::Home;
        self.last_code_surface = CodeSurface::Home;
        self.ui.mode = Mode::Normal;
        self.ui.product = ProductMode::Code;
        self.ui.confirm_quit_label.clear();
        self.pending_workspace_path = None;
        self.nvim_bufs.clear();
        self.nvim_diag = (0, 0);
        self.config.remember_project(&root);
        self.lsp = LspManager::new(root.clone(), self.config.clone());
        self.refresh_queries_dir();
        self.git = None;
        self.search = SearchState::default();
        self.pending_nvim_blank = false;
        let name = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("workspace");
        self.set_status(format!("opened {name}"), false);
        Ok(())
    }

    async fn request_quit(&mut self) -> Result<()> {
        if let Some(nvim) = self.nvim.as_ref() {
            let dirty = nvim.dirty_buf_count().await.unwrap_or(0);
            if dirty > 0 {
                self.pending_workspace_path = None;
                self.ui.confirm_quit_label = format!(
                    "{dirty} dirty Neovim buffer(s). y=save & quit · !=discard · n=cancel"
                );
                self.ui.mode = Mode::ConfirmQuit;
                return Ok(());
            }
            nvim.quit().await;
            self.nvim = None;
            self.should_quit = true;
            return Ok(());
        }
        if self.buffers.iter().any(|b| b.dirty) {
            self.set_status("unsaved buffers — :q! to force or :w to save", true);
            return Ok(());
        }
        self.should_quit = true;
        Ok(())
    }

    async fn confirm_quit_save(&mut self) -> Result<()> {
        self.ui.mode = Mode::Normal;
        self.ui.confirm_quit_label.clear();
        if let Some(path) = self.pending_workspace_path.take() {
            return self.switch_workspace_force(path, true).await;
        }
        if let Some(nvim) = self.nvim.take() {
            let _ = nvim.quit_after_write().await;
        }
        self.should_quit = true;
        Ok(())
    }

    async fn confirm_quit_discard(&mut self) -> Result<()> {
        self.ui.mode = Mode::Normal;
        self.ui.confirm_quit_label.clear();
        if let Some(path) = self.pending_workspace_path.take() {
            return self.switch_workspace_force(path, false).await;
        }
        if let Some(nvim) = self.nvim.take() {
            nvim.quit().await;
        }
        self.should_quit = true;
        Ok(())
    }

    async fn confirm_open_folder(&mut self) -> Result<()> {
        let raw = self.ui.open_folder_input.trim().to_string();
        self.ui.open_folder_input.clear();
        self.ui.mode = Mode::Normal;
        if raw.is_empty() {
            return Ok(());
        }
        let path = expand_user_path(&raw);
        if !path.is_dir() {
            self.set_status(format!("not a folder: {raw}"), true);
            return Ok(());
        }
        self.switch_workspace(path).await
    }

    async fn ensure_nvim(&mut self, cols: u16, rows: u16) -> Result<()> {
        if self.nvim.is_some() {
            return Ok(());
        }
        let mut launch = detect_astronvim();
        if let Some(bin) = self.config.code.nvim_binary.as_ref() {
            let p = PathBuf::from(bin);
            if p.is_file() {
                launch.binary = Some(p);
                launch
                    .notes
                    .push(format!("nvim binary from config: {bin}"));
            }
        }
        if let Some(app) = self.config.code.nvim_appname.as_ref() {
            if !app.trim().is_empty() {
                launch.appname = Some(app.clone());
                launch
                    .notes
                    .push(format!("NVIM_APPNAME from config: {app}"));
            }
        }
        for note in &launch.notes {
            info!("nvim launch: {note}");
        }
        match NvimSession::start(&self.workspace.root, cols, rows, launch.clone()).await {
            Ok(session) => {
                let label = if session.is_astronvim() {
                    "AstroNvim"
                } else {
                    "Neovim"
                };
                self.nvim_label = label.into();
                self.nvim_fallback = false;
                self.set_status(
                    format!("{label} UI attached — plugins/LSP/Mason run inside Neovim"),
                    false,
                );
                self.nvim = Some(session);
                Ok(())
            }
            Err(e) => {
                warn!("nvim embed failed: {e:#}");
                self.nvim_fallback = true;
                self.set_status(
                    format!("Neovim unavailable ({e}); native Code editor active"),
                    true,
                );
                Err(e)
            }
        }
    }

    fn nvim_active(&self) -> bool {
        self.ui.product == ProductMode::Code
            && self.ui.code_surface == CodeSurface::Editor
            && self.nvim.is_some()
    }

    fn enter_code_editor(&mut self) {
        if self.ui.product != ProductMode::Code {
            return;
        }
        self.ui.code_surface = CodeSurface::Editor;
        self.last_code_surface = CodeSurface::Editor;
        self.ui.focus = Focus::Editor;
        self.ui.mode = Mode::Normal;
        self.ui.sidebar = Sidebar::Files;
        self.ui.show_sidebar = false;
        self.pending_nvim_blank = true;
        self.set_status("Code", false);
    }

    fn go_home(&mut self) {
        self.ui.mode = Mode::Normal;
        if self.ui.product == ProductMode::Database {
            self.go_db_home();
            return;
        }
        self.ui.code_surface = CodeSurface::Home;
        self.last_code_surface = CodeSurface::Home;
        self.set_status("Home dashboard", false);
    }

    fn open_command_palette(&mut self) {
        self.ui.mode = Mode::Palette;
        self.ui.palette = Palette::open_commands();
        let plugin_items: Vec<(String, String)> = self
            .plugins
            .all_commands()
            .into_iter()
            .map(|(plugin, id, title)| (format!("[{plugin}] {title}"), format!("plugin:{id}")))
            .collect();
        self.ui.palette.append_items(plugin_items);
        // Custom themes from ~/.config/tui-ide/themes/*.toml
        ide_ui::theme::reload_customs();
        let custom_items: Vec<(String, String)> = ide_ui::theme::customs()
            .into_iter()
            .map(|t| (format!("Theme: {} (custom)", t.label), format!("theme_{}", t.id)))
            .collect();
        self.ui.palette.append_items(custom_items);
        // Snapshot committed theme so Esc can undo live previews.
        self.theme_before_preview = None;
        self.preview_palette_theme();
    }

    fn open_db_object_palette(&mut self) {
        use ide_db::DbNodeKind;
        let items: Vec<(String, String, String)> = self
            .db
            .flat
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e.kind, DbNodeKind::Table | DbNodeKind::View))
            .map(|(i, e)| {
                let label = e.label.clone();
                let sub = match (&e.schema, &e.database) {
                    (Some(s), Some(d)) if !d.is_empty() => format!("{d}.{s}"),
                    (Some(s), _) => s.clone(),
                    _ => e.source_id.clone(),
                };
                (label, sub, i.to_string())
            })
            .collect();
        self.ui.mode = Mode::Palette;
        self.ui.palette = Palette::open_db_objects(items);
    }

    fn db_object_palette_items(&self) -> Vec<(String, String, String)> {
        self.db
            .flat
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e.kind, DbNodeKind::Table | DbNodeKind::View))
            .map(|(i, e)| {
                let label = e.label.clone();
                let sub = match (&e.schema, &e.database) {
                    (Some(s), Some(d)) if !d.is_empty() => format!("{d}.{s}"),
                    (Some(s), _) => s.clone(),
                    _ => e.source_id.clone(),
                };
                (label, sub, i.to_string())
            })
            .collect()
    }

    fn open_create_menu(&mut self) {
        if self.db.viewer_hides_ddl() {
            self.set_status(
                "Create DDL hidden — connection is viewer/read-only (Support: ask Admin)",
                true,
            );
            return;
        }
        let driver = self
            .db
            .active_source
            .as_ref()
            .and_then(|id| self.db.sources.iter().find(|s| s.id == *id))
            .map(|s| s.driver)
            .or_else(|| {
                self.db
                    .flat
                    .get(self.db.selected)
                    .and_then(|e| {
                        self.db
                            .sources
                            .iter()
                            .find(|s| s.id == e.source_id)
                            .map(|s| s.driver)
                    })
            })
            .unwrap_or_default();
        self.ui.mode = Mode::Palette;
        self.ui.palette = Palette::open_create_for(driver);
        self.set_status(
            format!("Create · {} · type to filter · Enter", driver.label()),
            false,
        );
    }

    fn open_search_everywhere(&mut self) {
        self.ui.mode = Mode::Palette;
        self.ui.palette = Palette::open_search_everywhere(self.db_object_palette_items());
        self.set_status("Search Everywhere · tools · objects · commands", false);
    }

    fn resume_console_editing(&mut self) {
        self.ui.focus = Focus::Editor;
        self.ui.mode = Mode::Insert;
        self.ui.show_completions = false;
        self.ui.resume_console_insert = false;
        self.ui.db_sql_all_selected = false;
    }

    async fn open_console_database_palette(&mut self) {
        if self.ui.product != ProductMode::Database {
            return;
        }
        // Always return to Insert on Console after picking — bare `b` left users stuck in NORMAL.
        self.ui.resume_console_insert = true;
        match self.db.list_console_databases().await {
            Ok(names) if names.is_empty() => {
                self.ui.resume_console_insert = false;
                self.set_status("no databases to list for this source", true);
            }
            Ok(names) => {
                self.ui.mode = Mode::Palette;
                self.ui.palette = Palette::open_console_databases(names);
                self.set_status("database · type to filter · Enter · Esc back to Console", false);
            }
            Err(e) => {
                self.ui.resume_console_insert = false;
                self.set_status(format!("databases: {e:#}"), true);
            }
        }
    }

    async fn open_console_schema_palette(&mut self) {
        if self.ui.product != ProductMode::Database {
            return;
        }
        self.ui.resume_console_insert = true;
        match self.db.list_console_schemas().await {
            Ok(names) if names.is_empty() => {
                self.ui.resume_console_insert = false;
                self.set_status("no schemas to list — pick a database first", true);
            }
            Ok(names) => {
                self.ui.mode = Mode::Palette;
                self.ui.palette = Palette::open_console_schemas(names);
                self.set_status("schema · type to filter · Enter · Esc back to Console", false);
            }
            Err(e) => {
                self.ui.resume_console_insert = false;
                self.set_status(format!("schemas: {e:#}"), true);
            }
        }
    }

    fn begin_table_filter_edit(&mut self, field: TableFilterField) {
        if !self.db.active_is_table_tab() {
            self.set_status("WHERE / ORDER BY only on table data tabs", true);
            return;
        }
        self.ui.table_filter_field = field;
        self.ui.table_filter_draft = match field {
            TableFilterField::Where => self.db.active_table_where().to_string(),
            TableFilterField::OrderBy => self.db.active_table_order_by().to_string(),
        };
        self.ui.focus = Focus::Results;
        self.ui.mode = Mode::EditTableFilter;
        // Keep footer bindings visible (no toast that hides `o order`).
        self.ui.status = None;
        self.refresh_table_filter_completions();
    }

    async fn confirm_table_filter(&mut self) -> Result<()> {
        let draft = self.ui.table_filter_draft.clone();
        match self.ui.table_filter_field {
            TableFilterField::Where => self.db.set_active_table_where(draft),
            TableFilterField::OrderBy => self.db.set_active_table_order_by(draft),
        }
        self.ui.mode = Mode::Normal;
        self.ui.table_filter_draft.clear();
        self.ui.show_completions = false;
        self.ui.completions.clear();
        self.ui.focus = Focus::Results;
        match self.db.apply_table_filters().await {
            Ok(()) => {
                self.ui.bottom = BottomPanel::Results;
                self.set_status(self.db.status.clone(), false);
            }
            Err(e) => self.set_status(format!("filter: {e:#}"), true),
        }
        Ok(())
    }

    fn cycle_table_filter_field(&mut self, forward: bool) {
        if self.ui.mode != Mode::EditTableFilter {
            return;
        }
        // Persist current draft into the active field before switching.
        let draft = self.ui.table_filter_draft.clone();
        match self.ui.table_filter_field {
            TableFilterField::Where => self.db.set_active_table_where(draft),
            TableFilterField::OrderBy => self.db.set_active_table_order_by(draft),
        }
        self.ui.table_filter_field = if forward {
            match self.ui.table_filter_field {
                TableFilterField::Where => TableFilterField::OrderBy,
                TableFilterField::OrderBy => TableFilterField::Where,
            }
        } else {
            match self.ui.table_filter_field {
                TableFilterField::Where => TableFilterField::OrderBy,
                TableFilterField::OrderBy => TableFilterField::Where,
            }
        };
        self.ui.table_filter_draft = match self.ui.table_filter_field {
            TableFilterField::Where => self.db.active_table_where().to_string(),
            TableFilterField::OrderBy => self.db.active_table_order_by().to_string(),
        };
        self.refresh_table_filter_completions();
    }

    /// Current identifier token at end of WHERE/ORDER draft → (byte-free char start, token).
    fn table_filter_token(draft: &str) -> (usize, String) {
        let chars: Vec<char> = draft.chars().collect();
        let mut i = chars.len();
        while i > 0 {
            let c = chars[i - 1];
            if c.is_ascii_alphanumeric() || c == '_' {
                i -= 1;
            } else {
                break;
            }
        }
        (i, chars[i..].iter().collect())
    }

    fn refresh_table_filter_completions(&mut self) {
        if self.ui.mode != Mode::EditTableFilter {
            return;
        }
        let draft = self.ui.table_filter_draft.clone();
        let cols: Vec<String> = self
            .db
            .last_result
            .as_ref()
            .map(|r| r.columns.clone())
            .unwrap_or_default();
        let (start, token) = Self::table_filter_token(&draft);
        let token_l = token.to_ascii_lowercase();
        let mut items: Vec<ide_lsp::CompletionItemView> = Vec::new();

        // After a finished column + space in ORDER BY → ASC / DESC.
        let trimmed = draft.trim_end();
        let ends_space = draft.ends_with(' ') && !trimmed.is_empty();
        if matches!(self.ui.table_filter_field, TableFilterField::OrderBy) && ends_space {
            for dir in ["ASC", "DESC"] {
                items.push(ide_lsp::CompletionItemView {
                    label: dir.to_string(),
                    detail: Some("direction".into()),
                    insert_text: dir.to_string(),
                    kind: None,
                    replace_start_col: Some(draft.chars().count() as u32),
                    replace_end_col: Some(draft.chars().count() as u32),
                    filter_text: None,
                });
            }
        } else {
            for c in &cols {
                let cl = c.to_ascii_lowercase();
                if token_l.is_empty() || cl.starts_with(&token_l) || cl.contains(&token_l) {
                    items.push(ide_lsp::CompletionItemView {
                        label: c.clone(),
                        detail: Some("column".into()),
                        insert_text: c.clone(),
                        kind: None,
                        replace_start_col: Some(start as u32),
                        replace_end_col: Some(draft.chars().count() as u32),
                        filter_text: Some(token.clone()),
                    });
                }
            }
            items.sort_by(|a, b| {
                let a_pref = a
                    .label
                    .to_ascii_lowercase()
                    .starts_with(&token_l);
                let b_pref = b
                    .label
                    .to_ascii_lowercase()
                    .starts_with(&token_l);
                b_pref.cmp(&a_pref).then_with(|| a.label.cmp(&b.label))
            });
            items.truncate(24);
        }

        let n = items.len();
        self.ui.completions = items;
        self.ui.completion_idx = 0;
        self.ui.show_completions = n > 0;
    }

    fn accept_table_filter_completion(&mut self) {
        if !self.ui.show_completions || self.ui.completions.is_empty() {
            // No suggestions → Tab switches WHERE ↔ ORDER BY.
            self.cycle_table_filter_field(true);
            return;
        }
        let item = self.ui.completions[self.ui.completion_idx.min(self.ui.completions.len() - 1)]
            .clone();
        let draft = self.ui.table_filter_draft.clone();
        let chars: Vec<char> = draft.chars().collect();
        let start = item
            .replace_start_col
            .map(|c| c as usize)
            .unwrap_or(0)
            .min(chars.len());
        let end = item
            .replace_end_col
            .map(|c| c as usize)
            .unwrap_or(chars.len())
            .min(chars.len());
        let prefix: String = chars[..start].iter().collect();
        let suffix: String = chars[end..].iter().collect();
        let insert = if matches!(self.ui.table_filter_field, TableFilterField::OrderBy)
            && item.detail.as_deref() == Some("direction")
        {
            item.insert_text
        } else if matches!(self.ui.table_filter_field, TableFilterField::OrderBy) {
            // Leave a space so ASC/DESC suggestions appear next.
            format!("{} ", item.insert_text)
        } else {
            item.insert_text
        };
        self.ui.table_filter_draft = format!("{prefix}{insert}{suffix}");
        self.refresh_table_filter_completions();
    }

    async fn go_to_ddl(&mut self) -> Result<()> {
        match self.db.script_object_at_selection().await {
            Ok(()) => {
                self.ui.focus = Focus::Editor;
                self.set_status(self.db.status.clone(), false);
            }
            Err(e) => self.set_status(format!("DDL: {e:#}"), true),
        }
        Ok(())
    }

    fn selected_connection_id(&self) -> Option<String> {
        self.db
            .flat
            .get(self.db.selected)
            .filter(|e| matches!(e.kind, DbNodeKind::Source))
            .map(|e| e.source_id.clone())
            .or_else(|| self.db.active_source.clone())
    }

    fn start_delete_connection(&mut self, source_id: &str) {
        let label = self
            .db
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| format!("Remove \"{}\" from datasources?", s.name))
            .unwrap_or_else(|| format!("Remove connection {source_id}?"));
        self.ui.confirm_delete_source_id = Some(source_id.to_string());
        self.ui.confirm_delete_label = label;
        self.ui.mode = Mode::ConfirmDeleteSource;
    }

    fn confirm_delete_connection(&mut self) -> Result<()> {
        let Some(id) = self.ui.confirm_delete_source_id.take() else {
            return Ok(());
        };
        self.ui.confirm_delete_label.clear();
        self.ui.mode = Mode::Normal;
        self.db.remove_source(&id)?;
        self.set_status(self.db.status.clone(), false);
        Ok(())
    }

    fn show_timing_histogram(&mut self) {
        let text = self.db.timing_histogram_ascii();
        self.db.last_result = Some(ide_db::QueryResult {
            columns: vec!["bucket".into(), "chart".into()],
            rows: text
                .lines()
                .map(|l| {
                    if let Some((a, b)) = l.split_once(" |") {
                        vec![a.trim().to_string(), b.trim().to_string()]
                    } else {
                        vec![l.to_string(), String::new()]
                    }
                })
                .collect(),
            affected: None,
            elapsed_ms: 0,
            truncated: false,
            source_schema: None,
            source_table: None,
            primary_keys: Vec::new(),
            original_rows: Vec::new(),
            cursor_row: 0,
            cursor_col: 0,
            explain: None,
            chart: None,
            page: 0,
            has_more: false,
        });
        self.ui.focus = Focus::Results;
        self.ui.bottom = BottomPanel::Results;
        self.set_status("timing histogram (recent query history)", false);
    }

    fn db_push_undo(&mut self) {
        self.db_sql_undo_stack.push(self.db.sql.clone());
        if self.db_sql_undo_stack.len() > 50 {
            self.db_sql_undo_stack.remove(0);
        }
    }

    fn db_undo_sql(&mut self) {
        if let Some(prev) = self.db_sql_undo_stack.pop() {
            self.db.sql = prev;
            self.db_clamp_cursor();
        }
    }

    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        let tone = if error {
            ide_ui::StatusTone::Error
        } else {
            ide_ui::StatusTone::Info
        };
        self.push_status(text, tone);
    }

    /// Green OK badge — only for real successes (test/connect/health), not pane tips.
    fn set_ok(&mut self, text: impl Into<String>) {
        self.push_status(text, ide_ui::StatusTone::Success);
    }

    fn push_status(&mut self, text: impl Into<String>, tone: ide_ui::StatusTone) {
        let raw = text.into();
        let text = match tone {
            ide_ui::StatusTone::Error => Self::human_err(&raw),
            _ => Self::human_ok(&raw),
        };
        let secs = match tone {
            ide_ui::StatusTone::Error => 4,
            ide_ui::StatusTone::Success => 3,
            ide_ui::StatusTone::Info => 2,
        };
        self.ui.status = Some(StatusMessage {
            text,
            error: matches!(tone, ide_ui::StatusTone::Error),
            tone,
            until: std::time::Instant::now() + Duration::from_secs(secs),
        });
    }

    fn clear_expired_status(&mut self) {
        if let Some(msg) = &self.ui.status {
            if std::time::Instant::now() >= msg.until {
                self.ui.status = None;
            }
        }
    }

    fn human_ok(raw: &str) -> String {
        let line = raw.lines().next().unwrap_or(raw).trim();
        if line.chars().count() <= 88 {
            line.to_string()
        } else {
            format!("{}…", line.chars().take(87).collect::<String>())
        }
    }

    fn human_err(raw: &str) -> String {
        let line = raw
            .lines()
            .next()
            .unwrap_or(raw)
            .split("Caused by:")
            .next()
            .unwrap_or(raw)
            .trim();
        let cleaned = line
            .trim_start_matches("Error:")
            .trim_start_matches("error:")
            .trim();
        if cleaned.chars().count() <= 72 {
            cleaned.to_string()
        } else {
            format!("{}…", cleaned.chars().take(71).collect::<String>())
        }
    }

    fn enter_database_mode(&mut self) {
        self.ui.product = ProductMode::Database;
        self.ui.db_surface = DbSurface::Home;
        self.ui.sidebar = Sidebar::Database;
        self.ui.bottom = BottomPanel::Results;
        self.ui.show_sidebar = true;
        self.ui.focus = Focus::Editor;
        self.ui.mode = Mode::Normal;
        self.ui.cell_modal = false;
        self.ui.db_monitor = false;
        self.db.rebuild_flat();
        self.set_status("Database Home", false);
    }

    fn enter_db_studio(&mut self) {
        self.ui.product = ProductMode::Database;
        self.ui.db_surface = DbSurface::Studio;
        self.ui.sidebar = Sidebar::Database;
        self.ui.bottom = BottomPanel::Results;
        self.ui.show_sidebar = true;
        self.refresh_queries_dir();
        self.db.rebuild_flat();
        self.db.prepare_studio_entry();
        // Explorer first — DataGrip-like: see connection + properties strip; Console is Tab/2/3.
        self.ui.focus = Focus::Database;
        self.ui.mode = Mode::Normal;
        let conn = self
            .db
            .active_source
            .as_ref()
            .and_then(|id| self.db.sources.iter().find(|s| &s.id == id))
            .map(|s| s.name.as_str())
            .unwrap_or("—");
        self.set_status(
            format!("Studio · {conn} · p props · n New · 3 Console"),
            false,
        );
    }

    fn cycle_db_pane(&mut self) {
        if self.ui.product != ProductMode::Database || self.ui.db_surface != DbSurface::Studio {
            return;
        }
        self.ui.focus = match self.ui.focus {
            Focus::Database => Focus::Structure,
            Focus::Structure => Focus::Editor,
            Focus::Editor => Focus::Results,
            _ => Focus::Database,
        };
        self.ui.mode = Mode::Normal;
        // Fullscreen follows the focused pane when already zoomed.
        if self.ui.db_fullscreen.is_some() {
            self.ui.db_fullscreen = Some(self.ui.focus);
        }
        let pane = match self.ui.focus {
            Focus::Database => "1 Explorer",
            Focus::Structure => "2 Structure",
            Focus::Editor => "3 Console",
            Focus::Results => "4 Results",
            _ => "Explorer",
        };
        self.set_status(
            format!("{pane} · Tab next · f fullscreen · Esc explorer"),
            false,
        );
    }

    fn focus_db_pane(&mut self, n: u8) {
        if self.ui.product != ProductMode::Database || self.ui.db_surface != DbSurface::Studio {
            return;
        }
        self.ui.focus = match n {
            1 => Focus::Database,
            2 => Focus::Structure,
            3 => Focus::Editor,
            4 => Focus::Results,
            _ => return,
        };
        self.ui.mode = Mode::Normal;
        self.ui.bottom = BottomPanel::Results;
        if self.ui.db_fullscreen.is_some() {
            self.ui.db_fullscreen = Some(self.ui.focus);
        }
    }

    /// Code panes — Data Studio digits; product Code has no DB/Sessions chrome.
    /// Astro: 1 Files · 2 Lazygit · 3 Editor · 4 Editor (tree closed).
    async fn focus_code_pane(&mut self, n: u8) -> Result<()> {
        if self.ui.product != ProductMode::Code || self.ui.code_surface != CodeSurface::Editor {
            return Ok(());
        }
        // Agents/Sessions workspaces keep their own chrome.
        if matches!(self.ui.sidebar, Sidebar::Agents | Sidebar::Sessions) {
            return Ok(());
        }
        if self.nvim_active() {
            match n {
                1 => {
                    self.astro_cmd(
                        "Neotree filesystem focus left",
                        "1 Files · 2 Lazygit · 3/4 Editor · Ctrl-2 DB · Ctrl-3 Sessions",
                    )
                    .await?;
                }
                2 => {
                    self.astro_cmd(
                        "lua pcall(function() require('neo-tree.command').execute({action='close'}) end); \
                         vim.schedule(function() pcall(function() Snacks.lazygit() end) end)",
                        "2 Lazygit · q quit · 1 Files · 3 Editor",
                    )
                    .await?;
                }
                3 | 4 => {
                    self.astro_cmd(
                        "lua vim.cmd('stopinsert'); pcall(function() require('neo-tree.command').execute({action='close'}) end)",
                        "Editor · Esc/1 Files · 2 Lazygit",
                    )
                    .await?;
                    self.ui.focus = Focus::Editor;
                    self.ui.mode = Mode::Normal;
                }
                _ => {}
            }
            return Ok(());
        }
        match n {
            1 => {
                self.ui.sidebar = Sidebar::Files;
                self.ui.show_sidebar = true;
                self.ui.focus = Focus::Tree;
                self.ui.mode = Mode::Normal;
                self.set_status("1 Files · j/k · Enter · 2 editor · 3 outline · 4 term", false);
            }
            2 => {
                self.ui.focus = Focus::Editor;
                self.ui.mode = Mode::Normal;
                self.set_status("2 Editor · Esc → files · i insert", false);
            }
            3 => {
                self.ui.show_outline = true;
                self.ui.show_sidebar = true;
                self.ui.focus = Focus::Outline;
                self.ui.mode = Mode::Normal;
                self.set_status("3 Outline · j/k · Enter jump", false);
            }
            4 => {
                self.ui.bottom = BottomPanel::Terminal;
                self.ui.focus = Focus::Terminal;
                self.ui.mode = Mode::Normal;
                let cols = 80u16;
                let rows = 12u16;
                let _ = self.ensure_terminal(cols, rows);
                self.set_status("4 Terminal · Ctrl-` toggle · Esc/1 files", false);
            }
            _ => {}
        }
        Ok(())
    }

    fn toggle_db_fullscreen(&mut self) {
        if self.ui.product != ProductMode::Database || self.ui.db_surface != DbSurface::Studio {
            return;
        }
        if self.ui.db_fullscreen.is_some() {
            self.ui.db_fullscreen = None;
            self.set_status("fullscreen off", false);
        } else {
            let pane = match self.ui.focus {
                Focus::Database | Focus::Structure | Focus::Editor | Focus::Results => {
                    self.ui.focus
                }
                _ => Focus::Database,
            };
            self.ui.db_fullscreen = Some(pane);
            self.set_status("fullscreen · f / Esc exit", false);
        }
    }

    fn focus_db_explorer(&mut self) {
        self.ui.product = ProductMode::Database;
        self.ui.db_surface = DbSurface::Studio;
        self.ui.sidebar = Sidebar::Database;
        self.ui.focus = Focus::Database;
        self.ui.mode = Mode::Normal;
        self.ui.show_sidebar = true;
    }

    fn go_db_home(&mut self) {
        self.ui.product = ProductMode::Database;
        self.ui.db_surface = DbSurface::Home;
        self.ui.mode = Mode::Normal;
        if let Some(id) = self.db.active_source.clone() {
            if let Some(i) = self.db.sources.iter().position(|s| s.id == id) {
                self.ui.home_selected = i.min(8);
            }
        }
        let n = self.db.sources.len();
        self.set_status(
            if n == 0 {
                "DB Home · a add connection".to_string()
            } else {
                format!("DB Home · {n} source(s) · Enter open · r health")
            },
            false,
        );
    }

    async fn open_db_source_slot(&mut self, slot: u8) {
        if slot == 0 || slot > 9 {
            return;
        }
        let idx = (slot as usize).saturating_sub(1);
        let Some(src) = self.db.sources.get(idx).cloned() else {
            self.set_status(format!("no source [{slot}]"), true);
            return;
        };
        self.ui.home_selected = idx;
        self.ui.busy_label = Some(format!("connecting · {}", src.name));
        self.set_status(format!("connecting · {}…", src.name), false);
        match self.db.ensure_connected(&src.id, None).await {
            Ok(()) => {
                self.db.active_source = Some(src.id.clone());
                let _ = self.db.save_sources();
                self.db
                    .connection_health
                    .insert(src.id.clone(), ide_db::ConnHealth::ok_now());
                self.db.rebuild_flat();
                self.db.status = format!("connected · {}", src.name);
                self.ui.busy_label = None;
                self.enter_db_studio();
                // Select active source in Explorer + fill Structure properties + gray strip.
                if let Some(idx) = self
                    .db
                    .flat
                    .iter()
                    .position(|e| e.source_id == src.id && matches!(e.kind, DbNodeKind::Source))
                {
                    self.db.selected = idx;
                }
                let _ = self.db.refresh_structure_for_selection().await;
                self.set_ok(format!("connected · {}", src.name));
            }
            Err(e) => {
                self.ui.busy_label = None;
                self.db.connection_health.insert(
                    src.id.clone(),
                    ide_db::ConnHealth::Dead(format!("{e:#}")),
                );
                self.set_status(format!("connect failed · {}: {e:#}", src.name), true);
            }
        }
    }

    /// Home Enter: connect selected/active source, then open Studio.
    async fn home_enter_studio(&mut self) {
        match self.ui.home_pane {
            ide_ui::HomePane::Recents => {
                self.home_open_recent().await;
                return;
            }
            ide_ui::HomePane::Favorites => {
                self.home_open_favorite().await;
                return;
            }
            ide_ui::HomePane::Sources => {}
        }
        if self.db.sources.is_empty() {
            self.start_add_connection();
            return;
        }
        let idx = self
            .ui
            .home_selected
            .min(self.db.sources.len().saturating_sub(1));
        let slot = (idx + 1) as u8;
        self.open_db_source_slot(slot).await;
    }

    /// Studio: cycle active source by delta (±1), reconnect, stay in Studio.
    async fn cycle_db_source(&mut self, delta: i32) {
        if self.db.sources.is_empty() {
            self.set_status("no sources — a to add", true);
            return;
        }
        let cur = self
            .db
            .active_source
            .as_ref()
            .and_then(|id| self.db.sources.iter().position(|s| &s.id == id))
            .unwrap_or(0);
        let n = self.db.sources.len() as i32;
        let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
        let src = self.db.sources[next].clone();
        self.ui.busy_label = Some(format!("connecting · {}", src.name));
        self.set_status(format!("switching · {}…", src.name), false);
        match self.db.ensure_connected(&src.id, None).await {
            Ok(()) => {
                self.db.active_source = Some(src.id.clone());
                let _ = self.db.save_sources();
                self.db
                    .connection_health
                    .insert(src.id.clone(), ide_db::ConnHealth::ok_now());
                self.db.rebuild_flat();
                if let Some(tab) = self.db.tabs.get_mut(self.db.active_tab) {
                    tab.source_id = Some(src.id.clone());
                }
                self.ui.busy_label = None;
                self.set_status(format!("active · {}", src.name), false);
            }
            Err(e) => {
                self.ui.busy_label = None;
                self.db.connection_health.insert(
                    src.id.clone(),
                    ide_db::ConnHealth::Dead(format!("{e:#}")),
                );
                self.set_status(format!("switch failed · {}: {e:#}", src.name), true);
            }
        }
    }

    /// Home Favorites: open pinned source by selection.
    async fn home_open_favorite(&mut self) {
        let pinned: Vec<(usize, String)> = self
            .db
            .sources
            .iter()
            .enumerate()
            .filter(|(_, s)| s.pinned)
            .map(|(i, s)| (i, s.id.clone()))
            .collect();
        if pinned.is_empty() {
            self.set_status("no favorites — p to pin a source", true);
            return;
        }
        let idx = self
            .ui
            .home_favorite_selected
            .min(pinned.len().saturating_sub(1));
        let slot = (pinned[idx].0 + 1) as u8;
        self.open_db_source_slot(slot).await;
    }

    /// Home Recents: connect last source, load history SQL into console, open Studio.
    async fn home_open_recent(&mut self) {
        if self.db.history.is_empty() {
            self.set_status("no recent queries", true);
            return;
        }
        let newest_first_idx = self.ui.home_recent_selected.min(5);
        let Some(hist_idx) = self
            .db
            .history
            .len()
            .checked_sub(1)
            .map(|last| last.saturating_sub(newest_first_idx))
        else {
            self.set_status("no recent queries", true);
            return;
        };
        let Some(id) = self
            .db
            .active_source
            .clone()
            .or_else(|| self.db.sources.first().map(|s| s.id.clone()))
        else {
            self.start_add_connection();
            return;
        };
        self.set_status("connecting…", false);
        match self.db.ensure_connected(&id, None).await {
            Ok(()) => {
                self.db.active_source = Some(id.clone());
                let _ = self.db.save_sources();
                self.db
                    .connection_health
                    .insert(id, ide_db::ConnHealth::ok_now());
                if let Err(e) = self.db.open_history_entry(hist_idx) {
                    self.set_status(format!("history: {e:#}"), true);
                    return;
                }
                self.enter_db_studio();
                self.set_status(self.db.status.clone(), false);
            }
            Err(e) => {
                self.set_status(format!("connect failed: {e:#}"), true);
            }
        }
    }

    fn toggle_product_mode(&mut self) {
        // db-studio is Database-only — toggle between Home and Studio.
        if self.ui.db_surface == DbSurface::Studio {
            self.go_db_home();
            self.set_status("DB Home · a connect · e studio", false);
        } else {
            self.enter_db_studio();
            self.set_status("Studio · 1–4 panes · a connect", false);
        }
    }

    fn yank_result_cell(&mut self) {
        let Some(result) = self.db.last_result.as_ref() else {
            self.set_status("copy: no results", true);
            return;
        };
        let Some(row) = result.rows.get(result.cursor_row) else {
            self.set_status("copy: no row", true);
            return;
        };
        let Some(cell) = row.get(result.cursor_col) else {
            self.set_status("copy: no cell", true);
            return;
        };
        let text = if cell == "NULL" { "" } else { cell.as_str() };
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())) {
            Ok(()) => {
                let preview = if text.is_empty() {
                    "NULL".into()
                } else if text.chars().count() > 48 {
                    format!("{}…", text.chars().take(48).collect::<String>())
                } else {
                    text.to_string()
                };
                self.set_status(format!("copied · {preview}"), false);
            }
            Err(e) => self.set_status(format!("clipboard: {e}"), true),
        }
    }

    fn yank_result_row(&mut self) {
        let Some(result) = self.db.last_result.as_ref() else {
            self.set_status("copy row: no results", true);
            return;
        };
        let Some(row) = result.rows.get(result.cursor_row) else {
            self.set_status("copy row: no row", true);
            return;
        };
        let text = row
            .iter()
            .map(|c| if c == "NULL" { "" } else { c.as_str() })
            .collect::<Vec<_>>()
            .join("\t");
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.clone())) {
            Ok(()) => self.set_status(
                format!("copied row · {} cols TSV", row.len()),
                false,
            ),
            Err(e) => self.set_status(format!("clipboard: {e}"), true),
        }
    }

    fn yank_result_all(&mut self) {
        let Some(result) = self.db.last_result.as_ref() else {
            self.set_status("copy all: no results", true);
            return;
        };
        let mut lines: Vec<String> = Vec::with_capacity(result.rows.len() + 1);
        lines.push(result.columns.join("\t"));
        for row in &result.rows {
            lines.push(
                row.iter()
                    .map(|c| if c == "NULL" { "" } else { c.as_str() })
                    .collect::<Vec<_>>()
                    .join("\t"),
            );
        }
        let text = lines.join("\n");
        let n = result.rows.len();
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
            Ok(()) => self.set_status(format!("copied all · {n} rows TSV"), false),
            Err(e) => self.set_status(format!("clipboard: {e}"), true),
        }
    }

    fn open_results_actions(&mut self) {
        self.ui.mode = Mode::Palette;
        self.ui.palette = Palette::open_results_actions();
        self.set_status("results actions · Enter run · Esc", false);
    }

    async fn yank_nvim_to_host_clipboard(&mut self) -> Result<()> {
        let Some(nvim) = self.nvim.as_ref() else {
            self.set_status("yank: Neovim not attached", true);
            return Ok(());
        };
        let text = nvim.get_unnamed_register().await?;
        if text.is_empty() {
            self.set_status("yank: unnamed register empty (\"\" )", true);
            return Ok(());
        }
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.clone())) {
            Ok(()) => self.set_status(
                format!("yank → host clipboard ({} chars)", text.chars().count()),
                false,
            ),
            Err(e) => self.set_status(format!("clipboard set: {e}"), true),
        }
        Ok(())
    }

    async fn paste_host_clipboard_to_nvim(&mut self) -> Result<()> {
        let text = match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(t) => t,
            Err(e) => {
                self.set_status(format!("clipboard get: {e}"), true);
                return Ok(());
            }
        };
        if text.is_empty() {
            self.set_status("host clipboard empty", true);
            return Ok(());
        }
        if let Some(nvim) = self.nvim.as_ref() {
            nvim.paste_text(&text).await?;
            self.set_status(
                format!("pasted {} chars from host clipboard", text.chars().count()),
                false,
            );
        } else if let Some(buf) = self.buffers.active_mut() {
            for ch in text.chars() {
                buf.insert_char(ch);
            }
            self.set_status("pasted into native buffer", false);
        } else {
            self.set_status("paste: no editor target", true);
        }
        Ok(())
    }

    async fn switch_nvim_buf_slot(&mut self, slot: u8) -> Result<()> {
        let idx = slot.saturating_sub(1) as usize;
        let Some(info) = self.nvim_bufs.get(idx).cloned() else {
            self.set_status(format!("no buffer at Alt-{slot}"), true);
            return Ok(());
        };
        if let Some(nvim) = self.nvim.as_ref() {
            nvim.set_current_buf_id(info.id).await?;
            self.set_status(format!("→ {}", info.name), false);
        }
        Ok(())
    }

    fn db_line_count(&self) -> usize {
        self.db.sql.lines().count().max(1)
    }

    fn db_line(&self, row: usize) -> &str {
        self.db.sql.lines().nth(row).unwrap_or("")
    }

    fn db_ensure_cursor_visible(&mut self, view_h: usize) {
        if self.ui.db_cursor.row < self.ui.db_scroll_row {
            self.ui.db_scroll_row = self.ui.db_cursor.row;
        } else if self.ui.db_cursor.row >= self.ui.db_scroll_row + view_h {
            self.ui.db_scroll_row = self.ui.db_cursor.row.saturating_sub(view_h.saturating_sub(1));
        }
    }

    fn explorer_view_h(term_rows: u16) -> usize {
        // Studio: body ≈ height-2, explorer is ~58% of that minus border.
        let body = term_rows.saturating_sub(2) as usize;
        ((body * 58) / 100).saturating_sub(2).max(4)
    }

    async fn db_explorer_moved(&mut self, term_rows: u16) {
        self.db
            .ensure_explorer_visible(Self::explorer_view_h(term_rows));
        // Still refresh Structure (properties) even when the source has a connect error.
        let connect_err: Option<(String, String)> =
            self.db.flat.get(self.db.selected).and_then(|entry| {
                if !matches!(entry.kind, DbNodeKind::Source) {
                    return None;
                }
                let msg = self
                    .db
                    .connection_health
                    .get(&entry.source_id)
                    .and_then(|h| h.error_message())?
                    .to_string();
                let name = self
                    .db
                    .sources
                    .iter()
                    .find(|s| s.id == entry.source_id)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "source".into());
                Some((name, msg))
            });
        if let Err(e) = self.db.refresh_structure_for_selection().await {
            self.set_status(format!("structure: {e:#}"), true);
        }
        if let Some((name, msg)) = connect_err {
            self.set_status(format!("{name} · connect error · {msg}"), true);
        }
    }

    async fn structure_confirm(&mut self, term_rows: u16) -> Result<()> {
        let Some(line) = self.db.structure_lines.get(self.db.structure_scroll).cloned() else {
            return Ok(());
        };
        let Some(nav) = line.nav else {
            return Ok(());
        };
        if self.db.select_table_in_explorer(
            &nav.source_id,
            nav.database.as_deref(),
            &nav.schema,
            &nav.table,
        ) {
            self.db
                .ensure_explorer_visible(Self::explorer_view_h(term_rows));
            match self.db.open_selected_table_tab().await {
                Ok(true) => {
                    self.ui.focus = Focus::Editor;
                    self.set_status(format!("FK → {}.{}", nav.schema, nav.table), false);
                }
                Ok(false) => {}
                Err(e) => self.set_status(format!("fk: {e:#}"), true),
            }
        } else {
            self.set_status(
                format!(
                    "FK → {}.{} (table not in explorer — expand schema)",
                    nav.schema, nav.table
                ),
                false,
            );
        }
        Ok(())
    }

    async fn activate_db_selection(&mut self, term_rows: u16) -> Result<()> {
        let kind = self.db.flat.get(self.db.selected).map(|e| e.kind.clone());
        match kind {
            Some(DbNodeKind::Table) | Some(DbNodeKind::View) => {
                match self.db.open_selected_table_tab().await {
                    Ok(true) => {
                        self.ui.db_cursor = Cursor::default();
                        self.ui.db_scroll_row = 0;
                        self.ui.focus = Focus::Results;
                        self.ui.bottom = BottomPanel::Results;
                        self.ui.mode = Mode::Normal;
                        self.run_query().await?;
                    }
                    Ok(false) => {}
                    Err(e) => self.set_status(format!("table: {e:#}"), true),
                }
            }
            Some(DbNodeKind::Routine) => match self.db.script_routine_at_selection().await {
                Ok(()) => {
                    self.ui.focus = Focus::Editor;
                    self.ui.bottom = BottomPanel::Results;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("script: {e:#}"), true),
            },
            Some(DbNodeKind::AdminQuery) => {
                let meta = self.db.flat.get(self.db.selected).map(|e| {
                    (
                        e.label.clone(),
                        e.table.clone().unwrap_or_else(|| e.label.clone()),
                    )
                });
                match self.db.open_admin_query_tab().await {
                    Ok(true) => {
                        self.ui.db_cursor = Cursor::default();
                        self.ui.db_scroll_row = 0;
                        self.ui.bottom = BottomPanel::Results;
                        self.ui.mode = Mode::Normal;
                        self.ui.cell_modal = false;
                        self.ui.cell_modal_drill = false;
                        if let Some((label, slug)) = meta {
                            self.ui.admin_modal = true;
                            self.ui.admin_modal_title = label;
                            self.ui.admin_modal_view = slug;
                            self.ui.admin_modal_row = 0;
                        }
                        // Keep Explorer focus under the modal so Esc isn't stolen by Results.
                        self.ui.focus = Focus::Database;
                        self.set_status(
                            format!(
                                "admin · {} · r refresh · Esc",
                                self.ui.admin_modal_title
                            ),
                            false,
                        );
                        self.run_query().await?;
                    }
                    Ok(false) => {}
                    Err(e) => self.set_status(format!("admin: {e:#}"), true),
                }
            }
            Some(DbNodeKind::AdminAction) => {
                match self.db.selected_admin_action().as_deref() {
                    Some("edit-connection") => {
                        if let Some(source_id) = self
                            .db
                            .flat
                            .get(self.db.selected)
                            .map(|e| e.source_id.clone())
                        {
                            self.start_edit_connection(&source_id);
                        }
                    }
                    Some("duplicate-connection") => {
                        if let Some(source_id) = self
                            .db
                            .flat
                            .get(self.db.selected)
                            .map(|e| e.source_id.clone())
                        {
                            match self.db.duplicate_source(&source_id) {
                                Ok(_) => self.set_status(self.db.status.clone(), false),
                                Err(e) => self.set_status(format!("duplicate: {e:#}"), true),
                            }
                        }
                    }
                    Some("delete-connection") => {
                        if let Some(source_id) = self
                            .db
                            .flat
                            .get(self.db.selected)
                            .map(|e| e.source_id.clone())
                        {
                            self.start_delete_connection(&source_id);
                        }
                    }
                    Some("vacuum-analyze") => {
                        if let Err(e) = self
                            .db
                            .run_postgres_maintenance(PostgresMaintenance::VacuumAnalyze)
                            .await
                        {
                            self.set_status(format!("vacuum: {e:#}"), true);
                        } else {
                            self.ui.focus = Focus::Results;
                            self.ui.bottom = BottomPanel::Results;
                            self.set_status(self.db.status.clone(), false);
                        }
                    }
                    Some("analyze-table") => {
                        if let Err(e) = self
                            .db
                            .run_postgres_maintenance(PostgresMaintenance::Analyze)
                            .await
                        {
                            self.set_status(format!("analyze: {e:#}"), true);
                        } else {
                            self.ui.focus = Focus::Results;
                            self.ui.bottom = BottomPanel::Results;
                            self.set_status(self.db.status.clone(), false);
                        }
                    }
                    Some("reindex-table") => {
                        if let Err(e) = self
                            .db
                            .run_postgres_maintenance(PostgresMaintenance::Reindex)
                            .await
                        {
                            self.set_status(format!("reindex: {e:#}"), true);
                        } else {
                            self.ui.focus = Focus::Results;
                            self.ui.bottom = BottomPanel::Results;
                            self.set_status(self.db.status.clone(), false);
                        }
                    }
                    Some("dump") => self.dump_database().await?,
                    Some("restore") => self.start_restore_dump(),
                    Some("schema-diff") => self.start_schema_diff(),
                    Some("schema-diff-ddl") => self.generate_schema_diff_ddl().await?,
                    Some("data-compare") => self.start_data_compare(),
                    Some("txn-begin") => {
                        let _ = self.db.begin_transaction().await.map(|()| {
                            self.set_status(self.db.status.clone(), false);
                        }).map_err(|e| self.set_status(format!("begin txn: {e:#}"), true));
                    }
                    Some("txn-commit") => {
                        let _ = self.db.commit_transaction().await.map(|()| {
                            self.set_status(self.db.status.clone(), false);
                        }).map_err(|e| self.set_status(format!("commit txn: {e:#}"), true));
                    }
                    Some("txn-rollback") => {
                        let _ = self.db.rollback_transaction().await.map(|()| {
                            self.set_status(self.db.status.clone(), false);
                        }).map_err(|e| self.set_status(format!("rollback txn: {e:#}"), true));
                    }
                    Some("chart-results") => match self.db.chart_last_result(None) {
                        Ok(_) => {
                            self.ui.focus = Focus::Results;
                            self.set_status("chart appended in Results", false);
                        }
                        Err(e) => self.set_status(format!("chart: {e:#}"), true),
                    },
                    Some("save-result-temp") => {
                        self.ui.save_materialize_temp = true;
                        self.ui.mode = Mode::SaveMaterialize;
                        self.ui.import_csv_input.clear();
                        self.set_status("name for temp table · Enter · Esc cancel", false);
                    }
                    Some("save-result-view") => {
                        self.ui.save_materialize_temp = false;
                        self.ui.mode = Mode::SaveMaterialize;
                        self.ui.import_csv_input.clear();
                        self.set_status("name for view · Enter · Esc cancel", false);
                    }
                    Some("save-query") => {
                        match self.db.save_query_file(None, &self.db.sql.clone()) {
                            Ok(p) => self.set_status(format!("saved {}", p.display()), false),
                            Err(e) => self.set_status(format!("save: {e:#}"), true),
                        }
                    }
                    Some("sync-push") => match self.db.sync_queries_push() {
                        Ok(msg) => self.set_status(msg, false),
                        Err(e) => self.set_status(format!("sync push: {e:#}"), true),
                    },
                    Some("sync-pull") => match self.db.sync_queries_pull() {
                        Ok(msg) => self.set_status(msg, false),
                        Err(e) => self.set_status(format!("sync pull: {e:#}"), true),
                    },
                    other => self.set_status(format!("unknown admin action: {other:?}"), true),
                }
            }
            Some(_) => {
                if let Err(e) = self.db.toggle_selected().await {
                    self.set_status(format!("connect error · {e:#}"), true);
                } else {
                    self.db
                        .ensure_explorer_visible(Self::explorer_view_h(term_rows));
                    let _ = self.db.refresh_structure_for_selection().await;
                }
            }
            None => {}
        }
        Ok(())
    }

    fn db_clamp_cursor(&mut self) {
        let rows = self.db_line_count();
        if self.ui.db_cursor.row >= rows {
            self.ui.db_cursor.row = rows.saturating_sub(1);
        }
        let len = self.db_line(self.ui.db_cursor.row).chars().count();
        if self.ui.db_cursor.col > len {
            self.ui.db_cursor.col = len;
        }
    }

    fn db_insert_char(&mut self, c: char) {
        let replaced = self.clear_db_sql_selection_or_all();
        if !replaced {
            self.db_push_undo();
        }
        let row = self.ui.db_cursor.row;
        let col = self.ui.db_cursor.col;
        let mut lines: Vec<String> = if self.db.sql.is_empty() {
            vec![String::new()]
        } else {
            self.db.sql.lines().map(|l| l.to_string()).collect()
        };
        while lines.len() <= row {
            lines.push(String::new());
        }
        let line = if col >= lines[row].chars().count() {
            format!("{}{}", lines[row], c)
        } else {
            let mut s: String = lines[row].chars().take(col).collect();
            s.push(c);
            s.push_str(&lines[row].chars().skip(col).collect::<String>());
            s
        };
        lines[row] = line;
        self.db.sql = lines.join("\n");
        self.sync_active_sql_tab();
        self.ui.db_cursor.col += 1;
        self.db_clamp_cursor();
    }

    fn sync_active_sql_tab(&mut self) {
        if let Some(tab) = self.db.tabs.get_mut(self.db.active_tab) {
            tab.sql = self.db.sql.clone();
        }
    }

    fn db_insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let replaced = self.clear_db_sql_selection_or_all();
        if !replaced {
            self.db_push_undo();
        }
        for ch in text.chars() {
            if ch == '\n' || ch == '\r' {
                if ch == '\r' {
                    continue;
                }
                self.db_insert_newline();
            } else {
                // Avoid double undo frames per char — manually insert.
                let row = self.ui.db_cursor.row;
                let col = self.ui.db_cursor.col;
                let mut lines: Vec<String> = if self.db.sql.is_empty() {
                    vec![String::new()]
                } else {
                    self.db.sql.lines().map(|l| l.to_string()).collect()
                };
                while lines.len() <= row {
                    lines.push(String::new());
                }
                let line = if col >= lines[row].chars().count() {
                    format!("{}{}", lines[row], ch)
                } else {
                    let mut s: String = lines[row].chars().take(col).collect();
                    s.push(ch);
                    s.push_str(&lines[row].chars().skip(col).collect::<String>());
                    s
                };
                lines[row] = line;
                self.db.sql = lines.join("\n");
                self.ui.db_cursor.col += 1;
                self.db_clamp_cursor();
            }
        }
        self.sync_active_sql_tab();
    }

    fn clipboard_copy_sql_or_editor(&mut self) {
        let text = if self.editing_db_sql() {
            self.db.sql.clone()
        } else if let Some(buf) = self.active_buffer() {
            buf.text()
        } else {
            self.set_status("copy: nothing to copy", true);
            return;
        };
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.clone())) {
            Ok(()) => self.set_status(
                format!("copied {} chars", text.chars().count()),
                false,
            ),
            Err(e) => self.set_status(format!("clipboard: {e}"), true),
        }
    }

    fn clipboard_paste_sql_or_editor(&mut self) {
        let text = match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(t) => t,
            Err(e) => {
                self.set_status(format!("clipboard: {e}"), true);
                return;
            }
        };
        if text.is_empty() {
            self.set_status("clipboard empty", true);
            return;
        }
        if self.editing_db_sql() {
            if self.ui.mode != Mode::Insert {
                self.ui.mode = Mode::Insert;
            }
            self.db_insert_text(&text);
            self.set_status(format!("pasted {} chars", text.chars().count()), false);
        } else if let Some(buf) = self.active_buffer_mut() {
            for ch in text.chars() {
                if ch == '\n' {
                    buf.insert_newline();
                } else if ch != '\r' {
                    buf.insert_char(ch);
                }
            }
            self.set_status(format!("pasted {} chars", text.chars().count()), false);
        } else {
            self.set_status("paste: focus Console (3) first", true);
        }
    }

    fn select_all_sql_or_editor(&mut self) {
        if self.editing_db_sql() {
            self.ui.db_sql_all_selected = !self.db.sql.is_empty();
            self.ui.db_cursor.row = 0;
            self.ui.db_cursor.col = 0;
            self.ui.db_scroll_row = 0;
            if self.ui.mode != Mode::Insert {
                self.ui.mode = Mode::Insert;
            }
            let n = self.db.sql.chars().count();
            if self.ui.db_sql_all_selected {
                self.set_status(
                    format!("all selected · {n} chars · Backspace/Delete clear · type replaces"),
                    false,
                );
            } else {
                self.set_status("Console empty", false);
            }
        } else if self.ui.product == ProductMode::Database
            && self.ui.db_surface == DbSurface::Studio
        {
            // Ctrl-A from another pane — jump to Console and select.
            self.ui.focus = Focus::Editor;
            self.ui.mode = Mode::Insert;
            self.select_all_sql_or_editor();
        } else if let Some(buf) = self.active_buffer_mut() {
            buf.cursor.row = 0;
            buf.cursor.col = 0;
            self.set_status("Select all · Ctrl-C copy buffer", false);
        } else {
            self.set_status("select all: focus Console (3)", true);
        }
    }

    fn clear_db_sql_selection_or_all(&mut self) -> bool {
        if !self.ui.db_sql_all_selected {
            return false;
        }
        self.db_push_undo();
        self.db.sql.clear();
        self.sync_active_sql_tab();
        self.ui.db_sql_all_selected = false;
        self.ui.db_cursor.row = 0;
        self.ui.db_cursor.col = 0;
        self.ui.db_scroll_row = 0;
        self.ui.show_completions = false;
        true
    }

    fn db_insert_newline(&mut self) {
        if self.clear_db_sql_selection_or_all() {
            return;
        }
        self.db_push_undo();
        let row = self.ui.db_cursor.row;
        let col = self.ui.db_cursor.col;
        let mut lines: Vec<String> = if self.db.sql.is_empty() {
            vec![String::new()]
        } else {
            self.db.sql.lines().map(|l| l.to_string()).collect()
        };
        while lines.len() <= row {
            lines.push(String::new());
        }
        let current = lines[row].clone();
        let head: String = current.chars().take(col).collect();
        let tail: String = current.chars().skip(col).collect();
        lines[row] = head;
        lines.insert(row + 1, tail);
        self.db.sql = if lines.len() == 1 && lines[0].is_empty() {
            "\n".into()
        } else {
            format!("{}\n", lines.join("\n"))
        };
        self.sync_active_sql_tab();
        self.ui.db_cursor.row += 1;
        self.ui.db_cursor.col = 0;
        self.db_clamp_cursor();
    }

    fn db_backspace(&mut self) {
        if self.clear_db_sql_selection_or_all() {
            return;
        }
        self.db_push_undo();
        if self.ui.db_cursor.col > 0 {
            let row = self.ui.db_cursor.row;
            let col = self.ui.db_cursor.col - 1;
            let mut lines: Vec<String> = self.db.sql.lines().map(|l| l.to_string()).collect();
            if lines.is_empty() {
                lines.push(String::new());
            }
            while lines.len() <= row {
                lines.push(String::new());
            }
            let s: String = lines[row]
                .chars()
                .enumerate()
                .filter_map(|(i, c)| if i == col { None } else { Some(c) })
                .collect();
            lines[row] = s;
            self.db.sql = lines.join("\n");
            self.ui.db_cursor.col = col;
        } else if self.ui.db_cursor.row > 0 {
            let row = self.ui.db_cursor.row;
            let mut lines: Vec<String> = self.db.sql.lines().map(|l| l.to_string()).collect();
            let prev_len = lines[row - 1].chars().count();
            let tail = lines.remove(row);
            lines[row - 1].push_str(&tail);
            self.db.sql = lines.join("\n");
            self.ui.db_cursor.row -= 1;
            self.ui.db_cursor.col = prev_len;
        }
        self.sync_active_sql_tab();
        self.ui.show_completions = false;
        self.db_clamp_cursor();
    }

    fn db_delete_forward(&mut self) {
        if self.clear_db_sql_selection_or_all() {
            return;
        }
        self.db_push_undo();
        let row = self.ui.db_cursor.row;
        let col = self.ui.db_cursor.col;
        let mut lines: Vec<String> = if self.db.sql.is_empty() {
            vec![String::new()]
        } else {
            self.db.sql.lines().map(|l| l.to_string()).collect()
        };
        while lines.len() <= row {
            lines.push(String::new());
        }
        let len = lines[row].chars().count();
        if col < len {
            let s: String = lines[row]
                .chars()
                .enumerate()
                .filter_map(|(i, c)| if i == col { None } else { Some(c) })
                .collect();
            lines[row] = s;
            self.db.sql = lines.join("\n");
        } else if row + 1 < lines.len() {
            let next = lines.remove(row + 1);
            lines[row].push_str(&next);
            self.db.sql = lines.join("\n");
        }
        self.sync_active_sql_tab();
        self.ui.show_completions = false;
        self.db_clamp_cursor();
    }

    fn db_clear_selection_mark(&mut self) {
        self.ui.db_sql_all_selected = false;
    }

    fn db_move_left(&mut self) {
        self.db_clear_selection_mark();
        if self.ui.db_cursor.col > 0 {
            self.ui.db_cursor.col -= 1;
        } else if self.ui.db_cursor.row > 0 {
            self.ui.db_cursor.row -= 1;
            self.ui.db_cursor.col = self.db_line(self.ui.db_cursor.row).chars().count();
        }
    }

    fn db_move_right(&mut self) {
        self.db_clear_selection_mark();
        let len = self.db_line(self.ui.db_cursor.row).chars().count();
        if self.ui.db_cursor.col < len {
            self.ui.db_cursor.col += 1;
        } else if self.ui.db_cursor.row + 1 < self.db_line_count() {
            self.ui.db_cursor.row += 1;
            self.ui.db_cursor.col = 0;
        }
    }

    fn db_move_up(&mut self) {
        self.db_clear_selection_mark();
        if self.ui.db_cursor.row > 0 {
            self.ui.db_cursor.row -= 1;
            self.db_clamp_cursor();
        }
    }

    fn db_move_down(&mut self) {
        self.db_clear_selection_mark();
        if self.ui.db_cursor.row + 1 < self.db_line_count() {
            self.ui.db_cursor.row += 1;
            self.db_clamp_cursor();
        }
    }

    fn db_move_line_start(&mut self) {
        self.db_clear_selection_mark();
        self.ui.db_cursor.col = 0;
    }

    fn db_move_line_end(&mut self) {
        self.db_clear_selection_mark();
        self.ui.db_cursor.col = self.db_line(self.ui.db_cursor.row).chars().count();
    }

    fn editing_db_sql(&self) -> bool {
        self.ui.product == ProductMode::Database
            && self.ui.db_surface == DbSurface::Studio
            && self.ui.focus == Focus::Editor
    }

    async fn run_query(&mut self) -> Result<()> {
        if sql_has_query_params(self.db.sql.trim()) {
            self.ui.mode = Mode::QueryParams;
            self.ui.query_params_for_explain = false;
            self.ui.import_csv_input = query_param_prompt_seed(self.db.sql.trim());
            self.set_status("query params — key=value,… · Enter run · Esc cancel", false);
            return Ok(());
        }
        self.queue_db_query();
        Ok(())
    }

    fn queue_db_query(&mut self) {
        self.db.result_page = 0;
        // `query_running` is owned by execute_sql; busy_label covers the UI gap.
        self.pending_db_query = true;
        self.ui.busy_label = Some("RUNNING…".into());
        self.set_status("RUNNING… Esc cancels", false);
    }

    fn finish_query_ui(&mut self) {
        self.db.query_running = false;
        self.ui.busy_label = None;
        self.ui.results_col_window = 0;
        self.ui.cell_modal = false;
        self.ui.cell_modal_drill = false;
    }

    fn show_query_error(&mut self, msg: String) {
        self.finish_query_ui();
        self.db.last_result = Some(ide_db::QueryResult {
            columns: vec!["error".into()],
            rows: vec![vec![msg.clone()]],
            affected: None,
            elapsed_ms: 0,
            truncated: false,
            source_schema: None,
            source_table: None,
            primary_keys: Vec::new(),
            original_rows: vec![vec![msg.clone()]],
            cursor_row: 0,
            cursor_col: 0,
            explain: None,
            chart: None,
            page: 0,
            has_more: false,
        });
        // Stay in Console Insert so Backspace / typing work immediately after dismiss.
        self.ui.focus = Focus::Editor;
        self.ui.mode = Mode::Insert;
        self.ui.bottom = BottomPanel::Results;
        self.ui.query_error = Some(msg.clone());
        self.set_status(format!("query: {msg}"), true);
    }

    async fn run_query_executed(&mut self) -> Result<()> {
        self.ui.bottom = BottomPanel::Results;
        // Prefer the explorer selection's source/db when Console has no active target yet.
        if self.db.active_source.is_none() {
            if let Some(entry) = self.db.flat.get(self.db.selected).cloned() {
                self.db.active_source = Some(entry.source_id);
                if entry.database.is_some() {
                    self.db.active_database = entry.database;
                }
            } else if let Some(src) = self.db.sources.first() {
                self.db.active_source = Some(src.id.clone());
            }
        }
        let result = self.db.execute_sql().await;
        // Always clear RUNNING chrome — execute_sql can fail before/after its guard.
        self.finish_query_ui();
        match result {
            Ok(()) => {
                self.ui.focus = Focus::Results;
                self.ui.mode = Mode::Normal;
                self.ui.query_error = None;
                let _ = self.db.refresh_structure_from_console_query().await;
                self.set_status(self.db.status.clone(), false);
            }
            Err(e) => {
                self.show_query_error(format!("{e:#}"));
            }
        }
        Ok(())
    }

    async fn explain_query(&mut self) -> Result<()> {
        if self.ui.product != ProductMode::Database {
            self.set_status("explain: switch to database mode (8)", true);
            return Ok(());
        }
        self.ui.bottom = BottomPanel::Results;
        if sql_has_query_params(self.db.sql.trim()) {
            self.ui.mode = Mode::QueryParams;
            self.ui.query_params_for_explain = true;
            self.ui.import_csv_input = query_param_prompt_seed(self.db.sql.trim());
            self.set_status("query params for EXPLAIN — Enter · Esc cancel", false);
            return Ok(());
        }
        match self.db.explain_sql().await {
            Ok(()) => {
                self.ui.focus = Focus::Results;
                self.set_status(self.db.status.clone(), false);
            }
            Err(e) => self.set_status(format!("explain: {e:#}"), true),
        }
        Ok(())
    }

    async fn apply_row_updates(&mut self) -> Result<()> {
        match self.db.pending_updates_preview(16) {
            Ok(preview) => {
                self.ui.confirm_apply_pending = true;
                self.ui.confirm_delete_source_id = None;
                self.ui.confirm_delete_label = preview;
                self.ui.mode = Mode::ConfirmDeleteSource;
            }
            Err(e) => self.set_status(format!("apply: {e:#}"), true),
        }
        Ok(())
    }

    async fn confirm_apply_updates(&mut self) -> Result<()> {
        self.ui.confirm_apply_pending = false;
        self.ui.confirm_delete_label.clear();
        self.ui.mode = Mode::Normal;
        match self.db.apply_pending_updates().await {
            Ok(n) => self.set_status(format!("applied {n} update(s)"), false),
            Err(e) => self.set_status(format!("apply: {e:#}"), true),
        }
        Ok(())
    }

    fn export_stamp() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("{secs}")
    }

    fn export_results_csv(&mut self) -> Result<()> {
        let path = self
            .workspace
            .root
            .join("exports")
            .join(format!("result-{}.csv", Self::export_stamp()));
        let filt = self.db.results_filter.trim();
        match self.db.export_csv(&path) {
            Ok(()) => {
                let tag = if filt.is_empty() {
                    String::new()
                } else {
                    format!(" · filter /{filt}")
                };
                self.set_status(format!("exported {}{tag}", path.display()), false);
            }
            Err(e) => self.set_status(format!("export: {e:#}"), true),
        }
        Ok(())
    }

    fn export_results_sql(&mut self) -> Result<()> {
        let path = self
            .workspace
            .root
            .join("exports")
            .join(format!("result-{}.sql", Self::export_stamp()));
        let filt = self.db.results_filter.trim();
        match self.db.export_sql(&path) {
            Ok(()) => {
                let tag = if filt.is_empty() {
                    String::new()
                } else {
                    format!(" · filter /{filt}")
                };
                self.set_status(format!("exported {}{tag}", path.display()), false);
            }
            Err(e) => self.set_status(format!("export sql: {e:#}"), true),
        }
        Ok(())
    }

    fn export_results_json(&mut self) -> Result<()> {
        let path = self
            .workspace
            .root
            .join("exports")
            .join(format!("result-{}.json", Self::export_stamp()));
        let filt = self.db.results_filter.trim();
        match self.db.export_json(&path) {
            Ok(()) => {
                let tag = if filt.is_empty() {
                    String::new()
                } else {
                    format!(" · filter /{filt}")
                };
                self.set_status(format!("exported {}{tag}", path.display()), false);
            }
            Err(e) => self.set_status(format!("export json: {e:#}"), true),
        }
        Ok(())
    }

    fn export_results_markdown(&mut self) -> Result<()> {
        let path = self
            .workspace
            .root
            .join("exports")
            .join(format!("result-{}.md", Self::export_stamp()));
        let filt = self.db.results_filter.trim();
        match self.db.export_markdown(&path) {
            Ok(()) => {
                let tag = if filt.is_empty() {
                    String::new()
                } else {
                    format!(" · filter /{filt}")
                };
                self.set_status(format!("exported {}{tag}", path.display()), false);
            }
            Err(e) => self.set_status(format!("export md: {e:#}"), true),
        }
        Ok(())
    }

    fn start_alter_column(&mut self, kind: AlterColumnKind) {
        let marker = match kind {
            AlterColumnKind::Add => "__alter_add__",
            AlterColumnKind::Drop => "__alter_drop__",
            AlterColumnKind::Rename => "__alter_rename__",
        };
        let seed = match kind {
            AlterColumnKind::Add => "new_col TEXT",
            AlterColumnKind::Drop => "column_name",
            AlterColumnKind::Rename => "old_name=new_name",
        };
        self.ui.mode = Mode::ImportCsv;
        self.ui.import_csv_input = seed.into();
        self.ui.confirm_run_sql = Some(marker.into());
        self.set_status("ALTER column · Enter to confirm dialog · Esc cancel", false);
    }

    fn start_import_json(&mut self) {
        self.ui.mode = Mode::ImportCsv;
        self.ui.confirm_run_sql = Some("__import_json__".into());
        self.ui.import_csv_input = self
            .workspace
            .root
            .join("exports")
            .join("result.json")
            .display()
            .to_string();
    }

    fn start_migrate_apply(&mut self) {
        self.ui.mode = Mode::ImportCsv;
        self.ui.confirm_run_sql = Some("__migrate_apply__".into());
        self.ui.import_csv_input = self
            .db
            .latest_migration_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| {
                self.workspace
                    .root
                    .join("queries")
                    .join("migrate_.sql")
                    .display()
                    .to_string()
            });
        self.set_status("migration path · Enter apply · Esc cancel", false);
    }

    fn start_migrate_dry_run(&mut self) {
        self.ui.mode = Mode::ImportCsv;
        self.ui.confirm_run_sql = Some("__migrate_dry_run__".into());
        self.ui.import_csv_input = self
            .db
            .latest_migration_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| {
                self.workspace
                    .root
                    .join("queries")
                    .join("migrate_.sql")
                    .display()
                    .to_string()
            });
        self.set_status("migration dry-run path · Enter · Esc cancel", false);
    }

    fn start_delete_result_row(&mut self) {
        match self.db.delete_result_row() {
            Ok(()) => {
                self.ui.focus = Focus::Results;
                self.set_status(self.db.status.clone(), false);
            }
            Err(e) => self.set_status(format!("delete row: {e:#}"), true),
        }
    }

    fn start_destructive_sql(&mut self, truncate: bool) {
        match self.db.destructive_sql_for_selection(truncate) {
            Ok((sql, label)) => {
                self.ui.confirm_apply_pending = false;
                self.ui.confirm_delete_source_id = None;
                self.ui.confirm_run_sql = Some(sql);
                self.ui.confirm_delete_label = label;
                self.ui.mode = Mode::ConfirmDeleteSource;
            }
            Err(e) => self.set_status(format!("destructive: {e:#}"), true),
        }
    }

    async fn confirm_destructive_sql(&mut self) -> Result<()> {
        let Some(sql) = self.ui.confirm_run_sql.take() else {
            self.ui.mode = Mode::Normal;
            return Ok(());
        };
        self.ui.confirm_delete_label.clear();
        self.ui.mode = Mode::Normal;
        self.db.sql = sql;
        self.queue_db_query();
        Ok(())
    }

    async fn save_schema_diff_migration(&mut self) -> Result<()> {
        // Prefer generating DDL first if Results look like a schema diff.
        if self
            .db
            .last_result
            .as_ref()
            .is_some_and(|r| r.columns.first().map(|c| c.as_str()) == Some("change"))
        {
            if let Err(e) = self.db.generate_schema_diff_ddl().await {
                self.set_status(format!("schema diff DDL: {e:#}"), true);
                return Ok(());
            }
        }
        match self.db.save_migration_file() {
            Ok(path) => {
                self.ui.focus = Focus::Editor;
                self.set_status(format!("migration {}", path.display()), false);
            }
            Err(e) => self.set_status(format!("migration: {e:#}"), true),
        }
        Ok(())
    }

    fn start_import_csv(&mut self) {
        self.ui.mode = Mode::ImportCsv;
        self.ui.confirm_run_sql = None;
        self.ui.import_csv_input = self
            .workspace
            .root
            .join("exports")
            .join("result.csv")
            .display()
            .to_string();
    }

    async fn dump_database(&mut self) -> Result<()> {
        let out_dir = self.workspace.root.join("exports");
        match self.db.dump_database(&out_dir).await {
            Ok(path) => self.set_status(format!("dumped {}", path.display()), false),
            Err(e) => self.set_status(format!("dump: {e:#}"), true),
        }
        Ok(())
    }

    fn start_restore_dump(&mut self) {
        self.ui.mode = Mode::RestoreDump;
        self.ui.import_csv_input = self.workspace.root.join("exports").display().to_string() + "/";
    }

    fn start_schema_diff(&mut self) {
        self.ui.mode = Mode::SchemaDiff;
        self.ui.import_csv_input = self.db.schema_diff_prompt_seed();
    }

    fn start_data_compare(&mut self) {
        self.ui.mode = Mode::DataCompare;
        self.ui.import_csv_input = self.db.data_compare_prompt_seed();
    }

    async fn confirm_data_compare(&mut self) -> Result<()> {
        let raw = self.ui.import_csv_input.trim().to_string();
        self.ui.import_csv_input.clear();
        self.ui.mode = Mode::Normal;
        if raw.is_empty() || raw.ends_with("..") {
            self.set_status("data compare cancelled — need left..right", true);
            return Ok(());
        }
        match self.db.run_data_compare(&raw).await {
            Ok(()) => {
                self.ui.db_cursor = Cursor::default();
                self.ui.db_scroll_row = 0;
                self.ui.focus = Focus::Results;
                self.ui.bottom = BottomPanel::Results;
                self.set_status(self.db.status.clone(), false);
            }
            Err(e) => self.set_status(format!("data compare: {e:#}"), true),
        }
        Ok(())
    }

    async fn generate_schema_diff_ddl(&mut self) -> Result<()> {
        match self.db.generate_schema_diff_ddl().await {
            Ok(()) => {
                self.ui.focus = Focus::Editor;
                self.ui.bottom = BottomPanel::Results;
                self.set_status(self.db.status.clone(), false);
            }
            Err(e) => self.set_status(format!("schema diff DDL: {e:#}"), true),
        }
        Ok(())
    }

    async fn confirm_schema_diff(&mut self) -> Result<()> {
        let raw = self.ui.import_csv_input.trim().to_string();
        self.ui.import_csv_input.clear();
        self.ui.mode = Mode::Normal;
        if raw.is_empty() || raw.ends_with("..") {
            self.set_status("schema diff cancelled — need left..right", true);
            return Ok(());
        }
        match self.db.run_schema_diff(&raw).await {
            Ok(()) => {
                self.ui.db_cursor = Cursor::default();
                self.ui.db_scroll_row = 0;
                self.ui.focus = Focus::Results;
                self.ui.bottom = BottomPanel::Results;
                self.set_status(self.db.status.clone(), false);
            }
            Err(e) => self.set_status(format!("schema diff: {e:#}"), true),
        }
        Ok(())
    }

    async fn confirm_restore_dump(&mut self) -> Result<()> {
        let raw = self.ui.import_csv_input.trim().to_string();
        self.ui.import_csv_input.clear();
        self.ui.mode = Mode::Normal;
        if raw.is_empty() {
            return Ok(());
        }
        let path = if raw.starts_with("~/") {
            dirs::home_dir()
                .map(|h| h.join(raw.trim_start_matches("~/")))
                .unwrap_or_else(|| PathBuf::from(&raw))
        } else {
            let p = PathBuf::from(&raw);
            if p.is_absolute() {
                p
            } else {
                self.workspace.root.join(p)
            }
        };
        match self.db.restore_database(&path).await {
            Ok(()) => self.set_status(format!("restored {}", path.display()), false),
            Err(e) => self.set_status(format!("restore: {e:#}"), true),
        }
        Ok(())
    }

    async fn kill_session(&mut self) -> Result<()> {
        match self.db.kill_session_at_cursor().await {
            Ok(()) => self.set_status(self.db.status.clone(), false),
            Err(e) => self.set_status(format!("kill: {e:#}"), true),
        }
        Ok(())
    }

    async fn run_mssql_health(&mut self, which: &str) -> Result<()> {
        match self.db.open_mssql_health(which).await {
            Ok(_) => {
                self.ui.focus = Focus::Editor;
                self.ui.bottom = BottomPanel::Results;
                self.queue_db_query();
            }
            Err(e) => self.set_status(format!("mssql health: {e:#}"), true),
        }
        Ok(())
    }

    async fn run_health(&mut self, which: &str) -> Result<()> {
        match self.db.open_health(which).await {
            Ok(_) => {
                self.ui.focus = Focus::Editor;
                self.ui.bottom = BottomPanel::Results;
                self.queue_db_query();
            }
            Err(e) => self.set_status(format!("health: {e:#}"), true),
        }
        Ok(())
    }

    fn start_brain_ask(&mut self) {
        self.ui.oauth_paste_pending = false;
        self.ui.rename_tab_pending = false;
        self.ui.import_csv_input.clear();
        self.ui.mode = Mode::BrainAsk;
    }

    fn start_oauth_paste(&mut self) {
        self.ui.oauth_paste_pending = true;
        self.ui.import_csv_input.clear();
        self.ui.mode = Mode::BrainAsk;
        self.set_status("paste OAuth access token · Enter", false);
    }

    async fn confirm_brain_ask(&mut self) -> Result<()> {
        let q = self.ui.import_csv_input.trim().to_string();
        self.ui.import_csv_input.clear();
        self.ui.mode = Mode::Normal;
        if self.ui.rename_tab_pending {
            self.ui.rename_tab_pending = false;
            if self.db.rename_active_tab(&q) {
                self.set_status(self.db.status.clone(), false);
            } else {
                self.set_status(self.db.status.clone(), true);
            }
            return Ok(());
        }
        if self.ui.oauth_paste_pending {
            self.ui.oauth_paste_pending = false;
            if q.is_empty() {
                self.set_status("OAuth paste cancelled", false);
                return Ok(());
            }
            let driver = self
                .db
                .active_source
                .as_ref()
                .and_then(|id| self.db.sources.iter().find(|s| &s.id == id))
                .map(|s| s.driver)
                .unwrap_or(DbDriver::Bigquery);
            match ide_db::apply_pasted_token(driver, &q) {
                Ok(msg) => self.set_status(msg, false),
                Err(e) => self.set_status(format!("oauth: {e:#}"), true),
            }
            return Ok(());
        }
        if q.is_empty() {
            self.set_status("brain ask cancelled", false);
            return Ok(());
        }
        self.set_status("brain asking…", false);
        match ide_db::brain_ask(&q).await {
            Ok(text) => {
                let rows: Vec<Vec<String>> = text
                    .lines()
                    .map(|l| vec![l.to_string()])
                    .collect();
                let n = rows.len();
                self.db.last_result = Some(ide_db::QueryResult {
                    columns: vec!["brain".into()],
                    rows,
                    affected: None,
                    elapsed_ms: 0,
                    truncated: false,
                    source_schema: None,
                    source_table: Some("brain_ask".into()),
                    primary_keys: Vec::new(),
                    original_rows: Vec::new(),
                    cursor_row: 0,
                    cursor_col: 0,
                    explain: None,
                    chart: None,
                    page: 0,
                    has_more: false,
                });
                self.ui.focus = Focus::Results;
                self.set_status(format!("brain ask · {n} lines"), false);
            }
            Err(e) => self.set_status(format!("brain ask: {e:#}"), true),
        }
        Ok(())
    }

    async fn confirm_save_materialize(&mut self) -> Result<()> {
        let name = self.ui.import_csv_input.trim().to_string();
        let as_temp = self.ui.save_materialize_temp;
        self.ui.import_csv_input.clear();
        self.ui.mode = Mode::Normal;
        if name.is_empty() {
            self.set_status("save cancelled", true);
            return Ok(());
        }
        match self.db.materialize_last_select(&name, as_temp).await {
            Ok(ddl) => {
                self.ui.focus = Focus::Results;
                self.set_status(format!("created · {ddl}"), false);
            }
            Err(e) => self.set_status(format!("save result: {e:#}"), true),
        }
        Ok(())
    }

    fn start_save_result(&mut self, as_temp: bool) {
        self.ui.save_materialize_temp = as_temp;
        self.ui.mode = Mode::SaveMaterialize;
        self.ui.import_csv_input.clear();
        self.set_status(
            if as_temp {
                "temp table name · Enter · Esc cancel"
            } else {
                "view name · Enter · Esc cancel"
            },
            false,
        );
    }

    async fn confirm_query_params(&mut self) -> Result<()> {
        let raw = self.ui.import_csv_input.trim().to_string();
        self.ui.import_csv_input.clear();
        self.ui.mode = Mode::Normal;
        let for_explain = self.ui.query_params_for_explain;
        self.ui.query_params_for_explain = false;
        if raw.is_empty() {
            self.set_status("query params cancelled", true);
            return Ok(());
        }
        let sql = match substitute_query_params(&self.db.sql, &raw) {
            Ok(s) => s,
            Err(e) => {
                self.set_status(format!("params: {e:#}"), true);
                return Ok(());
            }
        };
        let result = if for_explain {
            self.db.explain_sql_text(&sql).await
        } else {
            self.db.execute_sql_text(&sql).await
        };
        match result {
            Ok(()) => {
                self.ui.focus = Focus::Results;
                self.set_status(self.db.status.clone(), false);
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if for_explain {
                    self.set_status(format!("explain: {msg}"), true);
                } else {
                    self.db.last_result = Some(ide_db::QueryResult {
                        columns: vec!["error".into()],
                        rows: vec![vec![msg.clone()]],
                        affected: None,
                        elapsed_ms: 0,
                        truncated: false,
                        source_schema: None,
                        source_table: None,
                        primary_keys: Vec::new(),
                        original_rows: vec![vec![msg.clone()]],
                        cursor_row: 0,
                        cursor_col: 0,
                        explain: None,
                        chart: None,
                        page: 0,
                        has_more: false,
                    });
                    self.ui.focus = Focus::Results;
                    self.set_status(format!("query: {msg}"), true);
                }
            }
        }
        Ok(())
    }

    async fn confirm_import_csv(&mut self) -> Result<()> {
        let marker = self.ui.confirm_run_sql.take();
        let raw = self.ui.import_csv_input.trim().to_string();
        self.ui.import_csv_input.clear();
        self.ui.mode = Mode::Normal;
        if raw.is_empty() {
            return Ok(());
        }
        match marker.as_deref() {
            Some("__alter_add__") | Some("__alter_drop__") | Some("__alter_rename__") => {
                let kind = match marker.as_deref() {
                    Some("__alter_drop__") => AlterColumnKind::Drop,
                    Some("__alter_rename__") => AlterColumnKind::Rename,
                    _ => AlterColumnKind::Add,
                };
                match self.db.alter_sql_for_selection(kind, &raw) {
                    Ok((sql, label)) => {
                        self.ui.confirm_apply_pending = false;
                        self.ui.confirm_delete_source_id = None;
                        self.ui.confirm_run_sql = Some(sql);
                        self.ui.confirm_delete_label = label;
                        self.ui.mode = Mode::ConfirmDeleteSource;
                    }
                    Err(e) => self.set_status(format!("alter: {e:#}"), true),
                }
                return Ok(());
            }
            Some("__migrate_dry_run__") | Some("__migrate_apply__") => {
                let path = if raw.starts_with("~/") {
                    dirs::home_dir()
                        .map(|h| h.join(raw.trim_start_matches("~/")))
                        .unwrap_or_else(|| PathBuf::from(&raw))
                } else {
                    let p = PathBuf::from(&raw);
                    if p.is_absolute() {
                        p
                    } else {
                        self.workspace.root.join(p)
                    }
                };
                if marker.as_deref() == Some("__migrate_dry_run__") {
                    match self.db.dry_run_migration_file(&path) {
                        Ok(()) => {
                            self.ui.focus = Focus::Results;
                            self.set_status(self.db.status.clone(), false);
                        }
                        Err(e) => self.set_status(format!("migrate dry-run: {e:#}"), true),
                    }
                } else {
                    match self.db.apply_migration_file(&path).await {
                        Ok(()) => {
                            self.ui.focus = Focus::Results;
                            self.set_status(self.db.status.clone(), false);
                        }
                        Err(e) => self.set_status(format!("migrate apply: {e:#}"), true),
                    }
                }
                return Ok(());
            }
            Some("__paste_tsv__") => {
                match self.db.last_result.as_mut() {
                    Some(r) => match r.paste_tsv_at_cursor(&raw) {
                        Ok(n) => {
                            self.db.flush_active_tab();
                            self.ui.focus = Focus::Results;
                            self.set_status(
                                format!("pasted {n} row(s) · Ctrl-S apply mutations"),
                                false,
                            );
                        }
                        Err(e) => self.set_status(format!("paste: {e:#}"), true),
                    },
                    None => self.set_status("no result grid", true),
                }
                return Ok(());
            }
            _ => {}
        }
        let path = if raw.starts_with("~/") {
            dirs::home_dir()
                .map(|h| h.join(raw.trim_start_matches("~/")))
                .unwrap_or_else(|| PathBuf::from(&raw))
        } else {
            let p = PathBuf::from(&raw);
            if p.is_absolute() {
                p
            } else {
                self.workspace.root.join(p)
            }
        };
        if marker.as_deref() == Some("__import_conn_pack__") {
            match self.db.import_connection_pack(&path) {
                Ok(n) => self.set_status(
                    format!("imported {n} connection(s) from {}", path.display()),
                    false,
                ),
                Err(e) => self.set_status(format!("pack import: {e:#}"), true),
            }
            return Ok(());
        }
        let as_json = marker.as_deref() == Some("__import_json__")
            || path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if as_json {
            match self.db.import_json_file(&path).await {
                Ok(n) => self.set_status(
                    format!("imported {n} JSON rows from {}", path.display()),
                    false,
                ),
                Err(e) => self.set_status(format!("import json: {e:#}"), true),
            }
            return Ok(());
        }
        match self.db.import_csv_file(&path).await {
            Ok(n) => self.set_status(format!("imported {n} rows from {}", path.display()), false),
            Err(e) => self.set_status(format!("import: {e:#}"), true),
        }
        Ok(())
    }

    fn open_settings(&mut self) {
        self.ui.settings_draft = IdeConfig::config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_else(|| "# config path unavailable\n".into());
        self.ui.mode = Mode::Settings;
    }

    /// db-studio: Ctrl-1…4 map to studio panes (not Code/Sessions products).
    async fn goto_product_tab(&mut self, n: u8) -> Result<()> {
        self.ui.product = ProductMode::Database;
        if self.ui.db_surface != DbSurface::Studio {
            self.enter_db_studio();
        }
        match n {
            0 => self.focus_db_pane(1),
            1 => self.focus_db_pane(2),
            2 => self.focus_db_pane(3),
            3 => self.focus_db_pane(4),
            _ => self.set_status("DB Studio · 1–4 panes · a connect · Ctrl-⇧H home", false),
        }
        Ok(())
    }

    /// Forward a host chord into AstroNvim (Code chrome lives there when attached).
    async fn astro_cmd(&mut self, cmd: &str, ok_status: &str) -> Result<()> {
        if let Some(nvim) = self.nvim.as_mut() {
            self.ui.focus = Focus::Editor;
            self.ui.mode = Mode::Normal;
            self.ui.sidebar = Sidebar::Files;
            self.ui.show_sidebar = false;
            self.ui.show_outline = false;
            match nvim.command(cmd).await {
                Ok(()) => {
                    nvim.force_next_resize();
                    self.set_status(ok_status, false);
                }
                Err(e) => self.set_status(format!("Astro: {e}"), true),
            }
        } else {
            self.set_status("Neovim not attached — Astro chrome unavailable", true);
        }
        Ok(())
    }

    fn session_form_field_mut(&mut self) -> &mut String {
        let i = self.ui.session_form_field;
        let ssh = self.sessions.form_kind_ssh;
        match i {
            0 => &mut self.sessions.form_name,
            1 => &mut self.sessions.form_host,
            2 => &mut self.sessions.form_user,
            3 => &mut self.sessions.form_port,
            4 if ssh => &mut self.sessions.form_identity,
            4 => &mut self.sessions.form_gateway,
            _ => &mut self.sessions.form_jump,
        }
    }

    fn session_form_push_char(&mut self, c: char) {
        self.session_form_field_mut().push(c);
    }

    fn session_form_pop_char(&mut self) {
        self.session_form_field_mut().pop();
    }

    fn agent_form_field_mut(&mut self) -> &mut String {
        match self.ui.agent_form_field {
            0 => &mut self.agents.form_name,
            1 => &mut self.agents.form_kind,
            2 => &mut self.agents.form_binary,
            3 => &mut self.agents.form_args,
            _ => &mut self.agents.form_seed,
        }
    }

    fn agent_form_push_char(&mut self, c: char) {
        self.agent_form_field_mut().push(c);
    }

    fn agent_form_pop_char(&mut self) {
        self.agent_form_field_mut().pop();
    }

    fn db_agent_seed(&self) -> Option<String> {
        if self.ui.product != ProductMode::Database {
            return None;
        }
        let sid = self.db.active_source.as_ref()?;
        let src = self.db.sources.iter().find(|s| &s.id == sid)?;
        let schema = ""; // keep generic — explorer selection optional later
        Some(ide_agents::db_context_seed(
            src.driver.label(),
            &src.host,
            src.port,
            &src.database,
            schema,
        ))
    }

    async fn confirm_settings(&mut self) -> Result<()> {
        let draft = self.ui.settings_draft.clone();
        self.ui.mode = Mode::Normal;
        let Some(path) = IdeConfig::config_path() else {
            return Ok(());
        };
        std::fs::write(&path, draft)?;
        self.config = IdeConfig::load_from(&path)?;
        ide_ui::theme::set_mode_name(&self.config.theme.name);
        self.db.query_timeout_secs = self.config.database.query_timeout_secs;
        self.db.select_limit = self.config.database.select_limit;
        self.lsp = LspManager::new(self.workspace.root.clone(), self.config.clone());
        self.set_status("config saved and reloaded", false);
        Ok(())
    }

    /// Apply a built-in theme id, persist to config, and toast the label.
    fn apply_theme_id(&mut self, name: &str) {
        ide_ui::theme::set_mode_name(name);
        let id = ide_ui::theme::current_id();
        self.config.theme.name = id.to_string();
        self.theme_before_preview = None;
        if let Err(e) = self.config.save() {
            self.set_status(format!("theme · {id} (save failed: {e:#})"), true);
            return;
        }
        self.set_status(
            format!("theme · {} ({id})", ide_ui::theme::current_label()),
            false,
        );
    }

    /// Live-preview theme under the palette cursor (DataRig-style). Does not write config.
    fn preview_palette_theme(&mut self) {
        if self.ui.mode != Mode::Palette {
            return;
        }
        let id = self
            .ui
            .palette
            .selected_item()
            .map(|it| it.id.clone())
            .unwrap_or_default();
        if id == "theme_new_custom" || id == "theme_reload" {
            if let Some(base) = self.theme_before_preview.clone() {
                ide_ui::theme::set_mode_name(&base);
            }
            return;
        }
        if let Some(name) = id.strip_prefix("theme_") {
            if self.theme_before_preview.is_none() {
                let mut base = self.config.theme.name.clone();
                if base.trim().is_empty() {
                    base = ide_ui::theme::current_id().to_string();
                }
                self.theme_before_preview = Some(base);
            }
            ide_ui::theme::set_mode_name(name);
            return;
        }
        // Left theme rows — snap back to the theme we had when preview started.
        if let Some(base) = self.theme_before_preview.clone() {
            ide_ui::theme::set_mode_name(&base);
        }
    }

    /// Esc from palette: drop preview and restore committed theme.
    fn restore_theme_preview(&mut self) {
        if let Some(id) = self.theme_before_preview.take() {
            ide_ui::theme::set_mode_name(&id);
        }
    }

    /// Enter on a Results cell: open inspector on Row (Cell is Enter drill-in).
    fn open_cell_modal(&mut self) {
        if self.try_er_navigate() {
            return;
        }
        let Some(result) = self.db.last_result.as_ref() else {
            self.set_status("no result cell", true);
            return;
        };
        if result.rows.get(result.cursor_row).is_none() {
            return;
        }
        self.ui.focus = Focus::Results;
        self.ui.bottom = BottomPanel::Results;
        self.ui.cell_modal = true;
        self.ui.cell_modal_tab = 0; // Row
        self.ui.cell_modal_drill = false;
        self.ui.mode = Mode::Normal;
        self.set_status(
            "inspector · Row|JSON|Meta · Enter cell · Esc",
            false,
        );
    }

    /// Keep Results H-scroll sticky: only slide when the cursor nears a window edge.
    fn nudge_results_h_scroll(&mut self) {
        let Some(result) = self.db.last_result.as_ref() else {
            return;
        };
        let ncols = result.columns.len();
        if ncols == 0 {
            return;
        }
        let pinned = self.ui.results_pinned_cols.min(ncols.saturating_sub(1));
        // Match render's typical unpinned budget (~6–8); render still clamps to fit.
        let budget = 6usize.max(1);
        let max_start = ncols.saturating_sub(budget).max(pinned);
        let cursor = result.cursor_col;
        let mut start = self.ui.results_col_window.clamp(pinned, max_start);
        if cursor >= pinned {
            let margin = (budget / 4).max(1);
            if cursor < start + margin {
                start = cursor.saturating_sub(budget / 3);
            } else if cursor + margin >= start + budget {
                start = (cursor + 1 + margin).saturating_sub(budget);
            }
        }
        self.ui.results_col_window = start.clamp(pinned, max_start);
    }

    /// Start editing the selected cell inside the open modal.
    fn start_cell_modal_edit(&mut self) {
        let Some(result) = self.db.last_result.as_ref() else {
            return;
        };
        let Some(cell) = result
            .rows
            .get(result.cursor_row)
            .and_then(|r| r.get(result.cursor_col))
        else {
            return;
        };
        self.ui.cell_edit_input = cell.clone();
        self.ui.mode = Mode::EditCell;
        self.set_status("editing cell · Enter apply · Esc cancel", false);
    }

    fn start_edit_cell(&mut self) {
        if self.try_er_navigate() {
            return;
        }
        let Some(result) = self.db.last_result.as_ref() else {
            self.set_status("no result cell", true);
            return;
        };
        let Some(cell) = result
            .rows
            .get(result.cursor_row)
            .and_then(|r| r.get(result.cursor_col))
        else {
            return;
        };
        self.ui.focus = Focus::Results;
        self.ui.bottom = BottomPanel::Results;
        self.ui.cell_edit_input = cell.clone();
        self.ui.mode = Mode::EditCell;
        self.set_status("editing cell · Enter apply · Esc cancel", false);
    }

    /// Enter on ER sketch line `a.b --> x.y.z` jumps Explorer to a table.
    fn try_er_navigate(&mut self) -> bool {
        let Some(result) = self.db.last_result.as_ref() else {
            return false;
        };
        if result.source_table.as_deref() != Some("er_sketch") {
            return false;
        }
        let Some(line) = result
            .rows
            .get(result.cursor_row)
            .and_then(|r| r.first())
            .cloned()
        else {
            return false;
        };
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() || line.starts_with('(') {
            return false;
        }
        let target = line
            .split_once("-->")
            .map(|(_, r)| r.trim())
            .unwrap_or(line);
        let bits: Vec<&str> = target.split('.').collect();
        let (schema, table) = match bits.as_slice() {
            [schema, table, _col, ..] => (*schema, *table),
            [table, _col] => ("", *table),
            [table] => ("", *table),
            _ => return false,
        };
        if self.db.jump_to_table(schema, table) || self.db.jump_to_table("", table) {
            self.ui.focus = Focus::Database;
            let label = if schema.is_empty() {
                table.to_string()
            } else {
                format!("{schema}.{table}")
            };
            self.set_status(format!("ER jump · {label}"), false);
        } else {
            self.set_status(format!("ER: table `{table}` not in explorer"), true);
        }
        true
    }

    fn confirm_edit_cell(&mut self) {
        let value = self.ui.cell_edit_input.clone();
        self.ui.mode = Mode::Normal;
        self.ui.cell_edit_input.clear();
        self.ui.focus = Focus::Results;
        if let Some(result) = self.db.last_result.as_mut() {
            let row = result.cursor_row;
            let col = result.cursor_col;
            if let Some(prev) = result.set_cell(value) {
                self.db.cell_undo.push((row, col, prev));
            }
            self.db.flush_active_tab();
            self.set_status("cell dirty · Ctrl-S apply · u undo cell", false);
        }
    }

    async fn refresh_debug_locals(&mut self) {
        let Some(dap) = self.dap.as_ref() else {
            self.set_status("no debug session", true);
            return;
        };
        match dap.stack_trace(self.debug_thread_id).await {
            Ok(frames) => {
                self.ui.stack_frames = frames;
                if self.ui.debug_frame_selected >= self.ui.stack_frames.len() {
                    self.ui.debug_frame_selected = 0;
                }
            }
            Err(e) => self.set_status(format!("stack: {e}"), true),
        }
        self.refresh_debug_variables().await;
    }

    async fn refresh_debug_variables(&mut self) {
        let Some(dap) = self.dap.as_ref() else {
            return;
        };
        let Some(frame) = self.ui.stack_frames.get(self.ui.debug_frame_selected) else {
            self.ui.variables.clear();
            return;
        };
        match dap.variables_for_frame(frame.id).await {
            Ok(vars) => self.ui.variables = vars,
            Err(e) => self.set_status(format!("variables: {e}"), true),
        }
    }

    async fn run_plugin_command(&mut self, command_id: &str) -> Result<()> {
        let file = self
            .active_buffer()
            .and_then(|b| b.path.clone())
            .filter(|p| p.is_absolute())
            .or_else(|| {
                self.active_buffer()
                    .and_then(|b| b.path.as_ref().map(|p| self.workspace.root.join(p)))
            });
        let line = self
            .active_buffer()
            .map(|b| b.cursor.row + 1)
            .unwrap_or(1);
        match self.plugins.run(
            command_id,
            &self.workspace.root,
            file.as_deref(),
            line,
        ) {
            Ok(out) => {
                let text = out.trim().lines().next().unwrap_or("ok").to_string();
                self.set_status(format!("plugin: {text}"), false);
            }
            Err(e) => self.set_status(format!("plugin: {e:#}"), true),
        }
        Ok(())
    }

    fn start_add_connection(&mut self) {
        self.ui.mode = Mode::DbConnect;
        self.ui.db_connect_input.clear();
        self.ui.db_connect_edit_id = None;
        self.ui.db_connect_form = DbConnectForm::fresh();
        self.set_status("new connection — ←→ choose · Ctrl-T test · Enter save", false);
    }

    fn start_edit_connection(&mut self, source_id: &str) {
        let Some(src) = self
            .db
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .cloned()
        else {
            self.set_status("connection not found", true);
            return;
        };
        self.ui.mode = Mode::DbConnect;
        self.ui.db_connect_input.clear();
        self.ui.db_connect_edit_id = Some(source_id.to_string());
        self.ui.db_connect_form = DbConnectForm::from_source(&src);
        self.set_status(
            "edit connection — Tab · Enter save · Esc",
            false,
        );
    }

    fn open_create_object_modal(&mut self, id: &str) {
        let Some(kind) = CreateObjectKind::from_id(id) else {
            self.set_status(format!("unknown create kind: {id}"), true);
            return;
        };
        let schema = self
            .db
            .active_schema
            .clone()
            .or_else(|| {
                self.db
                    .flat
                    .get(self.db.selected)
                    .and_then(|e| e.schema.clone())
            })
            .unwrap_or_else(|| "public".into());
        let (default_owner, driver) = self
            .db
            .active_source
            .as_ref()
            .and_then(|id| self.db.sources.iter().find(|s| s.id == *id))
            .map(|s| (s.user.clone(), s.driver))
            .unwrap_or_default();
        // Prefill schema.table for Index from Explorer table selection.
        let mut form =
            CreateObjectForm::fresh_with_owner(kind, &schema, &default_owner, driver);
        if kind == CreateObjectKind::Index {
            if let Some(e) = self.db.flat.get(self.db.selected) {
                if matches!(e.kind, DbNodeKind::Table | DbNodeKind::View) {
                    let sch = e.schema.as_deref().unwrap_or("public");
                    let tbl = e.table.as_deref().unwrap_or(e.label.as_str());
                    form.schema = format!("{sch}.{tbl}");
                }
            }
        }
        if matches!(
            kind,
            CreateObjectKind::Table
                | CreateObjectKind::View
                | CreateObjectKind::MaterializedView
                | CreateObjectKind::Sequence
        ) {
            if let Some(e) = self.db.flat.get(self.db.selected) {
                if let Some(sch) = e.schema.as_deref() {
                    form.schema = sch.to_string();
                } else if matches!(e.kind, DbNodeKind::Schema) {
                    form.schema = e.label.clone();
                }
            }
        }
        self.ui.create_object_form = form;
        self.ui.mode = Mode::CreateObject;
        self.set_status(
            format!("{} — Tab fields · Enter create · Esc", kind.title()),
            false,
        );
    }

    async fn confirm_create_object(&mut self) -> Result<()> {
        let form = self.ui.create_object_form.clone();
        let sql = match form.to_sql() {
            Ok(s) => s,
            Err(e) => {
                self.ui.create_object_form.feedback = Some((e.clone(), true));
                self.set_status(format!("create: {e}"), true);
                return Ok(());
            }
        };
        self.ui.create_object_form.feedback = Some(("creating…".into(), false));
        match self.db.execute_sql_text(&sql).await {
            Ok(()) => {
                let kind = form.kind.title();
                self.ui.mode = Mode::Normal;
                self.ui.focus = Focus::Database;
                // Reload catalogs so the new object appears in Explorer.
                if let Some(sid) = self.db.active_source.clone() {
                    let _ = self.db.ensure_connected(&sid, None).await;
                    self.db.rebuild_flat();
                }
                self.set_ok(format!("{kind} · created · Explorer refreshed"));
            }
            Err(e) => {
                let text = format!("{e:#}");
                let shown = Self::human_err(&text);
                self.ui.create_object_form.feedback = Some((shown.clone(), true));
                self.set_status(format!("create failed: {text}"), true);
            }
        }
        Ok(())
    }

    /// `p` / `e` — DataGrip-like properties for the Explorer selection.
    async fn open_object_properties(&mut self) {
        let entry = if self.ui.focus == Focus::Database {
            self.db.flat.get(self.db.selected).cloned()
        } else {
            None
        };
        let Some(entry) = entry else {
            if let Some(id) = self.db.active_source.clone() {
                self.start_edit_connection(&id);
            } else {
                self.set_status("select a connection in Explorer · p properties", true);
            }
            return;
        };
        match entry.kind {
            DbNodeKind::Source => {
                self.start_edit_connection(&entry.source_id);
            }
            DbNodeKind::Database | DbNodeKind::Schema | DbNodeKind::Table | DbNodeKind::View => {
                if let Err(e) = self.db.refresh_structure_for_selection().await {
                    self.set_status(format!("properties: {e:#}"), true);
                } else {
                    self.ui.focus = Focus::Structure;
                    self.set_status(
                        "properties · Structure · n New to create · Esc Explorer",
                        false,
                    );
                }
            }
            _ => {
                if let Err(e) = self.db.refresh_structure_for_selection().await {
                    self.set_status(format!("properties: {e:#}"), true);
                } else {
                    self.ui.focus = Focus::Structure;
                    self.set_status("properties · n New create DB/user/table", false);
                }
            }
        }
    }

    fn data_source_from_form(form: &DbConnectForm, edit_id: Option<&str>) -> Result<DataSource> {
        // Role=admin always wins over Read-only — otherwise CREATE looks broken.
        let read_only = form.read_only && !form.role.allows_admin();
        let name = if form.name.trim().is_empty() {
            match form.driver {
                DbDriver::Sqlite => {
                    let p = form.file.trim();
                    Path::new(p)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("sqlite")
                        .to_string()
                }
                _ => {
                    if form.database.trim().is_empty() {
                        form.host.clone()
                    } else {
                        form.database.clone()
                    }
                }
            }
        } else {
            form.name.trim().to_string()
        };

        match form.driver {
            DbDriver::Sqlite => {
                let file = form.file.trim();
                if file.is_empty() {
                    anyhow::bail!("File is required for SQLite");
                }
                let path = if file == ":memory:" {
                    ":memory:".into()
                } else {
                    expand_user_path(file).display().to_string()
                };
                Ok(DataSource {
                    id: edit_id
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    name,
                    driver: form.driver,
                    host: String::new(),
                    port: 0,
                    user: String::new(),
                    password: String::new(),
                    database: String::new(),
                    file: path,
                    ssh_host: String::new(),
                    ssh_user: String::new(),
                    ssh_port: 22,
                    ssh_key: String::new(),
                    ssl_mode: form.ssl_mode.trim().to_string(),
                    ssl_trust_cert: form.ssl_trust_cert,
                    ssl_ca: form.ssl_ca.trim().into(),
                    read_only,
                    role: form.role,
                    group: form.group.trim().into(),
                    accent: form.accent.trim().into(),
                    pinned: false,
                })
            }
            DbDriver::Duckdb => {
                let file = form.file.trim();
                if file.is_empty() {
                    anyhow::bail!("File is required for DuckDB");
                }
                let path = if file == ":memory:" {
                    ":memory:".into()
                } else {
                    expand_user_path(file).display().to_string()
                };
                Ok(DataSource {
                    id: edit_id
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    name,
                    driver: DbDriver::Duckdb,
                    host: String::new(),
                    port: 0,
                    user: String::new(),
                    password: String::new(),
                    database: String::new(),
                    file: path,
                    ssh_host: String::new(),
                    ssh_user: String::new(),
                    ssh_port: 22,
                    ssh_key: String::new(),
                    ssl_mode: form.ssl_mode.trim().to_string(),
                    ssl_trust_cert: form.ssl_trust_cert,
                    ssl_ca: form.ssl_ca.trim().into(),
                    read_only,
                    role: form.role,
                    group: form.group.trim().into(),
                    accent: form.accent.trim().into(),
                    pinned: false,
                })
            }
            DbDriver::Redis | DbDriver::Mongo | DbDriver::Snowflake | DbDriver::Bigquery | DbDriver::Elasticsearch | DbDriver::Trino => {
                if form.host.trim().is_empty() {
                    anyhow::bail!("Host is required");
                }
                let default_port = match form.driver {
                    DbDriver::Redis => 6379,
                    DbDriver::Mongo => 27017,
                    DbDriver::Elasticsearch => 9200,
                    DbDriver::Trino => 8080,
                    _ => 443,
                };
                let port: u16 = form.port.trim().parse().unwrap_or(default_port);
                Ok(DataSource {
                    id: edit_id
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    name,
                    driver: form.driver,
                    host: form.host.trim().into(),
                    port,
                    user: form.user.trim().into(),
                    password: form.password.clone(),
                    database: form.database.trim().into(),
                    file: String::new(),
                    ssh_host: form.ssh_host.trim().into(),
                    ssh_user: form.ssh_user.trim().into(),
                    ssh_port: form.ssh_port.trim().parse().unwrap_or(22),
                    ssh_key: form.ssh_key.trim().into(),
                    ssl_mode: form.ssl_mode.trim().to_string(),
                    ssl_trust_cert: form.ssl_trust_cert,
                    ssl_ca: form.ssl_ca.trim().into(),
                    read_only,
                    role: form.role,
                    group: form.group.trim().into(),
                    accent: form.accent.trim().into(),
                    pinned: false,
                })
            }
            DbDriver::Postgres | DbDriver::Mysql | DbDriver::Mssql | DbDriver::Clickhouse => {
                if form.host.trim().is_empty() {
                    anyhow::bail!("Host is required");
                }
                if form.database.trim().is_empty() {
                    anyhow::bail!("Database is required");
                }
                let default_port = match form.driver {
                    DbDriver::Mysql => 3306,
                    DbDriver::Mssql => 1433,
                    DbDriver::Clickhouse => 8123,
                    _ => 5432,
                };
                let port: u16 = form.port.trim().parse().unwrap_or(default_port);
                let ssh_port: u16 = form.ssh_port.trim().parse().unwrap_or(22);
                Ok(DataSource {
                    id: edit_id
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    name,
                    driver: form.driver,
                    host: form.host.trim().into(),
                    port,
                    user: form.user.trim().into(),
                    password: form.password.clone(),
                    database: form.database.trim().into(),
                    file: String::new(),
                    ssh_host: form.ssh_host.trim().into(),
                    ssh_user: form.ssh_user.trim().into(),
                    ssh_port,
                    ssh_key: form.ssh_key.trim().into(),
                    ssl_mode: form.ssl_mode.trim().to_string(),
                    ssl_trust_cert: form.ssl_trust_cert,
                    ssl_ca: form.ssl_ca.trim().into(),
                    read_only,
                    role: form.role,
                    group: form.group.trim().into(),
                    accent: form.accent.trim().into(),
                    pinned: false,
                })
            }
        }
    }

    async fn confirm_db_connect(&mut self) -> Result<()> {
        let form = self.ui.db_connect_form.clone();
        let edit_id = self.ui.db_connect_edit_id.clone();
        let keep_pinned = edit_id
            .as_ref()
            .and_then(|id| self.db.sources.iter().find(|s| s.id == *id).map(|s| s.pinned))
            .unwrap_or(false);
        let mut src = match Self::data_source_from_form(&form, edit_id.as_deref()) {
            Ok(s) => s,
            Err(e) => {
                self.set_status(format!("connect: {e:#}"), true);
                return Ok(());
            }
        };
        src.pinned = keep_pinned;
        self.ui.mode = Mode::Normal;
        self.ui.db_connect_input.clear();
        self.ui.db_connect_form = DbConnectForm::default();
        self.ui.db_connect_edit_id = None;
        let name = src.name.clone();
        let id = src.id.clone();
        if edit_id.is_some() {
            if let Err(e) = self.db.update_source(src) {
                self.set_status(format!("update: {e:#}"), true);
                return Ok(());
            }
            self.set_status(format!("updated · {name}"), false);
        } else {
            self.db.add_source(src);
            self.set_status(format!("added · {name}"), false);
        }
        self.db.active_source = Some(id);
        self.db.rebuild_flat();
        if self.ui.product == ProductMode::Database {
            self.enter_db_studio();
        }
        Ok(())
    }

    async fn confirm_find(&mut self) -> Result<()> {
        let q = self.ui.find_query.trim().to_string();
        self.ui.mode = Mode::Normal;
        if self.ui.focus == Focus::Results {
            self.db.results_filter = q.clone();
            if q.is_empty() {
                self.set_status("results filter cleared", false);
            } else {
                self.set_status(format!("results filter · {q}"), false);
            }
            return Ok(());
        }
        if q.is_empty() {
            return Ok(());
        }
        if let Some(buf) = self.active_buffer_mut() {
            if buf.find_next(&q) {
                buf.ensure_cursor_visible(30);
                self.set_status(format!("find: {q}"), false);
            } else {
                self.set_status("no match", true);
            }
        }
        Ok(())
    }

    async fn confirm_replace(&mut self) -> Result<()> {
        let replacement = self.ui.replace_query.clone();
        self.ui.mode = Mode::Normal;
        self.ui.replace_query.clear();
        let q = self.ui.find_query.trim().to_string();
        if q.is_empty() {
            return Ok(());
        }
        if let Some(buf) = self.active_buffer_mut() {
            if buf.replace_one(&q, &replacement) {
                self.sync_lsp_change().await;
                self.set_status("replaced one", false);
            } else {
                self.set_status("no match to replace", true);
            }
        }
        Ok(())
    }

    async fn confirm_git_commit(&mut self) -> Result<()> {
        let msg = self.ui.commit_input.trim().to_string();
        self.ui.mode = Mode::Normal;
        self.ui.commit_input.clear();
        if msg.is_empty() {
            return Ok(());
        }
        if let Some(git) = self.git.as_mut() {
            match git.commit(&msg).await {
                Ok(()) => self.set_status("committed", false),
                Err(e) => self.set_status(format!("commit: {e}"), true),
            }
        }
        Ok(())
    }

    fn active_buf_index(&self) -> usize {
        if self.ui.split.enabled && self.ui.split.focus_secondary {
            self.ui
                .split
                .secondary
                .unwrap_or_else(|| self.buffers.active_index())
        } else {
            self.buffers.active_index()
        }
    }

    fn active_buffer(&self) -> Option<&Buffer> {
        self.buffers.get(self.active_buf_index())
    }

    fn active_buffer_mut(&mut self) -> Option<&mut Buffer> {
        let idx = self.active_buf_index();
        self.buffers.get_mut(idx)
    }

    fn diagnostics(&self) -> Vec<DiagnosticItem> {
        self.lsp.all_diagnostics()
    }

    async fn ensure_rust_lsp(&mut self) {
        if let Err(e) = self.lsp.ensure_mut_for_language("rust").await {
            warn!("rust LSP: {e:#}");
        } else if self.lsp.client_for("rust").is_some() {
            info!("rust-analyzer ready");
            self.set_status("rust-analyzer ready", false);
        }
    }

    fn update_breadcrumbs(&mut self) {
        let Some(buf) = self.active_buffer() else {
            self.ui.breadcrumbs.clear();
            return;
        };
        let rel = buf
            .path
            .as_ref()
            .and_then(|p| p.strip_prefix(&self.workspace.root).ok())
            .map(|p| p.display().to_string())
            .or_else(|| buf.path.as_ref().map(|p| p.display().to_string()))
            .unwrap_or_else(|| "untitled".into());
        let sym = self
            .ui
            .outline
            .iter()
            .filter(|s| s.line as usize <= buf.cursor.row)
            .next_back()
            .map(|s| s.name.trim().to_string());
        self.ui.breadcrumbs = match sym {
            Some(s) if !s.is_empty() => format!("{rel} › {s}"),
            _ => format!("{rel} :{}:{}", buf.cursor.row + 1, buf.cursor.col + 1),
        };
    }

    async fn refresh_lsp_extras(&mut self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.path.clone() else {
            return;
        };
        let lang = buf.language.lsp_id().to_string();
        let end_line = buf.line_count().saturating_sub(1) as u32;
        let Some(client) = self.lsp.ensure_mut_for_language(&lang).await.ok().flatten() else {
            self.ui.outline.clear();
            self.ui.inlays.clear();
            self.update_breadcrumbs();
            return;
        };
        self.ui.outline = client.document_symbols(&path).await.unwrap_or_default();
        self.ui.outline_selected = 0;
        if self.config.editor.inlay_hints && self.ui.show_inlays {
            self.ui.inlays = client.inlay_hints(&path, end_line).await.unwrap_or_default();
        } else {
            self.ui.inlays.clear();
        }
        self.update_breadcrumbs();
    }

    async fn sync_lsp_change(&mut self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.path.clone() else {
            return;
        };
        let lang = buf.language.lsp_id().to_string();
        let text = buf.text();
        let version = buf.version;
        if let Ok(Some(client)) = self.lsp.ensure_mut_for_language(&lang).await {
            if let Err(e) = client.did_change(&path, &text, version).await {
                warn!("did_change: {e:#}");
            }
        }
    }

    async fn open_path(&mut self, path: PathBuf) -> Result<()> {
        // Prefer Neovim whenever the embed is alive (Home / tree / palette).
        if self.nvim.is_some() && self.ui.product == ProductMode::Code {
            self.ui.code_surface = CodeSurface::Editor;
            self.last_code_surface = CodeSurface::Editor;
            self.ui.focus = Focus::Editor;
            self.ui.mode = Mode::Normal;
            self.pending_nvim_blank = false;
            if let Some(nvim) = self.nvim.as_ref() {
                nvim.open_file(&path).await?;
            }
            // Refresh buffer tabs on next tick.
            self.nvim_tab_tick = 29;
            self.config.remember_file(&path);
            self.set_status(format!("nvim edit {}", path.display()), false);
            return Ok(());
        }

        let buf = Buffer::from_path(&path)?;
        let text = buf.text();
        let lang = buf.language.lsp_id().to_string();
        self.buffers.open(buf);
        self.ui.focus = Focus::Editor;
        self.ui.mode = Mode::Normal;
        self.ui.hover = None;
        self.ui.show_completions = false;

        if let Ok(Some(client)) = self.lsp.ensure_mut_for_language(&lang).await {
            if let Err(e) = client.did_open(&path, &text, &lang).await {
                warn!("did_open: {e:#}");
            }
        }
        self.config.remember_file(&path);
        self.refresh_lsp_extras().await;
        Ok(())
    }

    async fn open_home_slot(&mut self, slot: u8) -> Result<()> {
        if slot == 0 || slot > 9 {
            return Ok(());
        }
        let current = self.workspace.root.display().to_string();
        let slots = home_recent_slots(
            &self.config.recent_projects,
            &self.config.recent_files,
            &current,
        );
        let Some(item) = slots.get((slot as usize).saturating_sub(1)) else {
            self.set_status(format!("no recent item [{slot}]"), true);
            return Ok(());
        };
        match item {
            HomeRecentSlot::Project(p) => self.switch_workspace(PathBuf::from(p)).await?,
            HomeRecentSlot::File(p) => {
                let path = PathBuf::from(p);
                if !path.is_file() {
                    self.set_status(format!("missing file: {}", path.display()), true);
                    return Ok(());
                }
                self.ui.code_surface = CodeSurface::Editor;
                if let Some(nvim) = self.nvim.as_ref() {
                    let _ = nvim.open_file(&path).await;
                    self.pending_nvim_blank = false;
                    self.config.remember_file(&path);
                    self.set_status(format!("opened {}", path.display()), false);
                } else {
                    self.open_path(path).await?;
                }
            }
        }
        Ok(())
    }

    async fn format_active(&mut self) -> Result<()> {
        let Some(buf) = self.active_buffer() else {
            self.set_status("no buffer", true);
            return Ok(());
        };
        let path = buf.path.clone();
        let lang = buf.language.lsp_id().to_string();
        let Some(path) = path else {
            self.set_status("no path", true);
            return Ok(());
        };
        let Some(client) = self.lsp.ensure_mut_for_language(&lang).await? else {
            self.set_status("no LSP for this language", true);
            return Ok(());
        };
        match client.formatting(&path).await {
            Ok(edits) if edits.is_empty() => self.set_status("nothing to format", false),
            Ok(edits) => {
                self.apply_text_edits_to_path(&path, &edits)?;
                self.sync_lsp_change().await;
                self.refresh_lsp_extras().await;
                self.set_status("formatted", false);
            }
            Err(e) => self.set_status(format!("format: {e}"), true),
        }
        Ok(())
    }

    async fn save_active(&mut self) -> Result<()> {
        if self.config.editor.format_on_save {
            let _ = self.format_active().await;
        }
        let Some(buf) = self.buffers.active_mut() else {
            self.set_status("no buffer", true);
            return Ok(());
        };
        let path = buf.path.clone();
        let text = buf.text();
        let version = buf.version;
        let lang = buf.language.lsp_id().to_string();
        buf.save()?;
        self.set_status("saved", false);
        if let Some(path) = path {
            if let Some(client) = self.lsp.client_for(&lang) {
                let _ = client.did_save(&path, &text).await;
                let _ = client.did_change(&path, &text, version).await;
            }
        }
        Ok(())
    }

    fn accept_db_completion(&mut self) {
        if !self.ui.show_completions || self.ui.completions.is_empty() {
            return;
        }
        let item = self.ui.completions[self.ui.completion_idx].clone();
        let row = self.ui.db_cursor.row;
        let col = self.ui.db_cursor.col;
        let start = item
            .replace_start_col
            .map(|c| c as usize)
            .unwrap_or(col);
        let end = item.replace_end_col.map(|c| c as usize).unwrap_or(col);
        let mut lines: Vec<String> = if self.db.sql.is_empty() {
            vec![String::new()]
        } else {
            self.db.sql.lines().map(|l| l.to_string()).collect()
        };
        while lines.len() <= row {
            lines.push(String::new());
        }
        let line = lines[row].clone();
        let prefix: String = line.chars().take(start).collect();
        let suffix: String = line.chars().skip(end).collect();
        lines[row] = format!("{prefix}{}{suffix}", item.insert_text);
        self.db.sql = lines.join("\n");
        self.ui.db_cursor.col = start + item.insert_text.chars().count();
        self.ui.show_completions = false;
        self.ui.completions.clear();
    }

    fn accept_completion(&mut self) {
        if !self.ui.show_completions || self.ui.completions.is_empty() {
            return;
        }
        if self.editing_db_sql() {
            self.accept_db_completion();
            return;
        }
        let item = self.ui.completions[self.ui.completion_idx].clone();
        if let Some(buf) = self.buffers.active_mut() {
            let row = buf.cursor.row;
            let start = item
                .replace_start_col
                .map(|c| c as usize)
                .unwrap_or(buf.cursor.col);
            let end = item
                .replace_end_col
                .map(|c| c as usize)
                .unwrap_or(buf.cursor.col);
            buf.replace_line_range(row, start, end, &item.insert_text);
        }
        self.ui.show_completions = false;
        self.ui.completions.clear();
    }

    async fn request_db_completion(&mut self) {
        if let Err(e) = self.db.refresh_schema_cache().await {
            self.set_status(format!("schema cache: {e:#}"), true);
            return;
        }
        let _ = self.db.refresh_structure_from_console_query().await;
        let row = self.ui.db_cursor.row;
        let col = self.ui.db_cursor.col;
        let line = self.db_line(row);
        let rel = ide_db::expecting_relation(&line, col);
        let items: Vec<ide_lsp::CompletionItemView> = self
            .db
            .sql_completions(&line, col)
            .into_iter()
            .map(|c| ide_lsp::CompletionItemView {
                label: c.label,
                detail: c.detail,
                insert_text: c.insert_text,
                kind: None,
                replace_start_col: Some(c.replace_start_col as u32),
                replace_end_col: Some(col as u32),
                filter_text: None,
            })
            .collect();
        let n = items.len();
        self.ui.completions = items;
        self.ui.completion_idx = 0;
        self.ui.show_completions = n > 0;
        if n == 0 {
            let hint = if rel {
                "no tables yet · Enter on DataLake in Explorer to connect/expand"
            } else {
                "no SQL completions"
            };
            self.set_status(hint, false);
        } else {
            self.set_status(format!("{n} completions · Tab accept · Ctrl-n/p"), false);
        }
    }

    async fn request_completion(&mut self) {
        if self.editing_db_sql() {
            self.request_db_completion().await;
            return;
        }
        let (path, line, col, lang) = {
            let Some(buf) = self.active_buffer() else {
                return;
            };
            let Some(path) = buf.path.clone() else {
                self.set_status("no path", true);
                return;
            };
            (
                path,
                buf.cursor.row as u32,
                buf.cursor.col as u32,
                buf.language.lsp_id().to_string(),
            )
        };
        let Some(client) = self.lsp.ensure_mut_for_language(&lang).await.ok().flatten() else {
            self.set_status("no LSP", true);
            return;
        };
        match client.completion(&path, line, col).await {
            Ok(items) => {
                self.ui.completions = items;
                self.ui.completion_idx = 0;
                self.ui.show_completions = !self.ui.completions.is_empty();
                if self.ui.completions.is_empty() {
                    self.set_status("no completions", false);
                }
            }
            Err(e) => self.set_status(format!("completion: {e}"), true),
        }
    }

    async fn goto_definition(&mut self) {
        let (path, line, col, lang) = {
            let Some(buf) = self.active_buffer() else {
                return;
            };
            let Some(path) = buf.path.clone() else {
                return;
            };
            (
                path,
                buf.cursor.row as u32,
                buf.cursor.col as u32,
                buf.language.lsp_id().to_string(),
            )
        };
        let Some(client) = self.lsp.ensure_mut_for_language(&lang).await.ok().flatten() else {
            return;
        };
        match client.goto_definition(&path, line, col).await {
            Ok(Some((target, tline, tcol))) => {
                if let Err(e) = self.open_path(target).await {
                    self.set_status(format!("{e}"), true);
                    return;
                }
                if let Some(buf) = self.buffers.active_mut() {
                    buf.cursor.row = tline as usize;
                    buf.cursor.col = tcol as usize;
                    buf.clamp_cursor();
                    buf.ensure_cursor_visible(30);
                }
            }
            Ok(None) => self.set_status("no definition", false),
            Err(e) => self.set_status(format!("goto: {e}"), true),
        }
    }

    async fn hover(&mut self) {
        let (path, line, col, lang) = {
            let Some(buf) = self.active_buffer() else {
                return;
            };
            let Some(path) = buf.path.clone() else {
                return;
            };
            (
                path,
                buf.cursor.row as u32,
                buf.cursor.col as u32,
                buf.language.lsp_id().to_string(),
            )
        };
        let Some(client) = self.lsp.ensure_mut_for_language(&lang).await.ok().flatten() else {
            return;
        };
        match client.hover(&path, line, col).await {
            Ok(Some(text)) => self.ui.hover = Some(text),
            Ok(None) => {
                self.ui.hover = None;
                self.set_status("no hover", false);
            }
            Err(e) => self.set_status(format!("hover: {e}"), true),
        }
    }

    async fn start_rename(&mut self) {
        let word = self
            .active_buffer()
            .map(|b| b.word_at_cursor())
            .unwrap_or_default();
        self.ui.rename_input = word;
        self.ui.mode = Mode::Rename;
        self.ui.focus = Focus::Editor;
    }

    async fn confirm_rename(&mut self) -> Result<()> {
        let new_name = self.ui.rename_input.trim().to_string();
        self.ui.mode = Mode::Normal;
        self.ui.rename_input.clear();
        if new_name.is_empty() {
            return Ok(());
        }
        let (path, line, col, lang) = {
            let Some(buf) = self.active_buffer() else {
                return Ok(());
            };
            let Some(path) = buf.path.clone() else {
                return Ok(());
            };
            (
                path,
                buf.cursor.row as u32,
                buf.cursor.col as u32,
                buf.language.lsp_id().to_string(),
            )
        };
        let Some(client) = self.lsp.ensure_mut_for_language(&lang).await? else {
            self.set_status("no LSP", true);
            return Ok(());
        };
        match client.rename(&path, line, col, &new_name).await {
            Ok(Some(edit)) => {
                self.apply_workspace_edit(&edit)?;
                self.sync_lsp_change().await;
                self.refresh_lsp_extras().await;
                self.set_status("renamed", false);
            }
            Ok(None) => self.set_status("rename rejected", true),
            Err(e) => self.set_status(format!("rename: {e}"), true),
        }
        Ok(())
    }

    async fn fetch_code_actions(&mut self) -> Result<()> {
        let (path, line, col, end_line, end_col, lang, diags) = {
            let Some(buf) = self.active_buffer() else {
                return Ok(());
            };
            let Some(path) = buf.path.clone() else {
                return Ok(());
            };
            let line = buf.cursor.row as u32;
            let col = buf.cursor.col as u32;
            let lsp_diags: Vec<Diagnostic> = self
                .lsp
                .client_for(buf.language.lsp_id())
                .map(|c| c.diagnostics_for(&path))
                .unwrap_or_default()
                .into_iter()
                .map(|d| Diagnostic {
                    range: lsp_types::Range {
                        start: lsp_types::Position {
                            line: d.line,
                            character: d.character,
                        },
                        end: lsp_types::Position {
                            line: d.end_line,
                            character: d.end_character,
                        },
                    },
                    severity: Some(d.severity),
                    message: d.message,
                    ..Default::default()
                })
                .collect();
            (
                path,
                line,
                col,
                line,
                col,
                buf.language.lsp_id().to_string(),
                lsp_diags,
            )
        };
        let Some(client) = self.lsp.ensure_mut_for_language(&lang).await? else {
            self.set_status("no LSP", true);
            return Ok(());
        };
        let actions = client
            .code_actions(&path, line, col, end_line, end_col, diags)
            .await?;
        if actions.is_empty() {
            self.set_status("no code actions", false);
            return Ok(());
        }
        let mut palette_pairs = Vec::new();
        self.ui.pending_code_actions.clear();
        for (i, action) in actions.iter().enumerate() {
            match action {
                CodeActionOrCommand::CodeAction(ca) => {
                    let label = ca.title.clone();
                    let id = if let Some(edit) = &ca.edit {
                        serde_json::to_string(edit).unwrap_or_else(|_| format!("edit-{i}"))
                    } else {
                        format!("noop-{i}")
                    };
                    palette_pairs.push((label.clone(), id.clone()));
                    self.ui.pending_code_actions.push((label, id));
                }
                CodeActionOrCommand::Command(cmd) => {
                    palette_pairs.push((cmd.title.clone(), format!("cmd:{}", cmd.command)));
                }
            }
        }
        self.ui.mode = Mode::Palette;
        self.ui.palette = Palette::open_actions(palette_pairs);
        Ok(())
    }

    async fn apply_quick_fix(&mut self) -> Result<()> {
        if self.ui.focus == Focus::Diagnostics {
            let diags = self.diagnostics();
            if let Some(d) = diags.get(self.ui.diag_selected) {
                if let Err(e) = self.open_path(d.path.clone()).await {
                    self.set_status(e.to_string(), true);
                    return Ok(());
                }
                if let Some(buf) = self.buffers.active_mut() {
                    buf.cursor.row = d.line as usize;
                    buf.cursor.col = d.character as usize;
                    buf.clamp_cursor();
                }
            }
        }
        self.fetch_code_actions().await
    }

    fn apply_workspace_edit(&mut self, edit: &WorkspaceEdit) -> Result<()> {
        if let Some(changes) = &edit.changes {
            for (uri, edits) in changes {
                if let Some(path) = uri_to_path(uri) {
                    self.apply_text_edits_to_path(&path, edits)?;
                }
            }
        }
        if let Some(doc_changes) = &edit.document_changes {
            match doc_changes {
                DocumentChanges::Edits(edits) => {
                    for tde in edits {
                        if let Some(path) = uri_to_path(&tde.text_document.uri) {
                            let flat = flatten_document_edits(&tde.edits);
                            self.apply_text_edits_to_path(&path, &flat)?;
                        }
                    }
                }
                DocumentChanges::Operations(ops) => {
                    for op in ops {
                        if let DocumentChangeOperation::Edit(tde) = op {
                            if let Some(path) = uri_to_path(&tde.text_document.uri) {
                                let flat = flatten_document_edits(&tde.edits);
                                self.apply_text_edits_to_path(&path, &flat)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn apply_text_edits_to_path(&mut self, path: &Path, edits: &[TextEdit]) -> Result<()> {
        if edits.is_empty() {
            return Ok(());
        }
        let tuples: Vec<(u32, u32, u32, u32, String)> = edits
            .iter()
            .map(|e| {
                (
                    e.range.start.line,
                    e.range.start.character,
                    e.range.end.line,
                    e.range.end.character,
                    e.new_text.clone(),
                )
            })
            .collect();
        let buf_idx = self
            .buffers
            .iter()
            .position(|b| b.path.as_deref() == Some(path));
        if let Some(idx) = buf_idx {
            if let Some(buf) = self.buffers.get_mut(idx) {
                buf.apply_text_edits(&tuples);
            }
        } else if path.exists() {
            let mut buf = Buffer::from_path(path)?;
            buf.apply_text_edits(&tuples);
            std::fs::write(path, buf.text())?;
        }
        Ok(())
    }

    async fn run_search(&mut self) -> Result<()> {
        let query = self.ui.search_input.trim().to_string();
        self.ui.mode = Mode::Normal;
        self.ui.search_input.clear();
        self.ui.sidebar = Sidebar::Search;
        self.ui.focus = Focus::Search;
        self.search.query = query.clone();
        self.search.searching = true;
        self.search.error = None;
        match search_project(self.workspace.root.clone(), query, 500).await {
            Ok(hits) => {
                self.search.hits = hits;
                self.search.selected = 0;
                self.search.searching = false;
                self.set_status(format!("{} hits", self.search.hits.len()), false);
            }
            Err(e) => {
                self.search.searching = false;
                self.search.error = Some(e.to_string());
                self.set_status(format!("search: {e}"), true);
            }
        }
        Ok(())
    }

    fn ensure_terminal(&mut self, cols: u16, rows: u16) -> Result<()> {
        if self.terminal.is_none() {
            self.terminal = Some(TerminalPane::spawn(
                &self.workspace.root,
                cols.max(10),
                rows.max(3),
            )?);
        } else if let Some(t) = self.terminal.as_mut() {
            t.resize(cols, rows);
        }
        Ok(())
    }

    async fn ensure_dap(&mut self) -> Result<()> {
        if self.dap.is_some() {
            return Ok(());
        }
        let Some(adapter) = find_adapter() else {
            self.set_status("no DAP adapter (lldb-dap) in PATH", true);
            return Ok(());
        };
        let client = DapClient::start(adapter, &self.workspace.root).await?;
        let program = self
            .workspace
            .root
            .join("target/debug/tui-ide");
        if program.is_file() {
            let _ = client.launch(&program, &[]).await;
            self.debug_log.push_str(&format!("launched {}\n", program.display()));
        } else {
            self.debug_log
                .push_str("DAP ready (no target/debug/tui-ide — set program manually later)\n");
        }
        self.dap = Some(client);
        self.apply_all_breakpoints().await?;
        Ok(())
    }

    async fn apply_all_breakpoints(&mut self) -> Result<()> {
        let Some(dap) = self.dap.as_ref() else {
            return Ok(());
        };
        let paths: Vec<PathBuf> = self
            .buffers
            .iter()
            .filter_map(|b| b.path.clone())
            .collect();
        for path in paths {
            let _ = dap.apply_breakpoints(&path).await;
        }
        Ok(())
    }

    async fn toggle_breakpoint(&mut self) -> Result<()> {
        let Some(buf) = self.active_buffer() else {
            return Ok(());
        };
        let Some(path) = buf.path.clone() else {
            self.set_status("no file", true);
            return Ok(());
        };
        let line = (buf.cursor.row + 1) as u32;
        self.ensure_dap().await?;
        if let Some(dap) = self.dap.as_ref() {
            dap.toggle_breakpoint(&path, line);
            let _ = dap.apply_breakpoints(&path).await;
            self.set_status(format!("breakpoint line {line}"), false);
        }
        Ok(())
    }

    async fn debug_continue(&mut self) -> Result<()> {
        self.ensure_dap().await?;
        if let Some(dap) = self.dap.as_mut() {
            let tid = self.debug_thread_id;
            if let Err(e) = dap.continue_exec(tid).await {
                self.set_status(format!("continue: {e}"), true);
            }
        }
        Ok(())
    }

    async fn debug_step_over(&mut self) -> Result<()> {
        self.ensure_dap().await?;
        if let Some(dap) = self.dap.as_mut() {
            let _ = dap.next(self.debug_thread_id).await;
        }
        Ok(())
    }

    async fn debug_step_in(&mut self) -> Result<()> {
        self.ensure_dap().await?;
        if let Some(dap) = self.dap.as_mut() {
            let _ = dap.step_in(self.debug_thread_id).await;
        }
        Ok(())
    }

    async fn poll_dap(&mut self) {
        let mut stopped = false;
        let Some(dap) = self.dap.as_mut() else {
            return;
        };
        for ev in dap.poll_events() {
            match ev {
                DapEvent::Stopped { reason, thread_id } => {
                    self.debug_thread_id = thread_id;
                    self.debug_log
                        .push_str(&format!("stopped: {reason} (thread {thread_id})\n"));
                    stopped = true;
                }
                DapEvent::Output(s) => {
                    self.debug_log.push_str(&s);
                    if !s.ends_with('\n') {
                        self.debug_log.push('\n');
                    }
                }
                DapEvent::Terminated => self.debug_log.push_str("terminated\n"),
                DapEvent::Exited(code) => {
                    self.debug_log.push_str(&format!("exited {code}\n"));
                }
            }
        }
        if self.debug_log.len() > 32_000 {
            self.debug_log.drain(..16_000);
        }
        if stopped {
            self.refresh_debug_locals().await;
        }
    }

    async fn handle_action(&mut self, action: Action, view_h: usize, term_rows: u16) -> Result<()> {
        match action {
            Action::Quit => self.request_quit().await?,
            Action::ForceQuit => {
                if self.ui.mode == Mode::ConfirmQuit {
                    self.confirm_quit_discard().await?;
                } else {
                    if let Some(nvim) = self.nvim.take() {
                        nvim.quit().await;
                    }
                    self.should_quit = true;
                }
            }
            Action::Save => self.save_active().await?,
            Action::CloseTab => {
                if self.ui.product == ProductMode::Database {
                    if self.db.close_active_tab() {
                        self.ui.db_cursor = Cursor::default();
                        self.ui.db_scroll_row = 0;
                        let _ = self.db.refresh_structure_for_active_tab().await;
                        self.set_status(
                            format!(
                                "closed · now {}",
                                self.db
                                    .tabs
                                    .get(self.db.active_tab)
                                    .map(|t| t.title.as_str())
                                    .unwrap_or("?")
                            ),
                            false,
                        );
                    } else {
                        self.set_status(self.db.status.clone(), true);
                    }
                } else if let Err(e) = self.buffers.close_active() {
                    self.set_status(e.to_string(), true);
                }
            }
            Action::ForceCloseTab => self.buffers.force_close_active(),
            Action::EnterInsert => {
                self.ui.mode = Mode::Insert;
                self.ui.focus = Focus::Editor;
                self.ui.hover = None;
            }
            Action::EnterNormal => {
                self.ui.mode = Mode::Normal;
                self.ui.show_completions = false;
                self.ui.hover = None;
            }
            Action::EnterCommand => {
                self.ui.mode = Mode::Command;
                self.ui.command.clear();
            }
            Action::OpenPalette => self.open_command_palette(),
            Action::OpenFilePalette => {
                let files = list_workspace_files(&self.workspace.root)?;
                self.ui.mode = Mode::Palette;
                self.ui.palette = Palette::open_files(&files, &self.workspace.root);
            }
            Action::OpenDbObjectPalette => self.open_db_object_palette(),
            Action::Cancel => {
                if self.ui.show_discover {
                    self.ui.show_discover = false;
                    return Ok(());
                }
                if self.ui.mode == Mode::Palette {
                    self.restore_theme_preview();
                }
                if self.ui.query_error.is_some() {
                    self.ui.query_error = None;
                }
                if self.db.query_running || self.pending_db_query {
                    self.db.request_query_cancel();
                    self.pending_db_query = false;
                    self.finish_query_ui();
                    self.set_status("cancelling query…", false);
                }
                let was_edit_cell = self.ui.mode == Mode::EditCell;
                let was_table_filter = self.ui.mode == Mode::EditTableFilter;
                let resume_console = self.ui.resume_console_insert
                    || matches!(
                        self.ui.palette.mode,
                        PaletteMode::ConsoleDatabase | PaletteMode::ConsoleSchema
                    );
                if resume_console {
                    self.resume_console_editing();
                    self.set_status("Console · INSERT · keep typing", false);
                } else {
                    self.ui.mode = Mode::Normal;
                    self.ui.command.clear();
                    self.ui.rename_input.clear();
                    self.ui.search_input.clear();
                    self.ui.find_query.clear();
                    self.ui.replace_query.clear();
                    self.ui.commit_input.clear();
                    self.ui.db_connect_input.clear();
                    self.ui.db_connect_form = DbConnectForm::default();
                    self.ui.db_connect_edit_id = None;
                    self.ui.open_folder_input.clear();
                    self.ui.import_csv_input.clear();
                    self.ui.query_params_for_explain = false;
                    self.ui.cell_edit_input.clear();
                    self.ui.table_filter_draft.clear();
                    self.ui.settings_draft.clear();
                    self.ui.confirm_delete_source_id = None;
                    self.ui.confirm_delete_label.clear();
                    self.ui.confirm_quit_label.clear();
                    self.pending_workspace_path = None;
                    self.ui.confirm_apply_pending = false;
                    self.ui.confirm_run_sql = None;
                    self.ui.rename_tab_pending = false;
                    self.ui.show_completions = false;
                    self.ui.hover = None;
                    self.ui.pending_code_actions.clear();
                    if self.ui.cell_modal && was_edit_cell {
                        self.ui.mode = Mode::Normal;
                        self.ui.focus = Focus::Results;
                        self.ui.cell_modal_drill = true;
                        self.ui.status = None;
                    } else if self.ui.cell_modal && !was_edit_cell {
                        if self.ui.cell_modal_drill {
                            self.ui.cell_modal_drill = false;
                            self.ui.cell_modal_tab = 0;
                            self.ui.mode = Mode::Normal;
                            self.ui.focus = Focus::Results;
                            self.ui.status = None;
                        } else {
                            self.ui.cell_modal = false;
                            self.ui.cell_modal_drill = false;
                            self.ui.mode = Mode::Normal;
                            self.ui.focus = Focus::Results;
                            self.ui.status = None;
                        }
                    } else if was_edit_cell || was_table_filter {
                        self.ui.focus = Focus::Results;
                        self.set_status(
                            if was_table_filter {
                                "filter cancelled"
                            } else {
                                "edit cancelled"
                            },
                            false,
                        );
                    } else if self.ui.focus == Focus::Results
                        && self.ui.product == ProductMode::Database
                    {
                        // Results Esc (no modal) → Console, same as old FocusEditor binding.
                        self.ui.focus = Focus::Editor;
                        self.ui.mode = Mode::Normal;
                        self.set_status("Console focused · i insert SQL · Ctrl-Enter run", false);
                    }
                }
            }
            Action::Confirm => match self.ui.mode {
                Mode::ConfirmQuit => self.confirm_quit_save().await?,
                Mode::ConfirmDeleteSource => {
                    if self.ui.confirm_apply_pending {
                        self.confirm_apply_updates().await?;
                    } else if self
                        .ui
                        .confirm_run_sql
                        .as_deref()
                        .is_some_and(|s| s != "__import_conn_pack__")
                    {
                        self.confirm_destructive_sql().await?;
                    } else {
                        self.ui.confirm_run_sql = None;
                        self.confirm_delete_connection()?;
                    }
                }
                Mode::Command => self.run_command().await?,
                Mode::Palette => self.run_palette().await?,
                Mode::Rename => self.confirm_rename().await?,
                Mode::Search => self.run_search().await?,
                Mode::Find => self.confirm_find().await?,
                Mode::EditTableFilter => self.confirm_table_filter().await?,
                Mode::Replace => self.confirm_replace().await?,
                Mode::Commit => self.confirm_git_commit().await?,
                Mode::DbConnect => self.confirm_db_connect().await?,
                Mode::CreateObject => self.confirm_create_object().await?,
                Mode::OpenFolder => self.confirm_open_folder().await?,
                Mode::ImportCsv => self.confirm_import_csv().await?,
                Mode::RestoreDump => self.confirm_restore_dump().await?,
                Mode::SchemaDiff => self.confirm_schema_diff().await?,
                Mode::DataCompare => self.confirm_data_compare().await?,
                Mode::QueryParams => self.confirm_query_params().await?,
                Mode::SaveMaterialize => self.confirm_save_materialize().await?,
                Mode::BrainAsk => self.confirm_brain_ask().await?,
                Mode::EditCell => self.confirm_edit_cell(),
                Mode::Settings => self.confirm_settings().await?,
                Mode::SessionForm => {
                    match self.sessions.commit_form() {
                        Ok(()) => {
                            self.ui.mode = Mode::Normal;
                            self.ui.focus = Focus::Sessions;
                            self.set_status("profile saved", false);
                        }
                        Err(e) => self.set_status(format!("session form: {e}"), true),
                    }
                }
                Mode::AgentForm => {
                    match self.agents.commit_form() {
                        Ok(()) => {
                            self.ui.mode = Mode::Normal;
                            self.ui.focus = Focus::Agents;
                            self.set_status("agent profile saved", false);
                        }
                        Err(e) => self.set_status(format!("agent form: {e}"), true),
                    }
                }
                Mode::Normal if self.ui.focus == Focus::Agents => {
                    let cols = 80u16;
                    let rows = term_rows.max(6);
                    let extra = self.db_agent_seed();
                    match self.agents.launch_selected(
                        &self.workspace.root,
                        cols,
                        rows,
                        extra.as_deref(),
                    ) {
                        Ok(out) => {
                            self.ui.bottom = BottomPanel::Terminal;
                            self.ui.focus = Focus::Terminal;
                            self.set_status(
                                format!("agent · {} · Ctrl-] next tab", out.label),
                                false,
                            );
                        }
                        Err(e) => self.set_status(format!("agent: {e}"), true),
                    }
                }
                Mode::Normal if self.ui.focus == Focus::Sessions => {
                    let cols = 80u16;
                    let rows = term_rows.max(6);
                    match self.sessions.connect_selected(&self.workspace.root, cols, rows) {
                        Ok(ConnectOutcome::SshAttached { label }) => {
                            self.ui.bottom = BottomPanel::Terminal;
                            self.ui.focus = Focus::Terminal;
                            self.set_status(format!("ssh · {label}"), false);
                        }
                        Ok(ConnectOutcome::RdpSpawned { label }) => {
                            self.set_status(format!("rdp launched · {label}"), false);
                        }
                        Err(e) => self.set_status(format!("session: {e}"), true),
                    }
                }
                Mode::Normal if self.ui.focus == Focus::Diagnostics => {
                    self.jump_diag_async().await;
                }
                Mode::Normal if self.ui.focus == Focus::Git => {
                    self.git_confirm().await?;
                }
                Mode::Normal if self.ui.focus == Focus::Search => {
                    self.search_confirm().await?;
                }
                Mode::Normal if self.ui.focus == Focus::Outline => {
                    self.outline_confirm().await;
                }
                Mode::Normal if self.ui.focus == Focus::Database => {
                    self.activate_db_selection(term_rows).await?;
                }
                Mode::Normal if self.ui.focus == Focus::Structure => {
                    self.structure_confirm(term_rows).await?;
                }
                Mode::Normal if self.ui.focus == Focus::Debug => {
                    self.refresh_debug_variables().await;
                }
                _ => {}
            },
            Action::MoveUp => match self.ui.mode {
                Mode::Palette => {
                    self.ui.palette.move_up();
                    self.preview_palette_theme();
                }
                Mode::Insert | Mode::Normal if self.ui.show_completions => {
                    self.ui.completion_idx = self.ui.completion_idx.saturating_sub(1);
                }
                _ => match self.ui.focus {
                    Focus::Tree => self.workspace.move_up(),
                    Focus::Database => {
                        self.db.move_up();
                        self.db_explorer_moved(term_rows).await;
                    }
                    Focus::Structure => {
                        self.db.structure_scroll =
                            self.db.structure_scroll.saturating_sub(1);
                    }
                    Focus::Results => {
                        if let Some(result) = self.db.last_result.as_mut() {
                            result.move_cell(-1, 0);
                        }
                    }
                    Focus::Diagnostics => {
                        self.ui.diag_selected = self.ui.diag_selected.saturating_sub(1);
                    }
                    Focus::Git => {
                        if let Some(git) = self.git.as_mut() {
                            git.move_up();
                            let _ = git.load_diff().await;
                        }
                    }
                    Focus::Debug => {
                        if self.ui.debug_frame_selected > 0 {
                            self.ui.debug_frame_selected -= 1;
                        }
                    }
                    Focus::Search => self.search.move_up(),
                    Focus::Sessions => self.sessions.move_up(),
                    Focus::Agents => self.agents.move_up(),
                    Focus::Outline => {
                        self.ui.outline_selected = self.ui.outline_selected.saturating_sub(1);
                    }
                    Focus::Editor if self.editing_db_sql() => {
                        self.db_move_up();
                        self.db_ensure_cursor_visible(view_h);
                    }
                    Focus::Editor => {
                        if let Some(b) = self.active_buffer_mut() {
                            b.move_up();
                            b.ensure_cursor_visible(view_h);
                            self.update_breadcrumbs();
                        }
                    }
                    _ => {}
                },
            },
            Action::MoveDown => match self.ui.mode {
                Mode::Palette => {
                    self.ui.palette.move_down();
                    self.preview_palette_theme();
                }
                Mode::Insert | Mode::Normal if self.ui.show_completions => {
                    if self.ui.completion_idx + 1 < self.ui.completions.len().min(10) {
                        self.ui.completion_idx += 1;
                    }
                }
                _ => match self.ui.focus {
                    Focus::Tree => self.workspace.move_down(),
                    Focus::Database => {
                        self.db.move_down();
                        self.db_explorer_moved(term_rows).await;
                    }
                    Focus::Structure => {
                        let max = self.db.structure_lines.len().saturating_sub(1);
                        if self.db.structure_scroll < max {
                            self.db.structure_scroll += 1;
                        }
                    }
                    Focus::Results => {
                        if let Some(result) = self.db.last_result.as_mut() {
                            result.move_cell(1, 0);
                        }
                    }
                    Focus::Diagnostics => {
                        let n = self.diagnostics().len();
                        if self.ui.diag_selected + 1 < n {
                            self.ui.diag_selected += 1;
                        }
                    }
                    Focus::Git => {
                        if let Some(git) = self.git.as_mut() {
                            git.move_down();
                            let _ = git.load_diff().await;
                        }
                    }
                    Focus::Debug => {
                        if self.ui.debug_frame_selected + 1 < self.ui.stack_frames.len() {
                            self.ui.debug_frame_selected += 1;
                        }
                    }
                    Focus::Search => self.search.move_down(),
                    Focus::Sessions => self.sessions.move_down(),
                    Focus::Agents => self.agents.move_down(),
                    Focus::Outline => {
                        if self.ui.outline_selected + 1 < self.ui.outline.len() {
                            self.ui.outline_selected += 1;
                        }
                    }
                    Focus::Editor if self.editing_db_sql() => {
                        self.db_move_down();
                        self.db_ensure_cursor_visible(view_h);
                    }
                    Focus::Editor => {
                        if let Some(b) = self.active_buffer_mut() {
                            b.move_down();
                            b.ensure_cursor_visible(view_h);
                            self.update_breadcrumbs();
                        }
                    }
                    _ => {}
                },
            },
            Action::MoveLeft => {
                if self.ui.focus == Focus::Results {
                    if let Some(result) = self.db.last_result.as_mut() {
                        result.move_cell(0, -1);
                    }
                    self.nudge_results_h_scroll();
                } else if self.editing_db_sql() {
                    self.db_move_left();
                } else if let Some(b) = self.active_buffer_mut() {
                    b.move_left();
                }
            }
            Action::MoveRight => {
                if self.ui.focus == Focus::Results {
                    if let Some(result) = self.db.last_result.as_mut() {
                        result.move_cell(0, 1);
                    }
                    self.nudge_results_h_scroll();
                } else if self.editing_db_sql() {
                    self.db_move_right();
                } else if let Some(b) = self.active_buffer_mut() {
                    b.move_right();
                }
            }
            Action::MoveLineStart => {
                if self.editing_db_sql() {
                    self.db_move_line_start();
                } else if let Some(b) = self.active_buffer_mut() {
                    b.move_line_start();
                }
            }
            Action::MoveLineEnd => {
                if self.editing_db_sql() {
                    self.db_move_line_end();
                } else if let Some(b) = self.active_buffer_mut() {
                    b.move_line_end();
                }
            }
            Action::PageUp => {
                if self.editing_db_sql() {
                    for _ in 0..view_h.saturating_sub(1) {
                        self.db_move_up();
                    }
                    self.db_ensure_cursor_visible(view_h);
                } else if let Some(b) = self.active_buffer_mut() {
                    for _ in 0..view_h.saturating_sub(1) {
                        b.move_up();
                    }
                    b.ensure_cursor_visible(view_h);
                }
            }
            Action::PageDown => {
                if self.editing_db_sql() {
                    for _ in 0..view_h.saturating_sub(1) {
                        self.db_move_down();
                    }
                    self.db_ensure_cursor_visible(view_h);
                } else if let Some(b) = self.active_buffer_mut() {
                    for _ in 0..view_h.saturating_sub(1) {
                        b.move_down();
                    }
                    b.ensure_cursor_visible(view_h);
                }
            }
            Action::InsertNewline => {
                if self.ui.show_completions {
                    self.accept_completion();
                    self.sync_lsp_change().await;
                } else if self.editing_db_sql() {
                    self.db_insert_newline();
                    self.db_ensure_cursor_visible(view_h);
                } else if let Some(b) = self.active_buffer_mut() {
                    b.insert_newline();
                    b.ensure_cursor_visible(view_h);
                    self.sync_lsp_change().await;
                }
            }
            Action::Backspace => match self.ui.mode {
                Mode::Command => {
                    self.ui.command.pop();
                }
                Mode::Rename => {
                    self.ui.rename_input.pop();
                }
                Mode::Search => {
                    self.ui.search_input.pop();
                }
                Mode::Find => {
                    self.ui.find_query.pop();
                }
                Mode::EditTableFilter => {
                    self.ui.table_filter_draft.pop();
                    self.refresh_table_filter_completions();
                }
                Mode::Replace => {
                    self.ui.replace_query.pop();
                }
                Mode::Commit => {
                    self.ui.commit_input.pop();
                }
                Mode::DbConnect => {
                    self.ui.db_connect_form.backspace();
                }
                Mode::CreateObject => {
                    self.ui.create_object_form.backspace();
                }
                Mode::OpenFolder => {
                    self.ui.open_folder_input.pop();
                }
                Mode::ImportCsv
                | Mode::RestoreDump
                | Mode::SchemaDiff
                | Mode::DataCompare
                | Mode::QueryParams
                | Mode::SaveMaterialize
                | Mode::BrainAsk => {
                    self.ui.import_csv_input.pop();
                }
                Mode::EditCell => {
                    self.ui.cell_edit_input.pop();
                }
                Mode::Settings => {
                    self.ui.settings_draft.pop();
                }
                Mode::SessionForm => self.session_form_pop_char(),
                Mode::AgentForm => self.agent_form_pop_char(),
                Mode::Palette => {
                    self.ui.palette.backspace();
                    self.preview_palette_theme();
                }
                Mode::Insert if self.editing_db_sql() => {
                    self.db_backspace();
                    self.db_ensure_cursor_visible(view_h);
                }
                Mode::Insert => {
                    if let Some(b) = self.active_buffer_mut() {
                        b.backspace();
                        b.ensure_cursor_visible(view_h);
                    }
                    self.sync_lsp_change().await;
                }
                _ => {}
            },
            Action::Delete => {
                if self.editing_db_sql() {
                    self.db_delete_forward();
                    self.db_ensure_cursor_visible(view_h);
                } else if let Some(b) = self.active_buffer_mut() {
                    b.delete_forward();
                    self.sync_lsp_change().await;
                }
            }
            Action::Undo => {
                if self.ui.focus == Focus::Results && self.db.undo_last_cell_edit() {
                    self.set_status("cell edit undone", false);
                } else if self.editing_db_sql() {
                    self.db_undo_sql();
                } else if let Some(b) = self.active_buffer_mut() {
                    b.undo();
                    self.sync_lsp_change().await;
                }
            }
            Action::Redo => {
                if let Some(b) = self.active_buffer_mut() {
                    b.redo();
                }
                self.sync_lsp_change().await;
            }
            Action::NextTab => {
                self.buffers.next_tab();
                self.refresh_lsp_extras().await;
            }
            Action::PrevTab => {
                self.buffers.prev_tab();
                self.refresh_lsp_extras().await;
            }
            Action::ToggleTree => {
                if self.nvim_active() {
                    self.astro_cmd(
                        "Neotree filesystem focus left",
                        "Files focused · ↑↓ navigate · Enter open · Ctrl-B",
                    )
                    .await?;
                } else {
                    self.ui.show_sidebar = !self.ui.show_sidebar;
                    if self.ui.show_sidebar {
                        self.ui.focus = Focus::Tree;
                        self.ui.sidebar = Sidebar::Files;
                        self.set_status("file tree on", false);
                    } else {
                        self.ui.focus = Focus::Editor;
                        self.set_status("file tree off", false);
                    }
                }
            }
            Action::FocusEditor => {
                // Never leave a stuck inspector behind when Esc is remapped here.
                if self.ui.cell_modal {
                    if self.ui.cell_modal_drill {
                        self.ui.cell_modal_drill = false;
                        self.ui.cell_modal_tab = 0;
                    } else {
                        self.ui.cell_modal = false;
                        self.ui.cell_modal_drill = false;
                    }
                    self.ui.focus = Focus::Results;
                    self.ui.mode = Mode::Normal;
                    self.ui.status = None;
                } else if self.ui.product == ProductMode::Database
                    && self.ui.db_surface == DbSurface::Studio
                {
                    // Leaving a tool window overlay → back to Console.
                    if matches!(
                        self.ui.focus,
                        Focus::Git
                            | Focus::Search
                            | Focus::Outline
                            | Focus::Debug
                            | Focus::Terminal
                    ) {
                        self.ui.sidebar = Sidebar::Database;
                        self.ui.bottom = BottomPanel::Results;
                    }
                    self.ui.focus = Focus::Editor;
                    self.ui.mode = Mode::Normal;
                    self.set_status("Console focused · i insert SQL · Ctrl-Enter run", false);
                } else {
                    // Leave Agents workspace so Neovim is visible again.
                    if self.ui.sidebar == Sidebar::Agents {
                        self.ui.sidebar = Sidebar::Files;
                        self.ui.bottom = BottomPanel::Problems;
                    }
                    self.ui.focus = Focus::Editor;
                    self.ui.mode = Mode::Normal;
                }
            }
            Action::FocusTree | Action::FocusDbExplorer => {
                if self.ui.product == ProductMode::Database {
                    self.focus_db_explorer();
                    self.set_status("Explorer focused · j/k move · Enter expand", false);
                } else if self.nvim_active() {
                    self.astro_cmd(
                        "Neotree filesystem focus left",
                        "Files focused · ↑↓ navigate · Enter open",
                    )
                    .await?;
                } else {
                    self.ui.focus = Focus::Tree;
                    self.ui.sidebar = Sidebar::Files;
                    self.ui.mode = Mode::Normal;
                    self.ui.show_sidebar = true;
                }
            }
            Action::CycleDbPane => {
                self.cycle_db_pane();
                if self.ui.focus == Focus::Structure {
                    let _ = self.db.refresh_structure_from_console_query().await;
                }
            }
            Action::FocusDiagnostics => {
                self.ui.focus = Focus::Diagnostics;
                self.ui.bottom = BottomPanel::Problems;
                self.ui.mode = Mode::Normal;
            }
            Action::ToggleOrOpen => {
                if self.ui.focus == Focus::Database {
                    self.activate_db_selection(term_rows).await?;
                } else if self.ui.focus == Focus::Structure {
                    // ignore — Structure has no expand action
                } else if let Some(path) = self.workspace.toggle_or_open() {
                    if let Err(e) = self.open_path(path).await {
                        self.set_status(e.to_string(), true);
                    }
                }
            }
            Action::RequestCompletion => self.request_completion().await,
            Action::GotoDefinition => self.goto_definition().await,
            Action::Hover => self.hover().await,
            Action::AcceptCompletion => {
                if self.ui.mode == Mode::EditTableFilter {
                    self.accept_table_filter_completion();
                } else {
                    self.accept_completion();
                    self.sync_lsp_change().await;
                }
            }
            Action::NextCompletion => {
                if self.ui.completions.is_empty() {
                    // nothing
                } else if self.ui.completion_idx + 1 < self.ui.completions.len().min(10) {
                    self.ui.completion_idx += 1;
                }
            }
            Action::PrevCompletion => {
                self.ui.completion_idx = self.ui.completion_idx.saturating_sub(1);
            }
            Action::RefreshTree => {
                if self.ui.focus == Focus::Git {
                    if let Some(git) = self.git.as_mut() {
                        match git.refresh().await {
                            Ok(()) => self.set_status("git refreshed", false),
                            Err(e) => self.set_status(format!("git: {e:#}"), true),
                        }
                    }
                } else if self.ui.focus == Focus::Database {
                    self.db.rebuild_flat();
                    self.set_status("database tree refreshed", false);
                } else if let Err(e) = self.workspace.refresh() {
                    self.set_status(e.to_string(), true);
                } else {
                    self.set_status("tree refreshed", false);
                }
            }
            Action::Format => self.format_active().await?,
            Action::Rename => self.start_rename().await,
            Action::CodeAction => self.fetch_code_actions().await?,
            Action::ToggleInlayHints => {
                self.ui.show_inlays = !self.ui.show_inlays;
                self.refresh_lsp_extras().await;
            }
            Action::OpenSearch => {
                if self.nvim_active() {
                    self.astro_cmd(
                        "lua pcall(function() Snacks.picker.grep() end)",
                        "Astro search · Ctrl-⇧F · <Space>fw",
                    )
                    .await?;
                } else if self.ui.product == ProductMode::Database
                    && self.ui.db_surface == DbSurface::Studio
                {
                    // In Studio prefer Search Everywhere; project grep still available.
                    self.open_search_everywhere();
                } else {
                    self.ui.mode = Mode::Search;
                    self.ui.search_input.clear();
                    self.ui.sidebar = Sidebar::Search;
                    self.ui.focus = Focus::Search;
                    self.ui.show_sidebar = true;
                }
            }
            Action::ProductTab(n) => self.goto_product_tab(n).await?,
            Action::OpenGit => {
                // Studio: no in-app Git / lazygit surface (removed by product choice).
                if self.ui.product == ProductMode::Database {
                    return Ok(());
                }
                if self.nvim_active() {
                    self.astro_cmd(
                        "lua pcall(function() Snacks.lazygit() end)",
                        "Astro lazygit · Ctrl-⇧G · <Space>tl",
                    )
                    .await?;
                }
            }
            Action::GoToDdl => self.go_to_ddl().await?,
            Action::OpenCreateMenu => self.open_create_menu(),
            Action::SearchEverywhere => self.open_search_everywhere(),
            Action::OpenConsoleDatabase => self.open_console_database_palette().await,
            Action::OpenConsoleSchema => self.open_console_schema_palette().await,
            Action::CycleSelectLimit => self.cycle_select_limit(),
            Action::EditTableWhere => self.begin_table_filter_edit(TableFilterField::Where),
            Action::EditTableOrderBy => self.begin_table_filter_edit(TableFilterField::OrderBy),
            Action::OpenOutline => {
                if self.nvim_active() {
                    self.astro_cmd(
                        "AerialToggle!",
                        "Astro aerial · Ctrl-⇧O · <Space>lS",
                    )
                    .await?;
                } else if self.ui.product == ProductMode::Database
                    && self.ui.db_surface == DbSurface::Studio
                {
                    if self.ui.focus == Focus::Outline {
                        self.ui.focus = Focus::Structure;
                        self.ui.sidebar = Sidebar::Database;
                        self.set_status("Hierarchy closed", false);
                    } else {
                        self.ui.sidebar = Sidebar::Outline;
                        self.ui.focus = Focus::Outline;
                        self.ui.show_outline = true;
                        let _ = self.db.refresh_structure_for_selection().await;
                        self.set_status("Hierarchy · Esc back", false);
                    }
                } else {
                    self.ui.show_outline = !self.ui.show_outline;
                    if self.ui.show_outline {
                        self.refresh_lsp_extras().await;
                        self.set_status("outline on · Ctrl-⇧-O toggle", false);
                    } else {
                        self.set_status("outline off", false);
                    }
                }
            }
            Action::OpenTerminal => {
                if self.nvim_active()
                    && self.ui.sidebar != Sidebar::Agents
                    && self.ui.sidebar != Sidebar::Sessions
                {
                    self.astro_cmd(
                        "lua pcall(function() Snacks.terminal() end)",
                        "Astro terminal · Ctrl-` · <Space>tt",
                    )
                    .await?;
                } else if self.ui.bottom == BottomPanel::Terminal && self.ui.focus == Focus::Terminal
                {
                    self.ui.bottom = BottomPanel::Problems;
                    self.ui.focus = Focus::Editor;
                    self.set_status("terminal closed", false);
                } else {
                    self.ui.bottom = BottomPanel::Terminal;
                    self.ui.focus = Focus::Terminal;
                    let cols = 80u16;
                    self.ensure_terminal(cols, term_rows.max(6))?;
                    self.set_status("terminal · Ctrl-` close · Esc/Ctrl-` focus editor", false);
                }
            }
            Action::OpenDebug => {
                self.ui.sidebar = Sidebar::Debug;
                self.ui.bottom = BottomPanel::Debug;
                self.ui.focus = Focus::Debug;
                self.ui.show_sidebar = true;
                self.ensure_dap().await?;
            }
            Action::SidebarFiles => {
                self.ui.sidebar = Sidebar::Files;
                self.ui.focus = Focus::Tree;
                self.ui.show_sidebar = true;
            }
            Action::StageFile => {
                if let Some(git) = self.git.as_mut() {
                    if let Err(e) = git.stage_selected().await {
                        self.set_status(e.to_string(), true);
                    }
                }
            }
            Action::UnstageFile => {
                if let Some(git) = self.git.as_mut() {
                    if let Err(e) = git.unstage_selected().await {
                        self.set_status(e.to_string(), true);
                    }
                }
            }
            Action::Blame => {
                if let Some(path) = self.active_buffer().and_then(|b| b.path.clone()) {
                    if let Some(git) = self.git.as_mut() {
                        let _ = git.blame(&path).await;
                        self.ui.bottom = BottomPanel::Problems;
                        self.set_status("blame loaded (see git diff panel)", false);
                    }
                }
            }
            Action::SplitVertical => {
                if !self.ui.split.enabled {
                    let next = if self.buffers.len() > 1 {
                        (self.buffers.active_index() + 1) % self.buffers.len()
                    } else {
                        self.buffers.active_index()
                    };
                    self.ui.split.secondary = Some(next);
                    self.ui.split.enabled = true;
                    self.ui.split.focus_secondary = false;
                } else {
                    self.ui.split.enabled = false;
                }
            }
            Action::FocusOtherSplit => {
                if self.ui.split.enabled {
                    self.ui.split.focus_secondary = !self.ui.split.focus_secondary;
                }
            }
            Action::AddCursorBelow => {
                if let Some(b) = self.active_buffer_mut() {
                    b.add_cursor_below();
                }
            }
            Action::ClearExtraCursors => {
                if let Some(b) = self.active_buffer_mut() {
                    b.clear_extra_cursors();
                }
            }
            Action::ToggleBreakpoint => self.toggle_breakpoint().await?,
            Action::DebugContinue => self.debug_continue().await?,
            Action::DebugStepOver => self.debug_step_over().await?,
            Action::DebugStepIn => self.debug_step_in().await?,
            Action::ApplyQuickFix => {
                if self.ui.product == ProductMode::Code
                    && self.ui.code_surface == CodeSurface::Home
                {
                    self.enter_code_editor();
                } else if self.ui.product == ProductMode::Database
                    && self.ui.db_surface == DbSurface::Home
                {
                    self.enter_db_studio();
                } else {
                    self.apply_quick_fix().await?;
                }
            }
            Action::NextProblem => {
                let n = self.diagnostics().len();
                if n > 0 {
                    self.ui.diag_selected = (self.ui.diag_selected + 1) % n;
                    self.ui.focus = Focus::Diagnostics;
                    self.ui.bottom = BottomPanel::Problems;
                }
            }
            Action::PrevProblem => {
                let n = self.diagnostics().len();
                if n > 0 {
                    self.ui.diag_selected = if self.ui.diag_selected == 0 {
                        n - 1
                    } else {
                        self.ui.diag_selected - 1
                    };
                    self.ui.focus = Focus::Diagnostics;
                    self.ui.bottom = BottomPanel::Problems;
                }
            }
            Action::ToggleProductMode => self.toggle_product_mode(),
            Action::EnterCodeEditor => {
                if self.ui.product == ProductMode::Database {
                    self.enter_db_studio();
                } else {
                    self.enter_code_editor();
                }
            }
            Action::EnterDbStudio => self.enter_db_studio(),
            Action::GoHome => {
                if self.ui.product == ProductMode::Database {
                    self.ui.show_help = false;
                    self.ui.show_discover = false;
                    self.ui.busy_label = None;
                    self.go_db_home();
                } else {
                    self.go_home();
                }
            }
            Action::ToggleHelp => {
                self.ui.show_help = !self.ui.show_help;
                self.ui.show_discover = false;
            }
            Action::ToggleDiscover => {
                if self.ui.product == ProductMode::Database {
                    self.ui.show_discover = !self.ui.show_discover;
                    self.ui.show_help = false;
                } else {
                    // Code mode: Space still opens the command palette.
                    self.open_command_palette();
                }
            }
            Action::CycleTheme => {
                self.apply_theme_id(&ide_ui::theme::cycle_theme(true));
            }
            Action::ToggleResultsDensity => {
                self.ui.results_compact = !self.ui.results_compact;
                let label = if self.ui.results_compact {
                    "compact"
                } else {
                    "comfort"
                };
                self.set_status(format!("results density · {label}"), false);
            }
            Action::CycleDbSourceNext => self.cycle_db_source(1).await,
            Action::CycleDbSourcePrev => self.cycle_db_source(-1).await,
            Action::DeleteDataSource => {
                if let Some(id) = self.selected_connection_id() {
                    self.start_delete_connection(&id);
                } else {
                    self.set_status("select a connection in Explorer", true);
                }
            }
            Action::OpenFolder => self.start_open_folder(),
            Action::OpenProjects => self.open_projects_palette(),
            Action::OpenHomeSlot(n) => {
                if n == 0 {
                    self.open_settings();
                } else {
                    self.open_home_slot(n).await?;
                }
            }
            Action::OpenDbSourceSlot(n) => self.open_db_source_slot(n).await,
            Action::OpenDatabase => self.enter_database_mode(),
            Action::RunQuery => self.run_query().await?,
            Action::ResultPageNext => match self.db.next_result_page().await {
                Ok(()) => {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("page: {e:#}"), true),
            },
            Action::ResultPagePrev => match self.db.prev_result_page().await {
                Ok(()) => {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("page: {e:#}"), true),
            },
            Action::ChartResults => match self.db.chart_last_result(None) {
                Ok(_) => {
                    self.ui.focus = Focus::Results;
                    self.set_status("chart in Results", false);
                }
                Err(e) => self.set_status(format!("chart: {e:#}"), true),
            },
            Action::SaveResultTemp => self.start_save_result(true),
            Action::SaveResultView => self.start_save_result(false),
            Action::CancelQuery => {
                self.db.request_query_cancel();
                self.set_status("cancelling query…", false);
            },
            Action::AddDataSource => self.start_add_connection(),
            Action::EditDataSource | Action::ObjectProperties => {
                self.open_object_properties().await;
            }
            Action::DbConnectTest => {
                let form = self.ui.db_connect_form.clone();
                let edit_id = self.ui.db_connect_edit_id.clone();
                self.ui.db_connect_form.test_feedback =
                    Some(("testing…".into(), false));
                match Self::data_source_from_form(&form, edit_id.as_deref()) {
                    Ok(src) => {
                        self.set_status("testing connection…", false);
                        match ide_db::DbWorkspace::test_data_source(&src).await {
                            Ok(msg) => {
                                let text = format!("test ok · {msg}");
                                self.ui.db_connect_form.test_feedback =
                                    Some((text.clone(), false));
                                self.set_ok(text);
                            }
                            Err(e) => {
                                let text = format!("test failed: {e:#}");
                                let shown = Self::human_err(&text);
                                self.ui.db_connect_form.test_feedback =
                                    Some((shown.clone(), true));
                                self.set_status(text, true);
                            }
                        }
                    }
                    Err(e) => {
                        let text = format!("test: {e:#}");
                        let shown = Self::human_err(&text);
                        self.ui.db_connect_form.test_feedback = Some((shown.clone(), true));
                        self.set_status(text, true);
                    }
                }
            }
            Action::DbConnectNextField => {
                if self.ui.mode == Mode::EditTableFilter {
                    self.cycle_table_filter_field(true);
                } else {
                    self.ui.db_connect_form.next_field();
                }
            }
            Action::DbConnectPrevField => {
                if self.ui.mode == Mode::EditTableFilter {
                    self.cycle_table_filter_field(false);
                } else {
                    self.ui.db_connect_form.prev_field();
                }
            }
            Action::CreateObjectNextField => {
                let form = &mut self.ui.create_object_form;
                if form.on_add_button() {
                    form.add_column();
                } else {
                    form.next_field();
                }
            }
            Action::CreateObjectPrevField => self.ui.create_object_form.prev_field(),
            Action::CreateObjectToggleFlag => {
                let form = &mut self.ui.create_object_form;
                if form.on_flag_like_field() {
                    form.nudge_flag(true);
                }
            }
            Action::CreateObjectNudgeBack => {
                let form = &mut self.ui.create_object_form;
                if form.on_flag_like_field() {
                    form.nudge_flag(false);
                }
            }
            Action::DbConnectDriverNext => {
                let form = &mut self.ui.db_connect_form;
                if form.on_driver_field() {
                    form.cycle_driver(true);
                } else if form.on_accent_field() {
                    form.cycle_accent(true);
                } else if form.on_ssl_mode_field() {
                    form.cycle_ssl_mode(true);
                } else if form.on_role_field() {
                    form.cycle_role(true);
                } else if form.on_cycle_bool_field() {
                    form.cycle_bool_field(true);
                }
            }
            Action::DbConnectDriverPrev => {
                let form = &mut self.ui.db_connect_form;
                if form.on_driver_field() {
                    form.cycle_driver(false);
                } else if form.on_accent_field() {
                    form.cycle_accent(false);
                } else if form.on_ssl_mode_field() {
                    form.cycle_ssl_mode(false);
                } else if form.on_role_field() {
                    form.cycle_role(false);
                } else if form.on_cycle_bool_field() {
                    form.cycle_bool_field(false);
                }
            }
            Action::Find => {
                if self.ui.focus == Focus::Results {
                    self.ui.find_query = self.db.results_filter.clone();
                }
                self.ui.mode = Mode::Find;
            }
            Action::FindNext => {
                let q = self.ui.find_query.clone();
                if q.trim().is_empty() {
                    self.ui.mode = Mode::Find;
                } else if let Some(buf) = self.active_buffer_mut() {
                    if !buf.find_next(&q) {
                        self.set_status("no more matches", true);
                    }
                }
            }
            Action::FindPrev => {
                let q = self.ui.find_query.clone();
                if q.trim().is_empty() {
                    self.ui.mode = Mode::Find;
                } else if let Some(buf) = self.active_buffer_mut() {
                    if !buf.find_prev(&q) {
                        self.set_status("no previous match", true);
                    }
                }
            }
            Action::Replace => {
                self.ui.mode = Mode::Replace;
                self.ui.replace_query.clear();
            }
            Action::ReplaceAll => {
                let q = self.ui.find_query.clone();
                let r = self.ui.replace_query.clone();
                if q.trim().is_empty() {
                    self.ui.mode = Mode::Find;
                } else if let Some(buf) = self.active_buffer_mut() {
                    let n = buf.replace_all_matches(&q, &r);
                    self.sync_lsp_change().await;
                    self.set_status(format!("replaced {n}"), false);
                }
            }
            Action::GitCommit => {
                self.ui.mode = Mode::Commit;
                self.ui.commit_input.clear();
            }
            Action::GitPush => {
                if let Some(git) = self.git.as_mut() {
                    match git.push().await {
                        Ok(out) => self.set_status(out.trim().lines().next().unwrap_or("pushed"), false),
                        Err(e) => self.set_status(format!("push: {e}"), true),
                    }
                }
            }
            Action::GitPull => {
                if let Some(git) = self.git.as_mut() {
                    match git.pull().await {
                        Ok(out) => self.set_status(out.trim().lines().next().unwrap_or("pulled"), false),
                        Err(e) => self.set_status(format!("pull: {e}"), true),
                    }
                }
            }
            Action::CheckoutBranch => {
                if let Some(git) = self.git.as_mut() {
                    match git.checkout_selected_branch().await {
                        Ok(()) => {
                            let branch = git.branch.clone();
                            self.set_status(format!("on {branch}"), false);
                        }
                        Err(e) => self.set_status(format!("checkout: {e}"), true),
                    }
                }
            }
            Action::ExplainQuery => self.explain_query().await?,
            Action::ApplyRowUpdates => self.apply_row_updates().await?,
            Action::ExportCsv => self.export_results_csv()?,
            Action::ExportSql => self.export_results_sql()?,
            Action::ImportCsv => self.start_import_csv(),
            Action::DumpDatabase => self.dump_database().await?,
            Action::RestoreDump => self.start_restore_dump(),
            Action::SchemaDiff => self.start_schema_diff(),
            Action::SchemaDiffDdl => self.generate_schema_diff_ddl().await?,
            Action::DataCompare => self.start_data_compare(),
            Action::KillSession => self.kill_session().await?,
            Action::BookmarkSql => {
                self.db.bookmark_current_sql();
                self.set_status(self.db.status.clone(), false);
            }
            Action::EditCell => self.start_edit_cell(),
            Action::OpenCellModal => self.open_cell_modal(),
            Action::YankResultCell => self.yank_result_cell(),
            Action::YankResultRow => self.yank_result_row(),
            Action::OpenResultsActions => self.open_results_actions(),
            Action::SelectAll => self.select_all_sql_or_editor(),
            Action::ClipboardCopy => {
                if self.ui.focus == Focus::Results {
                    self.yank_result_cell();
                } else {
                    self.clipboard_copy_sql_or_editor();
                }
            }
            Action::ClipboardPaste => self.clipboard_paste_sql_or_editor(),
            Action::FocusResults => {
                self.ui.bottom = BottomPanel::Results;
                self.ui.focus = Focus::Results;
                self.ui.mode = Mode::Normal;
                if self.ui.product == ProductMode::Database {
                    self.set_status(
                        "Results focused · ←↑↓→ cells · i edit · Ctrl-S apply",
                        false,
                    );
                }
            }
            Action::NextBranch => {
                if let Some(git) = self.git.as_mut() {
                    git.branch_next();
                }
            }
            Action::PrevBranch => {
                if let Some(git) = self.git.as_mut() {
                    git.branch_prev();
                }
            }
            Action::RunPlugin => {
                let pairs: Vec<(String, String)> = self
                    .plugins
                    .all_commands()
                    .into_iter()
                    .map(|(plugin, id, title)| {
                        (format!("[{plugin}] {title}"), format!("plugin:{id}"))
                    })
                    .collect();
                if pairs.is_empty() {
                    self.set_status("no plugin commands", true);
                } else {
                    self.ui.mode = Mode::Palette;
                    self.ui.palette = Palette::open_actions(pairs);
                }
            }
            Action::OpenSessions => {
                // Home dashboard has no sidebar — jump to Editor chrome so Sessions list is visible.
                if self.ui.product != ProductMode::Code {
                    self.ui.product = ProductMode::Code;
                }
                if self.ui.code_surface != CodeSurface::Editor {
                    self.ui.code_surface = CodeSurface::Editor;
                    self.last_code_surface = CodeSurface::Editor;
                }
                self.ui.mode = Mode::Normal;
                self.ui.sidebar = Sidebar::Sessions;
                self.ui.focus = Focus::Sessions;
                self.ui.show_sidebar = true;
                let _ = self.sessions.reload();
                self.sessions.reap_rdp();
                self.set_status(
                    "Sessions · Enter connect · a add SSH · A add RDP · e edit · x del",
                    false,
                );
            }
            Action::SessionConnect => {
                let cols = 80u16;
                let rows = 12u16;
                match self.sessions.connect_selected(&self.workspace.root, cols, rows) {
                    Ok(ConnectOutcome::SshAttached { label }) => {
                        self.ui.bottom = BottomPanel::Terminal;
                        self.ui.focus = Focus::Terminal;
                        self.set_status(format!("ssh · {label} · Ctrl-] next tab"), false);
                    }
                    Ok(ConnectOutcome::RdpSpawned { label }) => {
                        self.set_status(format!("rdp launched · {label}"), false);
                    }
                    Err(e) => self.set_status(format!("session: {e}"), true),
                }
            }
            Action::SessionAddSsh => {
                self.sessions.begin_add(true);
                self.ui.session_form_field = 0;
                self.ui.mode = Mode::SessionForm;
            }
            Action::SessionAddRdp => {
                self.sessions.begin_add(false);
                self.ui.session_form_field = 0;
                self.ui.mode = Mode::SessionForm;
            }
            Action::SessionEdit => match self.sessions.begin_edit_selected() {
                Ok(()) => {
                    self.ui.session_form_field = 0;
                    self.ui.mode = Mode::SessionForm;
                }
                Err(e) => self.set_status(e.to_string(), true),
            },
            Action::SessionDelete => match self.sessions.delete_selected() {
                Ok(()) => self.set_status("profile deleted", false),
                Err(e) => self.set_status(e.to_string(), true),
            },
            Action::SessionNextTab => {
                if !self.agents.live.is_empty() {
                    self.agents.next_tab();
                    self.ui.bottom = BottomPanel::Terminal;
                    self.ui.focus = Focus::Terminal;
                } else {
                    self.sessions.next_ssh_tab();
                    if !self.sessions.live_ssh.is_empty() {
                        self.ui.bottom = BottomPanel::Terminal;
                        self.ui.focus = Focus::Terminal;
                    }
                }
            }
            Action::SessionPrevTab => {
                if !self.agents.live.is_empty() {
                    self.agents.prev_tab();
                    self.ui.bottom = BottomPanel::Terminal;
                    self.ui.focus = Focus::Terminal;
                } else {
                    self.sessions.prev_ssh_tab();
                    if !self.sessions.live_ssh.is_empty() {
                        self.ui.bottom = BottomPanel::Terminal;
                        self.ui.focus = Focus::Terminal;
                    }
                }
            }
            Action::SessionCloseTab => {
                if !self.agents.live.is_empty() {
                    self.agents.close_active();
                    self.set_status("agent tab closed", false);
                } else {
                    self.sessions.close_active_ssh();
                    self.set_status("ssh tab closed", false);
                }
            }
            Action::SessionFormNextField => {
                self.ui.session_form_field = (self.ui.session_form_field + 1) % 6;
            }
            Action::SessionFormPrevField => {
                self.ui.session_form_field = if self.ui.session_form_field == 0 {
                    5
                } else {
                    self.ui.session_form_field - 1
                };
            }
            Action::OpenAgents => {
                if self.ui.product != ProductMode::Code {
                    self.ui.product = ProductMode::Code;
                }
                if self.ui.code_surface != CodeSurface::Editor {
                    self.ui.code_surface = CodeSurface::Editor;
                    self.last_code_surface = CodeSurface::Editor;
                }
                self.ui.mode = Mode::Normal;
                self.ui.sidebar = Sidebar::Agents;
                self.ui.focus = Focus::Agents;
                self.ui.show_sidebar = true;
                self.ui.bottom = BottomPanel::Terminal;
                let _ = self.agents.reload();
                self.set_status(
                    "Agents workspace · Enter launch · Esc/2 back to editor",
                    false,
                );
            }
            Action::AgentLaunch => {
                let cols = 100u16;
                let rows = term_rows.max(16);
                let extra = self.db_agent_seed();
                match self.agents.launch_selected(
                    &self.workspace.root,
                    cols,
                    rows,
                    extra.as_deref(),
                ) {
                    Ok(out) => {
                        self.ui.sidebar = Sidebar::Agents;
                        self.ui.show_sidebar = true;
                        self.ui.bottom = BottomPanel::Terminal;
                        self.ui.focus = Focus::Terminal;
                        self.set_status(
                            format!("agent · {} · full pane · Ctrl-] next", out.label),
                            false,
                        );
                    }
                    Err(e) => self.set_status(format!("agent: {e}"), true),
                }
            }
            Action::AgentAdd => {
                self.agents.begin_add();
                self.ui.agent_form_field = 0;
                self.ui.mode = Mode::AgentForm;
            }
            Action::AgentEdit => match self.agents.begin_edit_selected() {
                Ok(()) => {
                    self.ui.agent_form_field = 0;
                    self.ui.mode = Mode::AgentForm;
                }
                Err(e) => self.set_status(e.to_string(), true),
            },
            Action::AgentDelete => match self.agents.delete_selected() {
                Ok(()) => self.set_status("agent profile deleted", false),
                Err(e) => self.set_status(e.to_string(), true),
            },
            Action::AgentFormNextField => {
                self.ui.agent_form_field = (self.ui.agent_form_field + 1) % 5;
            }
            Action::AgentFormPrevField => {
                self.ui.agent_form_field = if self.ui.agent_form_field == 0 {
                    4
                } else {
                    self.ui.agent_form_field - 1
                };
            }
            Action::OpenSettings => self.open_settings(),
            Action::RefreshDebugLocals => self.refresh_debug_locals().await,
            Action::BeginTransaction => match self.db.begin_transaction().await {
                Ok(()) => self.set_status(self.db.status.clone(), false),
                Err(e) => self.set_status(format!("begin txn: {e:#}"), true),
            },
            Action::CommitTransaction => match self.db.commit_transaction().await {
                Ok(()) => self.set_status(self.db.status.clone(), false),
                Err(e) => self.set_status(format!("commit txn: {e:#}"), true),
            },
            Action::RollbackTransaction => match self.db.rollback_transaction().await {
                Ok(()) => self.set_status(self.db.status.clone(), false),
                Err(e) => self.set_status(format!("rollback txn: {e:#}"), true),
            },
        }
        Ok(())
    }

    async fn git_confirm(&mut self) -> Result<()> {
        let path = self
            .git
            .as_ref()
            .and_then(|g| g.files.get(g.selected))
            .map(|f| self.workspace.root.join(&f.path));
        if let Some(path) = path {
            if self.nvim.is_some() && self.ui.product == ProductMode::Code {
                self.ui.code_surface = CodeSurface::Editor;
                self.last_code_surface = CodeSurface::Editor;
                if let Some(nvim) = self.nvim.as_ref() {
                    nvim.open_file(&path).await?;
                }
                self.ui.focus = Focus::Editor;
                self.nvim_tab_tick = 29;
                self.config.remember_file(&path);
                self.set_status(format!("git → {}", path.display()), false);
                return Ok(());
            }
            self.open_path(path).await?;
        }
        Ok(())
    }

    async fn search_confirm(&mut self) -> Result<()> {
        let hit = self.search.selected_hit().cloned();
        if let Some(hit) = hit {
            let path = hit.path.clone();
            let line = hit.line;
            let column = hit.column;
            if self.nvim.is_some() && self.ui.product == ProductMode::Code {
                self.ui.code_surface = CodeSurface::Editor;
                self.last_code_surface = CodeSurface::Editor;
                self.ui.focus = Focus::Editor;
                self.ui.mode = Mode::Normal;
                if let Some(nvim) = self.nvim.as_ref() {
                    nvim.open_file_at(&path, line, column).await?;
                }
                self.nvim_tab_tick = 29;
                self.config.remember_file(&path);
                self.set_status(
                    format!("search → {}:{}:{}", path.display(), line, column),
                    false,
                );
                return Ok(());
            }
            self.open_path(path).await?;
            if let Some(buf) = self.buffers.active_mut() {
                buf.cursor.row = line.saturating_sub(1);
                buf.cursor.col = column.saturating_sub(1);
                buf.clamp_cursor();
            }
        }
        Ok(())
    }

    async fn outline_confirm(&mut self) {
        if let Some(sym) = self.ui.outline.get(self.ui.outline_selected).cloned() {
            if let Some(buf) = self.active_buffer_mut() {
                buf.cursor.row = sym.line as usize;
                buf.cursor.col = sym.character as usize;
                buf.clamp_cursor();
                self.ui.focus = Focus::Editor;
                self.update_breadcrumbs();
            }
        }
    }

    async fn run_command(&mut self) -> Result<()> {
        let cmd = self.ui.command.trim().to_string();
        self.ui.mode = Mode::Normal;
        self.ui.command.clear();
        match cmd.as_str() {
            "q" | "quit" => self.request_quit().await?,
            "q!" => {
                if let Some(nvim) = self.nvim.take() {
                    nvim.quit().await;
                }
                self.should_quit = true;
            }
            "w" | "write" => {
                if let Some(nvim) = self.nvim.as_ref() {
                    nvim.write_all().await?;
                    self.set_status("nvim :wall", false);
                } else {
                    self.save_active().await?;
                }
            }
            "wq" => {
                if self.nvim.is_some() {
                    self.confirm_quit_save().await?;
                } else {
                    self.save_active().await?;
                    if !self.buffers.iter().any(|b| b.dirty) {
                        self.should_quit = true;
                    }
                }
            }
            "e" => {
                let files = list_workspace_files(&self.workspace.root)?;
                self.ui.mode = Mode::Palette;
                self.ui.palette = Palette::open_files(&files, &self.workspace.root);
            }
            other if other.starts_with("e ") => {
                let path = other.trim_start_matches('e').trim();
                let full = if Path::new(path).is_absolute() {
                    PathBuf::from(path)
                } else {
                    self.workspace.root.join(path)
                };
                self.open_path(full).await?;
            }
            "" => {}
            other => self.set_status(format!("unknown :{other}"), true),
        }
        Ok(())
    }

    async fn run_palette(&mut self) -> Result<()> {
        let mode = self.ui.palette.mode;
        let item = self.ui.palette.selected_item().cloned();
        self.ui.mode = Mode::Normal;
        let Some(item) = item else {
            self.restore_theme_preview();
            return Ok(());
        };
        // Committing a theme (or leaving palette on another command): clear preview session.
        let committing_theme = item.id.starts_with("theme_")
            && item.id != "theme_new_custom"
            && item.id != "theme_reload";
        if !committing_theme {
            self.restore_theme_preview();
        } else {
            // Enter keeps the previewed theme — persist happens in theme_* handlers.
            self.theme_before_preview = None;
        }
        if mode == PaletteMode::CodeActions {
            if item.id.starts_with("cmd:") {
                self.set_status(format!("command not wired: {}", item.id), true);
                return Ok(());
            }
            if item.id.starts_with("noop-") {
                self.set_status("action has no edit", false);
                return Ok(());
            }
            // Real LSP code actions store a JSON WorkspaceEdit in `id`.
            // open_actions() also reuses this mode for plain ids (workspace:N, …)
            // — those must fall through to the handlers below.
            if item.id.starts_with('{') {
                if let Ok(edit) = serde_json::from_str::<WorkspaceEdit>(&item.id) {
                    self.apply_workspace_edit(&edit)?;
                    self.sync_lsp_change().await;
                    self.refresh_lsp_extras().await;
                    self.set_status("applied code action", false);
                } else {
                    self.set_status("failed to parse action edit", true);
                }
                self.ui.pending_code_actions.clear();
                return Ok(());
            }
        }
        if mode == PaletteMode::Projects {
            if item.id == "open_folder" {
                self.start_open_folder();
                return Ok(());
            }
            if let Some(path) = item.path {
                self.switch_workspace(path).await?;
            }
            return Ok(());
        }
        if mode == PaletteMode::ConsoleDatabase {
            if let Some(name) = item.id.strip_prefix("condb:") {
                match self.db.set_console_database(name).await {
                    Ok(()) => self.set_status(self.db.status.clone(), false),
                    Err(e) => self.set_status(format!("database: {e:#}"), true),
                }
            }
            self.resume_console_editing();
            return Ok(());
        }
        if mode == PaletteMode::ConsoleSchema {
            if let Some(name) = item.id.strip_prefix("conschema:") {
                match self.db.set_console_schema(name).await {
                    Ok(()) => self.set_status(self.db.status.clone(), false),
                    Err(e) => self.set_status(format!("schema: {e:#}"), true),
                }
            }
            self.resume_console_editing();
            return Ok(());
        }
        if mode == PaletteMode::DbObjects
            || (mode == PaletteMode::SearchEverywhere && item.id.starts_with("dbobj:"))
        {
            if let Some(idx_str) = item.id.strip_prefix("dbobj:") {
                if let Ok(idx) = idx_str.parse::<usize>() {
                    if self.db.select_flat_index(idx) {
                        self.db
                            .ensure_explorer_visible(Self::explorer_view_h(24));
                        self.ui.focus = Focus::Database;
                        let _ = self.db.refresh_structure_for_selection().await;
                        self.set_status(format!("→ {}", item.label), false);
                    }
                }
            }
            if mode == PaletteMode::DbObjects {
                return Ok(());
            }
            // SearchEverywhere dbobj handled — stop before command match.
            if item.id.starts_with("dbobj:") {
                return Ok(());
            }
        }
        if let Some(slot_str) = item.id.strip_prefix("workspace:") {
            if let Ok(slot) = slot_str.parse::<u8>() {
                self.open_db_source_slot(slot).await;
            }
            return Ok(());
        }
        if let Some(path) = item.path {
            self.open_path(path).await?;
            return Ok(());
        }
        match item.id.as_str() {
            "open_file" => {
                let files = list_workspace_files(&self.workspace.root)?;
                self.ui.mode = Mode::Palette;
                self.ui.palette = Palette::open_files(&files, &self.workspace.root);
            }
            "open_folder" => self.start_open_folder(),
            "projects" => self.open_projects_palette(),
            "save" => self.save_active().await?,
            "format" => self.format_active().await?,
            "rename" => self.start_rename().await,
            "code_action" => self.fetch_code_actions().await?,
            "toggle_hints" => {
                self.ui.show_inlays = !self.ui.show_inlays;
                self.refresh_lsp_extras().await;
            }
            "search" => {
                self.ui.mode = Mode::Search;
                self.ui.search_input.clear();
            }
            "find" => {
                self.ui.mode = Mode::Find;
            }
            "replace" => {
                self.ui.mode = Mode::Replace;
                self.ui.replace_query.clear();
            }
            "git_commit" => {
                self.ui.mode = Mode::Commit;
                self.ui.commit_input.clear();
            }
            "git_push" => {
                if let Some(git) = self.git.as_mut() {
                    let _ = git.push().await;
                }
            }
            "git_pull" => {
                if let Some(git) = self.git.as_mut() {
                    let _ = git.pull().await;
                }
            }
            "database" => self.enter_database_mode(),
            "run_query" => self.run_query().await?,
            "explain_query" => self.explain_query().await?,
            "apply_updates" => self.apply_row_updates().await?,
            "insert_row" => match self.db.insert_result_row() {
                Ok(()) => {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("insert row: {e:#}"), true),
            },
            "delete_row" => self.start_delete_result_row(),
            "toggle_cell_null" => match self.db.toggle_result_cell_null() {
                Ok(()) => {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("null: {e:#}"), true),
            },
            "migrate_dry_run" => self.start_migrate_dry_run(),
            "migrate_apply" => self.start_migrate_apply(),
            "migrate_status" => match self.db.migration_status() {
                Ok(()) => {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("migrate status: {e:#}"), true),
            },
            "migrate_rollback" => match self.db.rollback_last_migration().await {
                Ok(()) => {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("rollback: {e:#}"), true),
            },
            "view_wide_cell" => match self.db.last_result.as_ref().map(|r| r.wide_cell_text()) {
                Some(Ok(text)) => {
                    let rows: Vec<Vec<String>> =
                        text.lines().map(|l| vec![l.to_string()]).collect();
                    let n = rows.len();
                    self.db.last_result = Some(ide_db::QueryResult {
                        columns: vec!["wide_cell".into()],
                        rows,
                        affected: None,
                        elapsed_ms: 0,
                        truncated: false,
                        source_schema: None,
                        source_table: Some("wide_cell".into()),
                        primary_keys: Vec::new(),
                        original_rows: Vec::new(),
                        cursor_row: 0,
                        cursor_col: 0,
                        explain: None,
                        chart: None,
                        page: 0,
                        has_more: false,
                    });
                    self.ui.focus = Focus::Results;
                    self.set_status(format!("wide cell · {n} lines"), false);
                }
                Some(Err(e)) => self.set_status(format!("wide cell: {e:#}"), true),
                None => self.set_status("no result cell", true),
            },
            "paste_tsv" => {
                self.ui.mode = Mode::ImportCsv;
                self.ui.confirm_run_sql = Some("__paste_tsv__".into());
                self.ui.import_csv_input.clear();
                self.set_status("paste TSV/CSV · Enter apply into Results · Esc cancel", false);
            },
            "undo_cell_edit" => {
                if self.db.undo_last_cell_edit() {
                    self.set_status("cell edit undone", false);
                } else {
                    self.set_status("nothing to undo", true);
                }
            }
            "export_csv" => self.export_results_csv()?,
            "export_sql" => self.export_results_sql()?,
            "export_json" => self.export_results_json()?,
            "export_markdown" => self.export_results_markdown()?,
            "yank_result_cell" => self.yank_result_cell(),
            "yank_result_row" => self.yank_result_row(),
            "yank_result_all" => self.yank_result_all(),
            "edit_table_where" => self.begin_table_filter_edit(TableFilterField::Where),
            "edit_table_order" => self.begin_table_filter_edit(TableFilterField::OrderBy),
            "format_sql" => match self.db.format_active_sql() {
                Ok(()) => self.set_status(self.db.status.clone(), false),
                Err(e) => self.set_status(format!("format: {e:#}"), true),
            },
            "schema_diff_migration" => self.save_schema_diff_migration().await?,
            "drop_table" => self.start_destructive_sql(false),
            "truncate_table" => self.start_destructive_sql(true),
            "alter_add_column" => self.start_alter_column(AlterColumnKind::Add),
            "alter_drop_column" => self.start_alter_column(AlterColumnKind::Drop),
            "alter_rename_column" => self.start_alter_column(AlterColumnKind::Rename),
            "fan_out_sql" => match self.db.run_fan_out_pinned().await {
                Ok(()) => {
                    self.ui.focus = Focus::Results;
                    self.ui.bottom = BottomPanel::Results;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("fan-out: {e:#}"), true),
            },
            "export_conn_pack" => {
                let path = self
                    .workspace
                    .root
                    .join("exports")
                    .join(format!("connections-{}.toml", Self::export_stamp()));
                match self.db.export_connection_pack(&path) {
                    Ok(()) => self.set_status(format!("pack {}", path.display()), false),
                    Err(e) => self.set_status(format!("pack export: {e:#}"), true),
                }
            }
            "import_conn_pack" => {
                self.ui.mode = Mode::ImportCsv;
                self.ui.import_csv_input = self
                    .workspace
                    .root
                    .join("exports")
                    .join("connections.toml")
                    .display()
                    .to_string();
                self.set_status("connection pack path · Enter import · Esc cancel", false);
                self.ui.confirm_run_sql = Some("__import_conn_pack__".into());
            }
            "chart_results" => match self.db.chart_last_result(None) {
                Ok(_) => {
                    self.ui.focus = Focus::Results;
                    self.set_status("chart in Results", false);
                }
                Err(e) => self.set_status(format!("chart: {e:#}"), true),
            },
            "toggle_results_density" => {
                self.ui.results_compact = !self.ui.results_compact;
                let label = if self.ui.results_compact {
                    "compact"
                } else {
                    "comfortable"
                };
                self.set_status(format!("results density · {label}"), false);
            }
            "toggle_pin_columns" => {
                self.ui.results_pinned_cols = if self.ui.results_pinned_cols == 0 {
                    1
                } else if self.ui.results_pinned_cols == 1 {
                    2
                } else {
                    0
                };
                self.set_status(
                    format!("pinned columns · {}", self.ui.results_pinned_cols),
                    false,
                );
            }
            "er_sketch" => match self.db.er_sketch_active_schema().await {
                Ok(text) => {
                    let rows: Vec<Vec<String>> =
                        text.lines().map(|l| vec![l.to_string()]).collect();
                    let n = rows.len();
                    self.db.last_result = Some(ide_db::QueryResult {
                        columns: vec!["er".into()],
                        rows,
                        affected: None,
                        elapsed_ms: 0,
                        truncated: false,
                        source_schema: None,
                        source_table: Some("er_sketch".into()),
                        primary_keys: Vec::new(),
                        original_rows: Vec::new(),
                        cursor_row: 0,
                        cursor_col: 0,
                        explain: None,
                        chart: None,
                        page: 0,
                        has_more: false,
                    });
                    self.ui.focus = Focus::Results;
                    self.set_status(format!("ER sketch · {n} lines"), false);
                }
                Err(e) => self.set_status(format!("ER: {e:#}"), true),
            },
            "er_canvas" => match self.db.er_canvas_active_schema().await {
                Ok(text) => {
                    let rows: Vec<Vec<String>> =
                        text.lines().map(|l| vec![l.to_string()]).collect();
                    let n = rows.len();
                    self.db.last_result = Some(ide_db::QueryResult {
                        columns: vec!["er_canvas".into()],
                        rows,
                        affected: None,
                        elapsed_ms: 0,
                        truncated: false,
                        source_schema: None,
                        source_table: Some("er_canvas".into()),
                        primary_keys: Vec::new(),
                        original_rows: Vec::new(),
                        cursor_row: 0,
                        cursor_col: 0,
                        explain: None,
                        chart: None,
                        page: 0,
                        has_more: false,
                    });
                    self.ui.focus = Focus::Results;
                    self.set_status(format!("ER canvas · {n} lines"), false);
                }
                Err(e) => self.set_status(format!("ER canvas: {e:#}"), true),
            },
            "refresh_health" => {
                if let Some(id) = self.db.active_source.clone() {
                    match self.db.refresh_connection_health(&id).await {
                        Ok(()) => self.set_status(self.db.status.clone(), false),
                        Err(e) => self.set_status(format!("health: {e:#}"), true),
                    }
                } else {
                    self.set_status("no active source", true);
                }
            }
            "filter_results" => {
                self.ui.focus = Focus::Results;
                self.ui.find_query = self.db.results_filter.clone();
                self.ui.mode = Mode::Find;
            }
            "recent_tables" => {
                let items: Vec<(String, String)> = self
                    .db
                    .recent_tables
                    .iter()
                    .map(|k| {
                        let label = k.replace('|', " · ");
                        (label, format!("recent:{k}"))
                    })
                    .collect();
                if items.is_empty() {
                    self.set_status("no recent tables yet", true);
                } else {
                    let mut p = Palette::open_actions(items);
                    p.mode = PaletteMode::Commands;
                    self.ui.palette = p;
                    self.ui.mode = Mode::Palette;
                }
            }
            "insert_snippet" => {
                let _ = ide_db::ensure_example_snippet();
                let items: Vec<(String, String)> = ide_db::list_snippets()
                    .into_iter()
                    .map(|(name, _body)| (name.clone(), format!("snippet:{name}")))
                    .collect();
                if items.is_empty() {
                    self.set_status("no snippets in ~/.config/tui-ide/snippets", true);
                } else {
                    let mut p = Palette::open_actions(items);
                    p.mode = PaletteMode::Commands;
                    self.ui.palette = p;
                    self.ui.mode = Mode::Palette;
                }
            }
            "oauth_open" => {
                let driver = self
                    .db
                    .active_source
                    .as_ref()
                    .and_then(|id| self.db.sources.iter().find(|s| &s.id == id))
                    .map(|s| s.driver)
                    .unwrap_or(DbDriver::Bigquery);
                match ide_db::authorize_url(driver) {
                    Some(url) => match ide_db::open_browser(url) {
                        Ok(()) => self.set_status(
                            format!("opened OAuth · {} — then palette OAuth paste token", driver.label()),
                            false,
                        ),
                        Err(e) => self.set_status(format!("oauth open: {e:#}"), true),
                    },
                    None => self.set_status(
                        "OAuth browser MVP for BigQuery/Snowflake — set active cloud source",
                        true,
                    ),
                }
            }
            "oauth_paste" => self.start_oauth_paste(),
            "find_connection" => {
                let items = self.db.connection_search_items();
                if items.is_empty() {
                    self.set_status("no connections", true);
                } else {
                    let mut p = Palette::open_actions(items);
                    p.mode = PaletteMode::Commands;
                    self.ui.palette = p;
                    self.ui.mode = Mode::Palette;
                }
            }
            "query_history" => {
                let items = self.db.query_history_palette_items();
                if items.is_empty() {
                    self.set_status("no query history yet", true);
                } else {
                    let mut p = Palette::open_actions(items);
                    p.mode = PaletteMode::Commands;
                    self.ui.palette = p;
                    self.ui.mode = Mode::Palette;
                    self.set_status("query history — type to filter · Enter load", false);
                }
            }
            "script_object" => match self.db.script_object_at_selection().await {
                Ok(()) => {
                    self.ui.focus = Focus::Editor;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("script: {e:#}"), true),
            },
            "create_menu" => self.open_create_menu(),
            "console_database" => self.open_console_database_palette().await,
            "console_schema" => self.open_console_schema_palette().await,
            "search_everywhere" => self.open_search_everywhere(),
            "create_query" => match self.db.open_create_stub("create_query") {
                Ok(()) => {
                    self.ui.focus = Focus::Editor;
                    self.ui.mode = Mode::Insert;
                    self.set_status("new query console", false);
                }
                Err(e) => self.set_status(format!("create: {e:#}"), true),
            },
            id @ ("create_schema"
            | "create_table"
            | "create_view"
            | "create_matview"
            | "create_index"
            | "create_sequence"
            | "create_extension"
            | "create_role"
            | "create_policy"
            | "create_function"
            | "create_publication"
            | "create_tablespace"
            | "create_database") => {
                let driver = self
                    .db
                    .active_source
                    .as_ref()
                    .and_then(|sid| self.db.sources.iter().find(|s| s.id == *sid))
                    .map(|s| s.driver)
                    .unwrap_or_default();
                if let Some(kind) = CreateObjectKind::from_id(id) {
                    if !ide_ui::create_dialect_supports(driver, kind) {
                        self.set_status(
                            format!("{} not available on {}", kind.title(), driver.label()),
                            true,
                        );
                        return Ok(());
                    }
                }
                self.open_create_object_modal(id);
            }
            "pin_connection" => {
                let Some(id) = self.selected_connection_id() else {
                    self.set_status("select a connection first", true);
                    return Ok(());
                };
                match self.db.toggle_pin_source(&id) {
                    Ok(_) => self.set_status(self.db.status.clone(), false),
                    Err(e) => self.set_status(format!("pin: {e:#}"), true),
                }
            }
            "pin_tab" => {
                if self.db.toggle_pin_active_tab() {
                    self.set_status(self.db.status.clone(), false);
                }
            }
            "rename_tab" => {
                self.ui.rename_tab_pending = true;
                self.ui.oauth_paste_pending = false;
                self.ui.import_csv_input = self
                    .db
                    .tabs
                    .get(self.db.active_tab)
                    .map(|t| t.title.clone())
                    .unwrap_or_default();
                self.ui.mode = Mode::BrainAsk;
                self.set_status("rename tab · Enter", false);
            }
            "close_tab" => {
                if self.db.close_active_tab() {
                    self.set_status(
                        format!(
                            "closed · now {}",
                            self.db
                                .tabs
                                .get(self.db.active_tab)
                                .map(|t| t.title.as_str())
                                .unwrap_or("?")
                        ),
                        false,
                    );
                } else {
                    self.set_status(self.db.status.clone(), true);
                }
            }
            "new_tab" => {
                self.db.new_query_tab();
                self.ui.focus = Focus::Editor;
                self.set_status(self.db.status.clone(), false);
            }
            "theme_reload" => {
                ide_ui::theme::reload_customs();
                ide_ui::theme::set_mode_name(&self.config.theme.name);
                self.set_status(
                    format!(
                        "themes reloaded · {} custom",
                        ide_ui::theme::customs().len()
                    ),
                    false,
                );
            }
            "theme_new_custom" => {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() % 10_000)
                    .unwrap_or(0);
                let name = format!("custom{stamp}");
                let extends = ide_ui::theme::current_id();
                match ide_ui::theme::create_custom_theme(&name, &extends) {
                    Ok(path) => {
                        self.apply_theme_id(&name);
                        self.set_status(
                            format!(
                                "custom theme · edit {} · Theme: reload after changes",
                                path.display()
                            ),
                            false,
                        );
                    }
                    Err(e) => self.set_status(format!("theme new: {e:#}"), true),
                }
            }
            id if id.starts_with("theme_") => {
                let name = id.trim_start_matches("theme_");
                self.apply_theme_id(name);
            }
            "studio_help" => {
                let text = ide_db::DbWorkspace::studio_help_text();
                let rows: Vec<Vec<String>> = text.lines().map(|l| vec![l.to_string()]).collect();
                self.db.last_result = Some(ide_db::QueryResult {
                    columns: vec!["help".into()],
                    rows,
                    affected: None,
                    elapsed_ms: 0,
                    truncated: false,
                    source_schema: None,
                    source_table: Some("studio_help".into()),
                    primary_keys: Vec::new(),
                    original_rows: Vec::new(),
                    cursor_row: 0,
                    cursor_col: 0,
                    explain: None,
                    chart: None,
                    page: 0,
                    has_more: false,
                });
                self.ui.focus = Focus::Results;
                self.set_status("studio help", false);
            }
            "onboarding_tips" => {
                self.db.show_text_result("tips", ide_db::onboarding_tips());
                self.ui.focus = Focus::Results;
            }
            "recipe_gallery" => {
                let text = self.db.list_recipes_text();
                self.db.show_text_result("recipes", &text);
                self.ui.focus = Focus::Results;
            }
            "recipe_top_customers" => {
                let _ = self.db.insert_recipe("top_customers");
                self.set_status(self.db.status.clone(), false);
            }
            "recipe_revenue_by_day" => {
                let _ = self.db.insert_recipe("revenue_by_day");
                self.set_status(self.db.status.clone(), false);
            }
            "recipe_stratified_sample" => {
                let _ = self.db.insert_recipe("stratified_sample");
                self.set_status(self.db.status.clone(), false);
            }
            "recipe_embedding_similarity" => {
                let _ = self.db.insert_recipe("embedding_similarity");
                self.set_status(self.db.status.clone(), false);
            }
            "recipe_feature_table" => {
                let _ = self.db.insert_recipe("feature_table");
                self.set_status(self.db.status.clone(), false);
            }
            "recipe_batch_score" => {
                let _ = self.db.insert_recipe("batch_score");
                self.set_status(self.db.status.clone(), false);
            }
            "chart_bar" | "chart_line" | "chart_pie" => {
                let kind = item.id.strip_prefix("chart_").unwrap_or("bar");
                match self.db.chart_last_result_kind(kind) {
                    Ok(c) => {
                        self.set_status(format!("chart {kind}"), false);
                        let _ = c;
                        self.ui.focus = Focus::Results;
                    }
                    Err(e) => self.set_status(format!("chart: {e:#}"), true),
                }
            }
            "export_result_pack" => match self.db.export_explore_pack() {
                Ok(p) => self.set_status(format!("pack → {}", p.display()), false),
                Err(e) => self.set_status(format!("pack: {e:#}"), true),
            },
            "quality_strip" => match self.db.show_quality_strip() {
                Ok(()) => self.ui.focus = Focus::Results,
                Err(e) => self.set_status(format!("quality: {e:#}"), true),
            },
            "toggle_mask_pii" => {
                let on = self.db.toggle_mask_pii();
                self.set_status(if on { "PII mask ON" } else { "PII mask OFF" }, false);
            }
            "privilege_audit" => {
                let role = self
                    .db
                    .active_source
                    .as_ref()
                    .and_then(|id| self.db.sources.iter().find(|s| s.id == *id))
                    .map(|s| s.user.clone())
                    .unwrap_or_else(|| "public".into());
                let driver = self
                    .db
                    .active_source
                    .as_ref()
                    .and_then(|id| self.db.sources.iter().find(|s| s.id == *id))
                    .map(|s| s.driver)
                    .unwrap_or_default();
                match ide_db::privilege_audit_sql(driver, &role) {
                    Ok(sql) => {
                        self.db.sql = sql;
                        self.set_status(
                            format!("privilege audit SQL for {role} — F5 to run"),
                            false,
                        );
                    }
                    Err(e) => self.set_status(format!("audit: {e:#}"), true),
                }
            }
            "export_audit_csv" => {
                let path = self.workspace.root.join("queries").join("audit_export.csv");
                match ide_db::audit_export_csv(&path) {
                    Ok(()) => self.set_status(format!("audit → {}", path.display()), false),
                    Err(e) => self.set_status(format!("audit csv: {e:#}"), true),
                }
            }
            "diagnose_pack" => {
                let (schema, table) = self
                    .db
                    .last_result
                    .as_ref()
                    .map(|r| {
                        (
                            r.source_schema.clone().unwrap_or_else(|| "public".into()),
                            r.source_table.clone().unwrap_or_else(|| "table".into()),
                        )
                    })
                    .unwrap_or_else(|| ("public".into(), "table".into()));
                let explain = self
                    .db
                    .last_explain
                    .clone()
                    .unwrap_or_else(|| "(run Ctrl-E EXPLAIN first)".into());
                let stats = self
                    .db
                    .last_result
                    .as_ref()
                    .map(ide_db::quality_strip)
                    .unwrap_or_else(|| "(no results)".into());
                let pack = ide_db::diagnose_pack_template(&schema, &table, &explain, &stats);
                let path = self
                    .workspace
                    .root
                    .join("queries")
                    .join(format!("diagnose-{schema}-{table}.txt"));
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match std::fs::write(&path, &pack) {
                    Ok(()) => {
                        self.db.show_text_result("diagnose", &pack);
                        self.ui.focus = Focus::Results;
                        self.set_status(format!("diagnose pack → {}", path.display()), false);
                    }
                    Err(e) => self.set_status(format!("diagnose: {e:#}"), true),
                }
            }
            "lineage_sketch" => {
                let focus = self
                    .db
                    .last_result
                    .as_ref()
                    .and_then(|r| r.source_table.clone())
                    .unwrap_or_default();
                let edges = match self.db.er_sketch_active_schema().await {
                    Ok(sketch) => ide_db::parse_er_edges(&sketch),
                    Err(_) => Vec::new(),
                };
                let text = ide_db::lineage_from_er(&edges, &focus);
                self.db.show_text_result("lineage", &text);
                self.ui.focus = Focus::Results;
            }
            "cdc_lag" => {
                let driver = self
                    .db
                    .active_source
                    .as_ref()
                    .and_then(|id| self.db.sources.iter().find(|s| s.id == *id))
                    .map(|s| s.driver)
                    .unwrap_or_default();
                match ide_db::cdc_lag_sql(driver) {
                    Ok(sql) => {
                        self.db.sql = sql.to_string();
                        self.set_status("CDC lag SQL loaded — F5", false);
                    }
                    Err(e) => self.set_status(format!("cdc: {e:#}"), true),
                }
            }
            "dbt_detect" => {
                match ide_db::detect_dbt_project(&self.workspace.root) {
                    Some(p) => self.set_status(format!("dbt project: {}", p.display()), false),
                    None => self.set_status("no dbt_project.yml above workspace root", true),
                }
            }
            "dbt_run" | "dbt_test" | "dbt_compile" => {
                let sub = item.id.strip_prefix("dbt_").unwrap_or("run");
                match ide_db::detect_dbt_project(&self.workspace.root) {
                    Some(p) => {
                        let cmd = ide_db::dbt_command(&p, sub, None);
                        self.db.sql = format!("-- Agents / Terminal recipe:\n-- {cmd}\n");
                        self.set_status(
                            format!("dbt {sub} recipe ready — Agents → dbt shell or Terminal"),
                            false,
                        );
                    }
                    None => self.set_status("dbt project not found", true),
                }
            }
            "dbt_schema_stubs" => {
                let path = ide_db::detect_dbt_project(&self.workspace.root)
                    .map(|p| p.join("models").join("schema.yml"))
                    .unwrap_or_else(|| self.workspace.root.join("models/schema.yml"));
                match ide_db::parse_dbt_schema_yml(&path) {
                    Ok(text) => {
                        self.db.show_text_result("dbt_yml", &text);
                        self.ui.focus = Focus::Results;
                    }
                    Err(e) => self.set_status(format!("dbt yml: {e:#}"), true),
                }
            }
            "airflow_link" => match ide_db::airflow_deep_link(&self.db.persona) {
                Ok(url) => {
                    self.db.show_text_result("airflow", &format!("Open: {url}\n"));
                    self.ui.focus = Focus::Results;
                    let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
                    self.set_status(url, false);
                }
                Err(e) => self.set_status(format!("airflow: {e:#}"), true),
            },
            "bi_deeplink" => {
                let schema = self
                    .db
                    .last_result
                    .as_ref()
                    .and_then(|r| r.source_schema.clone())
                    .unwrap_or_else(|| "public".into());
                let table = self
                    .db
                    .last_result
                    .as_ref()
                    .and_then(|r| r.source_table.clone())
                    .unwrap_or_else(|| "table".into());
                match ide_db::bi_deep_link(&self.db.persona, &schema, &table) {
                    Ok(url) => {
                        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
                        self.set_status(url, false);
                    }
                    Err(e) => self.set_status(format!("bi: {e:#}"), true),
                }
            }
            "bi_export_csv" => {
                let dir = if self.db.persona.bi_export_dir.is_empty() {
                    self.workspace.root.join("bi_export")
                } else {
                    PathBuf::from(&self.db.persona.bi_export_dir)
                };
                match self.db.export_explore_pack() {
                    Ok(p) => self.set_status(
                        format!("BI export pack {} (configure bi_export_dir={})", p.display(), dir.display()),
                        false,
                    ),
                    Err(e) => self.set_status(format!("bi export: {e:#}"), true),
                }
            }
            "model_registry_link" => match ide_db::model_registry_link(&self.db.persona) {
                Ok(url) => {
                    let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
                    self.set_status(url, false);
                }
                Err(e) => self.set_status(format!("registry: {e:#}"), true),
            },
            "codegen_rust" => match self.db.codegen_from_structure("rust") {
                Ok(code) => {
                    self.db.show_text_result("codegen", &code);
                    self.ui.focus = Focus::Results;
                }
                Err(e) => self.set_status(format!("codegen: {e:#}"), true),
            },
            "codegen_ts" => match self.db.codegen_from_structure("ts") {
                Ok(code) => {
                    self.db.show_text_result("codegen", &code);
                    self.ui.focus = Focus::Results;
                }
                Err(e) => self.set_status(format!("codegen: {e:#}"), true),
            },
            "seed_fixtures" => {
                let dir = self.workspace.root.join("testdata");
                let mut n = 0usize;
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.extension().and_then(|s| s.to_str()) == Some("sql") {
                            if let Ok(sql) = std::fs::read_to_string(&p) {
                                self.db.sql = sql;
                                n += 1;
                            }
                        }
                    }
                }
                if n == 0 {
                    self.set_status(
                        format!("no *.sql in {} — add fixtures then re-run", dir.display()),
                        true,
                    );
                } else {
                    self.set_status(
                        format!("loaded last of {n} fixture(s) from testdata/ — F5 to apply"),
                        false,
                    );
                }
            }
            "golden_diff" => {
                let golden = self
                    .workspace
                    .root
                    .join("testdata")
                    .join("golden")
                    .join("latest.csv");
                match self.db.last_result.as_ref() {
                    Some(r) => match ide_db::golden_diff(r, &golden) {
                        Ok(msg) => {
                            self.db.show_text_result("golden", &msg);
                            self.ui.focus = Focus::Results;
                        }
                        Err(e) => self.set_status(format!("golden: {e:#}"), true),
                    },
                    None => self.set_status("run a query first", true),
                }
            }
            "fanout_assert" => {
                // Compare last result rowcount to itself as placeholder when multi-pin unavailable.
                let n = self
                    .db
                    .last_result
                    .as_ref()
                    .map(|r| r.rows.len())
                    .unwrap_or(0);
                let msg = ide_db::fanout_rowcount_assert(&[
                    ("active".into(), n),
                    ("pinned-check".into(), n),
                ]);
                self.db.show_text_result("fanout", &msg);
                self.ui.focus = Focus::Results;
            }
            "explain_cost" => {
                let text = self
                    .db
                    .last_explain
                    .as_deref()
                    .unwrap_or("(no EXPLAIN — Ctrl-E first)");
                let msg = ide_db::explain_cost_hint(text);
                self.db.show_text_result("cost", &msg);
                self.ui.focus = Focus::Results;
            }
            "schema_adr_stub" => {
                let note = format!(
                    "# Schema ADR stub\n\n## Context\nSchema change in workspace {}\n\n## Decision\nTBD\n\n## Consequences\nTBD\n",
                    self.workspace.root.display()
                );
                let path = self
                    .workspace
                    .root
                    .join("queries")
                    .join("schema-adr-stub.md");
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&path, &note);
                self.db.show_text_result("adr", &note);
                self.ui.focus = Focus::Results;
                self.set_status(
                    format!("ADR stub → {} (or brain decide)", path.display()),
                    false,
                );
            }
            "naming_hint" => {
                let driver = self
                    .db
                    .active_source
                    .as_ref()
                    .and_then(|id| self.db.sources.iter().find(|s| s.id == *id))
                    .map(|s| s.driver)
                    .unwrap_or_default();
                let name = self
                    .db
                    .last_result
                    .as_ref()
                    .and_then(|r| r.source_table.clone())
                    .unwrap_or_else(|| "CustomerOrder".into());
                let hint = ide_db::naming_hint(driver, &name);
                self.db.show_text_result("naming", &hint);
                self.ui.focus = Focus::Results;
            }
            "health_webhook" => {
                let url = self.db.persona.health_webhook_url.clone();
                match ide_db::post_health_webhook(&url, "db-studio health check fail test") {
                    Ok(()) => self.set_status("webhook POST ok", false),
                    Err(e) => self.set_status(format!("webhook: {e:#}"), true),
                }
            }
            "signed_conn_pack" => {
                let path = self
                    .workspace
                    .root
                    .join("queries")
                    .join("connections.signed.toml");
                match self.db.export_connection_pack(&path) {
                    Ok(()) => {
                        if let Ok(text) = std::fs::read_to_string(&path) {
                            let signed = path.with_extension("signed.toml");
                            match ide_db::signed_pack_export(&text, &signed) {
                                Ok(()) => {
                                    self.set_status(format!("signed pack → {}", signed.display()), false)
                                }
                                Err(e) => self.set_status(format!("sign: {e:#}"), true),
                            }
                        }
                    }
                    Err(e) => self.set_status(format!("pack: {e:#}"), true),
                }
            }
            "agents_ask_sql" => {
                if let Some(idx) = self
                    .agents
                    .profiles
                    .iter()
                    .position(|p| p.name.contains("Ask → SQL"))
                {
                    self.agents.selected = idx;
                }
                self.set_status(
                    "Agents: Ask → SQL selected — open Agents tab and launch (confirm before run)",
                    false,
                );
            }
            "code_help" => {
                if self.nvim_active() {
                    if let Some(nvim) = self.nvim.as_ref() {
                        match nvim.show_code_help().await {
                            Ok(()) => self.set_status("Code help (scratch buffer)", false),
                            Err(e) => self.set_status(format!("Code help: {e:#}"), true),
                        }
                    }
                } else {
                    let text = ide_nvim::NvimSession::code_help_text();
                    self.debug_log = text.to_string();
                    self.set_status(
                        "Code help → Debug log (attach Neovim for scratch buffer)",
                        false,
                    );
                }
            }
            "yank_clipboard" => self.yank_nvim_to_host_clipboard().await?,
            "paste_clipboard" => self.paste_host_clipboard_to_nvim().await?,
            "health_overview" => self.run_health("overview").await?,
            "health_activity" => self.run_health("activity").await?,
            "health_activity_auto_refresh" => {
                let on = self.db.toggle_health_activity_auto_refresh();
                self.health_activity_tick = 0;
                self.set_status(
                    if on {
                        "Health Activity auto-refresh ON (~5s)"
                    } else {
                        "Health Activity auto-refresh OFF"
                    },
                    false,
                );
            }
            "mssql_health_overview" => self.run_mssql_health("overview").await?,
            "mssql_health_activity" => self.run_mssql_health("activity").await?,
            "mssql_health_blocking" => self.run_mssql_health("blocking").await?,
            "mssql_health_waits" => self.run_mssql_health("waits").await?,
            "mssql_health_expensive" => self.run_mssql_health("expensive").await?,
            "mssql_health_io" => self.run_mssql_health("io").await?,
            "mssql_health_jobs" => self.run_mssql_health("jobs").await?,
            "mssql_health_indexes" => self.run_mssql_health("indexes").await?,
            "brain_ask" => self.start_brain_ask(),
            "result_page_next" => match self.db.next_result_page().await {
                Ok(()) => {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("page: {e:#}"), true),
            },
            "result_page_prev" => match self.db.prev_result_page().await {
                Ok(()) => {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("page: {e:#}"), true),
            },
            "save_result_temp" => self.start_save_result(true),
            "save_result_view" => self.start_save_result(false),
            "cancel_query" => {
                self.db.request_query_cancel();
                self.set_status("cancelling query…", false);
            }
            "import_csv" => self.start_import_csv(),
            "import_json" => self.start_import_json(),
            "dump_database" => self.dump_database().await?,
            "restore_dump" => self.start_restore_dump(),
            "schema_diff" => self.start_schema_diff(),
            "schema_diff_ddl" => self.generate_schema_diff_ddl().await?,
            "data_compare" => self.start_data_compare(),
            "txn_begin" => match self.db.begin_transaction().await {
                Ok(()) => self.set_status(self.db.status.clone(), false),
                Err(e) => self.set_status(format!("begin txn: {e:#}"), true),
            },
            "txn_commit" => match self.db.commit_transaction().await {
                Ok(()) => self.set_status(self.db.status.clone(), false),
                Err(e) => self.set_status(format!("commit txn: {e:#}"), true),
            },
            "txn_rollback" => match self.db.rollback_transaction().await {
                Ok(()) => self.set_status(self.db.status.clone(), false),
                Err(e) => self.set_status(format!("rollback txn: {e:#}"), true),
            },
            "kill_session" => self.kill_session().await?,
            "bookmark_sql" => {
                self.db.bookmark_current_sql();
                self.set_status(self.db.status.clone(), false);
            }
            "save_query" => match self.db.save_query_file(None, &self.db.sql.clone()) {
                Ok(p) => self.set_status(format!("saved {}", p.display()), false),
                Err(e) => self.set_status(format!("save: {e:#}"), true),
            },
            "sync_queries_push" => match self.db.sync_queries_push() {
                Ok(msg) => self.set_status(msg, false),
                Err(e) => self.set_status(format!("sync push: {e:#}"), true),
            },
            "sync_queries_pull" => match self.db.sync_queries_pull() {
                Ok(msg) => self.set_status(msg, false),
                Err(e) => self.set_status(format!("sync pull: {e:#}"), true),
            },
            "settings" => self.open_settings(),
            id if id.starts_with("snippet:") => {
                let name = id.trim_start_matches("snippet:");
                if let Some((_, body)) = ide_db::list_snippets()
                    .into_iter()
                    .find(|(n, _)| n == name)
                {
                    if self.db.sql.trim().is_empty() {
                        self.db.sql = body;
                    } else {
                        self.db.sql.push('\n');
                        self.db.sql.push_str(&body);
                    }
                    self.ui.focus = Focus::Editor;
                    self.set_status(format!("snippet · {name}"), false);
                } else {
                    self.set_status(format!("snippet not found · {name}"), true);
                }
            }
            id if id.starts_with("recent:") => {
                let key = id.trim_start_matches("recent:");
                if self.db.jump_recent_key(key) {
                    self.set_status(format!("jumped · {}", key.replace('|', " · ")), false);
                } else {
                    self.set_status(
                        format!("not in explorer · {}", key.replace('|', " · ")),
                        true,
                    );
                }
            }
            id if id.starts_with("hist:") => {
                let idx_str = id.trim_start_matches("hist:");
                match idx_str.parse::<usize>() {
                    Ok(idx) => match self.db.open_history_entry(idx) {
                        Ok(()) => {
                            self.ui.focus = Focus::Editor;
                            self.set_status(self.db.status.clone(), false);
                        }
                        Err(e) => self.set_status(format!("history: {e:#}"), true),
                    },
                    Err(_) => self.set_status("bad history id", true),
                }
            }
            id if id.starts_with("conn:") => {
                let cid = id.trim_start_matches("conn:");
                if let Some(idx) = self.db.flat.iter().position(|e| {
                    matches!(e.kind, DbNodeKind::Source) && e.source_id == cid
                }) {
                    self.db.selected = idx;
                    self.db.active_source = Some(cid.to_string());
                    self.ui.focus = Focus::Database;
                    let name = self
                        .db
                        .sources
                        .iter()
                        .find(|s| s.id == cid)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| cid.to_string());
                    self.set_status(format!("connection · {name}"), false);
                } else {
                    self.db.active_source = Some(cid.to_string());
                    self.set_status("connection selected (expand Explorer)", false);
                }
            }
            id if id.starts_with("plugin:") => {
                let cmd = id.trim_start_matches("plugin:");
                self.run_plugin_command(cmd).await?;
            }
            "add_datasource" => self.start_add_connection(),
            "edit_connection" => {
                if let Some(id) = self.selected_connection_id() {
                    self.start_edit_connection(&id);
                } else {
                    self.set_status("select a connection in Explorer first", true);
                }
            }
            "duplicate_connection" => {
                if let Some(id) = self.selected_connection_id() {
                    match self.db.duplicate_source(&id) {
                        Ok(_) => self.set_status(self.db.status.clone(), false),
                        Err(e) => self.set_status(format!("duplicate: {e:#}"), true),
                    }
                } else {
                    self.set_status("select a connection in Explorer first", true);
                }
            }
            "delete_connection" => {
                if let Some(id) = self.selected_connection_id() {
                    self.start_delete_connection(&id);
                } else {
                    self.set_status("select a connection in Explorer first", true);
                }
            }
            "vacuum_analyze" => {
                if let Err(e) = self
                    .db
                    .run_postgres_maintenance(PostgresMaintenance::VacuumAnalyze)
                    .await
                {
                    self.set_status(format!("vacuum: {e:#}"), true);
                } else {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
            }
            "analyze_table" => {
                if let Err(e) = self
                    .db
                    .run_postgres_maintenance(PostgresMaintenance::Analyze)
                    .await
                {
                    self.set_status(format!("analyze: {e:#}"), true);
                } else {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
            }
            "reindex_table" => {
                if let Err(e) = self
                    .db
                    .run_postgres_maintenance(PostgresMaintenance::Reindex)
                    .await
                {
                    self.set_status(format!("reindex: {e:#}"), true);
                } else {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
            }
            "inspect_privileges" => match self.db.inspect_table_privileges().await {
                Ok(()) => {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("privileges: {e:#}"), true),
            },
            "inspect_policies" => match self.db.inspect_rls_policies().await {
                Ok(()) => {
                    self.ui.focus = Focus::Results;
                    self.set_status(self.db.status.clone(), false);
                }
                Err(e) => self.set_status(format!("policies: {e:#}"), true),
            },
            "timing_histogram" => self.show_timing_histogram(),
            "db_object_search" => self.open_db_object_palette(),
            "add_brain" => {
                self.db.add_source(default_brain_source());
                self.enter_database_mode();
                self.set_status("added brain data source", false);
            }
            "git" => {
                self.ui.sidebar = Sidebar::Git;
                self.ui.focus = Focus::Git;
                self.ui.show_sidebar = true;
            }
            "outline" => {
                self.ui.sidebar = Sidebar::Outline;
                self.ui.focus = Focus::Outline;
                self.refresh_lsp_extras().await;
            }
            "terminal" => {
                self.ui.bottom = BottomPanel::Terminal;
                self.ui.focus = Focus::Terminal;
                self.ensure_terminal(80, 8)?;
                self.set_status("terminal · Ctrl-` toggle", false);
            }
            "sessions" => {
                self.ui.product = ProductMode::Code;
                self.ui.code_surface = CodeSurface::Editor;
                self.last_code_surface = CodeSurface::Editor;
                self.ui.mode = Mode::Normal;
                self.ui.sidebar = Sidebar::Sessions;
                self.ui.focus = Focus::Sessions;
                self.ui.show_sidebar = true;
                let _ = self.sessions.reload();
                self.sessions.reap_rdp();
                self.set_status("Sessions · Enter connect · a/A add", false);
            }
            "agents" => {
                self.ui.product = ProductMode::Code;
                self.ui.code_surface = CodeSurface::Editor;
                self.last_code_surface = CodeSurface::Editor;
                self.ui.mode = Mode::Normal;
                self.ui.sidebar = Sidebar::Agents;
                self.ui.focus = Focus::Agents;
                self.ui.show_sidebar = true;
                self.ui.bottom = BottomPanel::Terminal;
                let _ = self.agents.reload();
                self.set_status("Agents workspace · Enter launch", false);
            }
            "debug" => {
                self.ui.sidebar = Sidebar::Debug;
                self.ui.bottom = BottomPanel::Debug;
                self.ui.focus = Focus::Debug;
                self.ensure_dap().await?;
            }
            "split" => {
                let next = if self.buffers.len() > 1 {
                    (self.buffers.active_index() + 1) % self.buffers.len()
                } else {
                    self.buffers.active_index()
                };
                self.ui.split.secondary = Some(next);
                self.ui.split.enabled = !self.ui.split.enabled;
            }
            "close_tab" => {
                if let Err(e) = self.buffers.close_active() {
                    self.set_status(e.to_string(), true);
                }
            }
            "toggle_tree" => self.ui.show_sidebar = !self.ui.show_sidebar,
            "focus_editor" => self.ui.focus = Focus::Editor,
            "focus_tree" => {
                self.ui.focus = Focus::Tree;
                self.ui.show_sidebar = true;
            }
            "focus_diagnostics" => {
                self.ui.focus = Focus::Diagnostics;
                self.ui.bottom = BottomPanel::Problems;
            }
            "goto_def" => self.goto_definition().await,
            "hover" => self.hover().await,
            "completion" => self.request_completion().await,
            "next_problem" => {
                let n = self.diagnostics().len();
                if n > 0 {
                    self.ui.diag_selected = (self.ui.diag_selected + 1) % n;
                }
            }
            "refresh" => {
                if let Err(e) = self.workspace.refresh() {
                    self.set_status(e.to_string(), true);
                } else {
                    self.set_status("tree refreshed", false);
                }
            }
            "quit" => self.request_quit().await?,
            _ => {}
        }
        Ok(())
    }

    async fn jump_diag_async(&mut self) {
        let diags = self.diagnostics();
        let Some(d) = diags.get(self.ui.diag_selected).cloned() else {
            return;
        };
        if let Err(e) = self.open_path(d.path.clone()).await {
            self.set_status(e.to_string(), true);
            return;
        }
        if let Some(buf) = self.buffers.active_mut() {
            buf.cursor.row = d.line as usize;
            buf.cursor.col = d.character as usize;
            buf.clamp_cursor();
            buf.ensure_cursor_visible(30);
        }
        self.ui.focus = Focus::Editor;
    }

    fn forward_terminal_key(&self, key: KeyEvent) -> Result<()> {
        let term = if let Some(p) = self.agents.active_pane() {
            p
        } else if let Some(p) = self.sessions.active_ssh_pane() {
            p
        } else if let Some(t) = self.terminal.as_ref() {
            t
        } else {
            return Ok(());
        };
        match key.code {
            KeyCode::Enter => term.write_bytes(b"\r")?,
            KeyCode::Backspace => term.write_bytes(b"\x7f")?,
            KeyCode::Tab => term.write_bytes(b"\t")?,
            KeyCode::Esc => term.write_bytes(b"\x1b")?,
            KeyCode::Up => term.write_bytes(b"\x1b[A")?,
            KeyCode::Down => term.write_bytes(b"\x1b[B")?,
            KeyCode::Right => term.write_bytes(b"\x1b[C")?,
            KeyCode::Left => term.write_bytes(b"\x1b[D")?,
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                term.write_char(c)?;
            }
            _ => {}
        }
        Ok(())
    }
}

fn flatten_document_edits(edits: &[OneOf<TextEdit, AnnotatedTextEdit>]) -> Vec<TextEdit> {
    edits
        .iter()
        .map(|e| match e {
            OneOf::Left(te) => te.clone(),
            OneOf::Right(ate) => TextEdit {
                range: ate.text_edit.range,
                new_text: ate.text_edit.new_text.clone(),
            },
        })
        .collect()
}

#[allow(dead_code)]
fn parse_postgres_url(url: &str) -> Result<DataSource> {
    let url = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))
        .context("not a postgres URL")?;
    let (auth_host, database) = url.split_once('/').context("missing database")?;
    let database = database.split('?').next().unwrap_or("").to_string();
    let (user, hostport) = if let Some((u, rest)) = auth_host.split_once('@') {
        let user = u.split(':').next().unwrap_or("postgres").to_string();
        (user, rest)
    } else {
        ("postgres".into(), auth_host)
    };
    let (host, port) = if let Some((h, p)) = hostport.split_once(':') {
        (h.to_string(), p.parse().unwrap_or(5432))
    } else {
        (hostport.to_string(), 5432)
    };
    Ok(DataSource {
        id: uuid::Uuid::new_v4().to_string(),
        name: database.clone(),
        driver: DbDriver::Postgres,
        host,
        port,
        user,
        password: String::new(),
        database,
        file: String::new(),
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_port: 22,
        ssh_key: String::new(),
        ssl_mode: "prefer".into(),
        ssl_trust_cert: true,
        ssl_ca: String::new(),
        read_only: false,
        role: ide_db::SourceRole::Admin,
        group: String::new(),
        accent: String::new(),
        pinned: false,
    })
}

/// Best-effort layout sniff (cached). Prefers `setxkbmap`, then env.
fn keyboard_layout_is_spanish() -> bool {
    use std::sync::OnceLock;
    static SPANISH: OnceLock<bool> = OnceLock::new();
    *SPANISH.get_or_init(|| {
        if let Ok(out) = std::process::Command::new("setxkbmap")
            .arg("-query")
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("layout:") {
                    let layout = rest.trim();
                    if layout == "es"
                        || layout.starts_with("es,")
                        || layout.contains(",es")
                        || layout.starts_with("latam")
                    {
                        return true;
                    }
                    // Explicit non-Spanish layout from XKB wins over LANG.
                    return false;
                }
            }
        }
        for var in ["XKB_DEFAULT_LAYOUT", "LANG", "LC_ALL", "LC_CTYPE"] {
            if let Ok(v) = std::env::var(var) {
                let v = v.to_ascii_lowercase();
                if v.starts_with("es") || v.contains("es_") || v.starts_with("latam") {
                    return true;
                }
            }
        }
        false
    })
}

/// Unshifted base key → Shift glyph (Kitty REPORT_ALL sends base + SHIFT, not `%`).
fn shift_symbol(base: char, spanish: bool) -> Option<char> {
    if spanish {
        Some(match base {
            '1' => '!',
            '2' => '"',
            '3' => '·',
            '4' => '$',
            '5' => '%',
            '6' => '&',
            '7' => '/',
            '8' => '(',
            '9' => ')',
            '0' => '=',
            '\'' => '?',
            '¡' => '¿',
            // Shared / US-like punctuation still present on many ES boards:
            '-' => '_',
            '=' => '+',
            '[' => '{',
            ']' => '}',
            '\\' => '|',
            ';' => ':',
            ',' => ';',
            '.' => ':',
            '`' => '^',
            '<' => '>',
            // Latin America variants often keep these:
            '+' => '*',
            '{' => '[',
            '}' => ']',
            _ => return None,
        })
    } else {
        Some(match base {
            '1' => '!',
            '2' => '@',
            '3' => '#',
            '4' => '$',
            '5' => '%',
            '6' => '^',
            '7' => '&',
            '8' => '*',
            '9' => '(',
            '0' => ')',
            '-' => '_',
            '=' => '+',
            '[' => '{',
            ']' => '}',
            '\\' => '|',
            ';' => ':',
            '\'' => '"',
            ',' => '<',
            '.' => '>',
            '/' => '?',
            '`' => '~',
            _ => return None,
        })
    }
}

/// Fold SHIFT / Caps Lock into the character for Kitty `REPORT_ALL_KEYS_AS_ESCAPE_CODES`
/// (letters arrive as base glyph + SHIFT and/or `KeyEventState::CAPS_LOCK`; digits/punct
/// as unshifted base + SHIFT).
fn normalize_typed_key(mut key: KeyEvent) -> KeyEvent {
    if let KeyCode::Char(c) = key.code {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT)
            || key.modifiers.contains(KeyModifiers::SUPER)
        {
            return key;
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let caps = key.state.contains(KeyEventState::CAPS_LOCK);
        if c.is_ascii_alphabetic() {
            // Caps Lock XOR Shift (standard keyboards). Kitty reports base letter
            // (usually lowercase) + CAPS_LOCK state — without this, Caps Lock does nothing.
            // Legacy TTYs already deliver the final case with empty flags; leave those alone.
            if caps || shift {
                let want_upper = caps ^ shift;
                key.code = KeyCode::Char(if want_upper {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                });
                key.modifiers.remove(KeyModifiers::SHIFT);
            }
        } else if shift {
            if let Some(sym) = shift_symbol(c, keyboard_layout_is_spanish()) {
                key.code = KeyCode::Char(sym);
            }
            key.modifiers.remove(KeyModifiers::SHIFT);
        }
    }
    key
}

fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let s = uri.as_str();
    if let Some(rest) = s.strip_prefix("file://") {
        let path = if rest.starts_with('/') && rest.chars().nth(2) == Some(':') {
            // file:///C:/...
            rest.trim_start_matches('/').to_string()
        } else {
            rest.to_string()
        };
        return Some(PathBuf::from(path));
    }
    None
}

/// Map host terminal mouse → `nvim_input_mouse` (grid 0).
/// `chrome_top` = header [+ buffer tabs]; `chrome_left` = host file tree.
fn map_mouse_to_nvim(
    me: MouseEvent,
    term_height: u16,
    chrome_top: u16,
    chrome_left: u16,
) -> Option<(&'static str, &'static str, String, u16, u16)> {
    let chrome_top = chrome_top.max(1);
    if me.row < chrome_top || me.row + 1 >= term_height {
        return None;
    }
    if me.column < chrome_left {
        return None;
    }
    let row = me.row.saturating_sub(chrome_top);
    let col = me.column.saturating_sub(chrome_left);
    let mut mod_s = String::new();
    if me.modifiers.contains(KeyModifiers::CONTROL) {
        mod_s.push('c');
    }
    if me.modifiers.contains(KeyModifiers::SHIFT) {
        mod_s.push('s');
    }
    if me.modifiers.contains(KeyModifiers::ALT) {
        mod_s.push('a');
    }
    let (button, action) = match me.kind {
        MouseEventKind::Down(MouseButton::Left) => ("left", "press"),
        MouseEventKind::Down(MouseButton::Right) => ("right", "press"),
        MouseEventKind::Down(MouseButton::Middle) => ("middle", "press"),
        MouseEventKind::Up(MouseButton::Left) => ("left", "release"),
        MouseEventKind::Up(MouseButton::Right) => ("right", "release"),
        MouseEventKind::Up(MouseButton::Middle) => ("middle", "release"),
        MouseEventKind::Drag(MouseButton::Left) => ("left", "drag"),
        MouseEventKind::Drag(MouseButton::Right) => ("right", "drag"),
        MouseEventKind::Drag(MouseButton::Middle) => ("middle", "drag"),
        MouseEventKind::Moved => ("left", "move"),
        MouseEventKind::ScrollUp => ("wheel", "up"),
        MouseEventKind::ScrollDown => ("wheel", "down"),
        MouseEventKind::ScrollLeft => ("wheel", "left"),
        MouseEventKind::ScrollRight => ("wheel", "right"),
    };
    Some((button, action, mod_s, row, col))
}

pub async fn run(root: PathBuf) -> Result<()> {
    let mut app = App::new(root)?;
    match GitState::discover(&app.workspace.root).await {
        Ok(git) => {
            app.git = Some(git);
            info!("git repo discovered");
        }
        Err(e) => warn!("git: {e:#}"),
    }
    // db-studio: entry Home (not Studio). Restore last source; soft health ping.
    app.ui.product = ProductMode::Database;
    app.ui.db_surface = DbSurface::Home;
    app.ui.focus = Focus::Database;
    app.ui.sidebar = Sidebar::Database;
    app.ui.show_sidebar = true;
    app.ui.show_outline = false;
    app.ui.mode = Mode::Normal;
    if let Some(id) = app.db.active_source.clone() {
        if let Some(i) = app.db.sources.iter().position(|s| s.id == id) {
            app.ui.home_selected = i.min(8);
        }
    }
    app.db.refresh_stale_health().await;
    // No boot toast — Home footer already shows keybinds (toast would hide them).

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    // Kitty/foot/wezterm: make Ctrl-Enter distinct from Enter (otherwise many TTYs
    // collapse it to Ctrl-J / plain Enter and Run never fires).
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        )
    );
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let _size = terminal.size()?;
    // db-studio: no Neovim embed / LSP IDE stack.

    let result = event_loop(&mut terminal, &mut app).await;

    if let Some(nvim) = app.nvim.take() {
        nvim.quit().await;
        drop(nvim);
    }

    disable_raw_mode()?;
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;
    result
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<()> {
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(33));

    loop {
        if app.should_quit {
            break;
        }

        let _ = app.lsp.poll_events();
        app.poll_dap().await;

        let size = terminal.size()?;
        let agents_workspace = app.ui.product == ProductMode::Code
            && app.ui.code_surface == CodeSurface::Editor
            && app.ui.sidebar == Sidebar::Agents;
        let sessions_workspace = app.ui.product == ProductMode::Code
            && app.ui.code_surface == CodeSurface::Editor
            && app.ui.sidebar == Sidebar::Sessions;
        let astro_fullscreen = app.nvim_active()
            && app.ui.product == ProductMode::Code
            && app.ui.code_surface == CodeSurface::Editor
            && !agents_workspace
            && !sessions_workspace;
        let term_rows = if agents_workspace {
            size.height.saturating_sub(4).max(12)
        } else {
            (size.height / 4).max(6).min(16) as u16
        };
        let view_h = size.height.saturating_sub(6) as usize;
        // Astro owns buffer tabs / product tabs / terminal — host: status only on Code.
        let code_bottom_h = if agents_workspace || astro_fullscreen {
            0
        } else if app.ui.product == ProductMode::Code
            && app.ui.code_surface == CodeSurface::Editor
            && matches!(app.ui.bottom, BottomPanel::Terminal | BottomPanel::Debug)
        {
            10u16
        } else {
            0
        };
        app.nvim_chrome_top = if astro_fullscreen { 0 } else { 1 };
        let outline_w = if !agents_workspace
            && !astro_fullscreen
            && app.nvim_active()
            && app.ui.show_sidebar
            && app.ui.show_outline
        {
            28u16
        } else {
            0
        };
        app.nvim_chrome_left = if agents_workspace || sessions_workspace {
            30
        } else if astro_fullscreen {
            0
        } else if app.nvim_active() && app.ui.show_sidebar {
            26
        } else {
            0
        };
        // Astro Code is full-bleed (no host status strip) — match Home calm.
        let host_status_h = if astro_fullscreen { 0u16 } else { 1u16 };
        let nvim_rows = size
            .height
            .saturating_sub(app.nvim_chrome_top + host_status_h + code_bottom_h)
            .max(5);
        let nvim_cols = size
            .width
            .saturating_sub(app.nvim_chrome_left + outline_w)
            .max(20);
        if app.ui.bottom == BottomPanel::Terminal && !agents_workspace {
            let _ = app.ensure_terminal(size.width.saturating_sub(2).max(10), term_rows);
        }

        if let Some(nvim) = app.nvim.as_mut() {
            let _ = nvim.resize(nvim_cols, nvim_rows).await;
            if app.pending_nvim_blank {
                let boot = if nvim.is_astronvim() {
                    // Full-bleed ide_home (like host Home). Neo-tree only after Files.
                    "silent! set shortmess+=I number relativenumber signcolumn=yes \
                     cursorline laststatus=0 showtabline=0 cmdheight=0 \
                     noshowmode noruler | silent! enew | \
                     lua vim.defer_fn(function() \
                       pcall(function() require('ide_home').open() end); \
                     end, 80)"
                } else {
                    "silent! set shortmess+=I number relativenumber signcolumn=yes \
                     cursorline laststatus=0 | silent! enew"
                };
                let _ = nvim.command(boot).await;
                app.pending_nvim_blank = false;
            }
        }
        // Buffer tabs ~1Hz; diagnostics ~0.5Hz; neo-tree product tabs ~6Hz.
        if app.nvim_active() {
            app.nvim_tab_tick = app.nvim_tab_tick.wrapping_add(1);
            if app.nvim_tab_tick % 5 == 0 {
                if let Some(nvim) = app.nvim.as_ref() {
                    if let Ok(Some(goto)) = nvim.take_tui_ide_goto().await {
                        let tab = match goto.as_str() {
                            "database" => Some(1u8),
                            "sessions" => Some(2u8),
                            "agents" => Some(3u8),
                            "code" | "files" | "git" => Some(0u8),
                            _ => None,
                        };
                        if let Some(n) = tab {
                            let _ = app.goto_product_tab(n).await;
                        }
                    }
                }
            }
            if app.nvim_tab_tick % 30 == 0 {
                if let Some(nvim) = app.nvim.as_ref() {
                    if let Ok(bufs) = nvim.list_buf_infos().await {
                        app.nvim_bufs = bufs;
                    }
                }
            }
            if app.nvim_tab_tick % 60 == 0 {
                if let Some(nvim) = app.nvim.as_ref() {
                    if let Ok(d) = nvim.diagnostic_counts().await {
                        app.nvim_diag = d;
                    }
                }
            }
        } else if !app.nvim_bufs.is_empty() {
            app.nvim_bufs.clear();
            app.nvim_diag = (0, 0);
        }

        if app.editing_db_sql() {
            app.db_ensure_cursor_visible(view_h.max(1));
        } else if !app.nvim_active() {
            if let Some(buf) = app.active_buffer_mut() {
                buf.ensure_cursor_visible(view_h.max(1));
            }
        }

        let diags = app.diagnostics();
        app.sessions.reap_rdp();
        if agents_workspace {
            let cols = size.width.saturating_sub(32).max(40);
            if !app.agents.live.is_empty() {
                app.agents.resize_active(cols, term_rows);
            }
        } else if app.ui.bottom == BottomPanel::Terminal {
            let cols = size.width.saturating_sub(2).max(10);
            if !app.agents.live.is_empty() {
                app.agents.resize_active(cols, term_rows);
            } else {
                app.sessions.resize_active_ssh(cols, term_rows);
            }
        }
        let (session_tab_labels, session_tab_active, terminal_tab_kind): (Vec<String>, usize, &str) =
            if !app.agents.live.is_empty() {
                (
                    app.agents
                        .live
                        .iter()
                        .map(|s| s.label.chars().take(18).collect::<String>())
                        .collect(),
                    app.agents.active,
                    "Agent",
                )
            } else {
                (
                    app.sessions
                        .live_ssh
                        .iter()
                        .map(|s| s.label.chars().take(18).collect::<String>())
                        .collect(),
                    app.sessions.active_ssh,
                    if app.sessions.live_ssh.is_empty() {
                        ""
                    } else {
                        "SSH"
                    },
                )
            };
        let terminal_lines = if let Some(pane) = app.agents.active_pane() {
            pane.visible_lines()
        } else if let Some(pane) = app.sessions.active_ssh_pane() {
            pane.visible_lines()
        } else {
            app.terminal
                .as_ref()
                .map(|t| t.visible_lines())
                .unwrap_or_default()
        };
        let debug_log = app.debug_log.as_str();
        let nvim_snap = app.nvim.as_ref().map(|n| n.snapshot());
        let nvim_status_label = {
            let mode = app
                .nvim
                .as_ref()
                .map(|n| n.mode())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "n".into());
            if app.nvim_active() {
                format!("{} · {}", app.nvim_label, mode)
            } else if app.nvim_fallback {
                "native fallback".into()
            } else {
                app.nvim_label.clone()
            }
        };
        let recent_projects = app.config.recent_projects.as_slice();
        let recent_files = app.config.recent_files.as_slice();
        let nvim_bufs = app.nvim_bufs.as_slice();
        let nvim_fallback = app.nvim_fallback;

        terminal.draw(|frame| {
            render(
                frame,
                RenderCtx {
                    ui: &app.ui,
                    buffers: &app.buffers,
                    workspace: &app.workspace,
                    highlighter: &app.highlighter,
                    diagnostics: &diags,
                    git: app.git.as_ref(),
                    search: Some(&app.search),
                    sessions: Some(&app.sessions),
                    agents: Some(&app.agents),
                    terminal_lines: Some(&terminal_lines),
                    session_tab_labels: &session_tab_labels,
                    session_tab_active,
                    terminal_tab_kind,
                    debug_log: Some(debug_log),
                    db: Some(&app.db),
                    nvim_grid: nvim_snap.as_ref(),
                    nvim_label: Some(nvim_status_label.as_str()),
                    nvim_bufs,
                    nvim_fallback,
                    nvim_diag: app.nvim_diag,
                    recent_projects,
                    recent_files,
                },
            );
        })?;

        if app.pending_db_query {
            tokio::select! {
                res = app.run_query_executed() => {
                    app.pending_db_query = false;
                    app.finish_query_ui();
                    if let Err(e) = res {
                        app.show_query_error(format!("{e:#}"));
                    }
                }
                maybe_ev = events.next() => {
                    if let Some(Ok(Event::Key(key))) = maybe_ev {
                        if key.kind == KeyEventKind::Press && key.code == KeyCode::Esc {
                            app.db.request_query_cancel();
                            app.set_status("cancelling query…", false);
                        }
                    }
                }
            }
            continue;
        }

        tokio::select! {
            _ = tick.tick() => {
                app.clear_expired_status();
                // ~ every 45s: mark stale + re-ping up to 3 sources (Wave 7).
                app.health_tick_counter = app.health_tick_counter.saturating_add(1);
                if app.health_tick_counter >= 1360 {
                    app.health_tick_counter = 0;
                    if app.ui.product == ProductMode::Database {
                        app.db.refresh_stale_health().await;
                    }
                }
                // ~ every 5s: re-run Health · Activity when auto-refresh is on (Wave 9).
                if app.db.health_activity_auto_refresh
                    && app.ui.product == ProductMode::Database
                    && !app.db.query_running
                    && !app.pending_db_query
                    && app.db.active_tab_is_health_activity()
                {
                    app.health_activity_tick = app.health_activity_tick.saturating_add(1);
                    if app.health_activity_tick >= 150 {
                        app.health_activity_tick = 0;
                        app.queue_db_query();
                    }
                } else {
                    app.health_activity_tick = 0;
                }
            }
            maybe_ev = events.next() => {
                let Some(Ok(ev)) = maybe_ev else { continue };
                match ev {
                    Event::Paste(text) => {
                        if app.ui.mode == Mode::ImportCsv
                            && app.ui.confirm_run_sql.as_deref() == Some("__paste_tsv__")
                        {
                            app.ui.import_csv_input.push_str(&text);
                        } else if app.ui.mode == Mode::EditCell {
                            app.ui.cell_edit_input.push_str(&text);
                        } else if app.ui.mode == Mode::EditTableFilter {
                            app.ui.table_filter_draft.push_str(&text);
                            app.refresh_table_filter_completions();
                        } else if app.editing_db_sql()
                            && matches!(app.ui.mode, Mode::Insert | Mode::Normal)
                        {
                            if app.ui.mode != Mode::Insert {
                                app.ui.mode = Mode::Insert;
                            }
                            app.db_insert_text(&text);
                            app.set_status(
                                format!("pasted {} chars", text.chars().count()),
                                false,
                            );
                        } else if app.ui.focus == Focus::Results
                            && app.ui.product == ProductMode::Database
                        {
                            if let Some(r) = app.db.last_result.as_mut() {
                                match r.paste_tsv_at_cursor(&text) {
                                    Ok(n) => {
                                        app.db.flush_active_tab();
                                        app.set_status(
                                            format!("pasted {n} row(s) · Ctrl-S apply"),
                                            false,
                                        );
                                    }
                                    Err(e) => app.set_status(format!("paste: {e:#}"), true),
                                }
                            }
                        } else if app.nvim_active() && app.ui.mode != Mode::Palette {
                            if let Some(nvim) = app.nvim.as_ref() {
                                if let Err(e) = nvim.paste_text(&text).await {
                                    app.set_status(format!("nvim paste: {e:#}"), true);
                                }
                            }
                        } else if matches!(
                            app.ui.mode,
                            Mode::Insert | Mode::BrainAsk | Mode::QueryParams | Mode::Find
                        ) {
                            // fall through: append into common text inputs when focused editor
                            if app.ui.mode == Mode::Insert {
                                if let Some(buf) = app.buffers.active_mut() {
                                    for ch in text.chars() {
                                        buf.insert_char(ch);
                                    }
                                }
                            } else {
                                app.ui.import_csv_input.push_str(&text);
                            }
                        }
                    }
                    Event::Mouse(me) => {
                        // Product ribbon click (row 0) — not on Home (no ribbon there).
                        if matches!(me.kind, MouseEventKind::Down(MouseButton::Left))
                            && me.row == 0
                            && !(app.ui.product == ProductMode::Code
                                && app.ui.code_surface == CodeSurface::Home)
                        {
                            if let Some(tab) = ide_ui::product_tab_at_col(me.column) {
                                app.handle_action(Action::ProductTab(tab), view_h, term_rows)
                                    .await?;
                                continue;
                            }
                        }
                        if app.nvim_active() && app.ui.mode != Mode::Palette {
                            if let Some((button, action, modifier, row, col)) = map_mouse_to_nvim(
                                me,
                                size.height,
                                app.nvim_chrome_top,
                                app.nvim_chrome_left,
                            ) {
                                if let Some(nvim) = app.nvim.as_ref() {
                                    let _ = nvim
                                        .input_mouse(button, action, &modifier, row, col)
                                        .await;
                                }
                            }
                        }
                    }
                    Event::Key(key) if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat => {
                        // Kitty/enhanced protocol reports base letter + SHIFT; fold to real glyph.
                        let key = normalize_typed_key(key);
                        // Query error modal — Enter/Esc dismiss (Console stays in Insert underneath).
                        if app.ui.query_error.is_some()
                            && !key.modifiers.contains(KeyModifiers::CONTROL)
                            && !key.modifiers.contains(KeyModifiers::SUPER)
                        {
                            match key.code {
                                KeyCode::Esc | KeyCode::Enter => {
                                    app.ui.query_error = None;
                                    app.ui.focus = Focus::Editor;
                                    app.ui.mode = Mode::Insert;
                                    app.set_status("Console · fix SQL · F5 run", false);
                                    continue;
                                }
                                _ => {
                                    // Swallow other keys until dismissed so focus isn't stolen.
                                    continue;
                                }
                            }
                        }
                        // Admin ops modal owns Esc/q/r/arrows while open.
                        if app.ui.admin_modal
                            && app.ui.mode == Mode::Normal
                            && !key.modifiers.contains(KeyModifiers::CONTROL)
                            && !key.modifiers.contains(KeyModifiers::SUPER)
                        {
                            match key.code {
                                KeyCode::Esc | KeyCode::Char('q') => {
                                    app.ui.admin_modal = false;
                                    app.ui.focus = Focus::Database;
                                    app.ui.status = None;
                                    continue;
                                }
                                KeyCode::Char('r') | KeyCode::Char('R') => {
                                    app.set_status("admin · refreshing…", false);
                                    app.run_query().await?;
                                    continue;
                                }
                                KeyCode::Up => {
                                    app.ui.admin_modal_row =
                                        app.ui.admin_modal_row.saturating_sub(1);
                                    continue;
                                }
                                KeyCode::Down => {
                                    let max = app
                                        .db
                                        .last_result
                                        .as_ref()
                                        .map(|r| r.rows.len().saturating_sub(1))
                                        .unwrap_or(0);
                                    if app.ui.admin_modal_row < max {
                                        app.ui.admin_modal_row += 1;
                                    }
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        // Inspector owns Esc/q while open — before FocusEditor / status / fullscreen.
                        if app.ui.cell_modal
                            && app.ui.mode == Mode::Normal
                            && !key.modifiers.contains(KeyModifiers::CONTROL)
                            && !key.modifiers.contains(KeyModifiers::SUPER)
                            && matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
                        {
                            if app.ui.cell_modal_drill {
                                app.ui.cell_modal_drill = false;
                                app.ui.cell_modal_tab = 0;
                            } else {
                                app.ui.cell_modal = false;
                                app.ui.cell_modal_drill = false;
                            }
                            app.ui.focus = Focus::Results;
                            app.ui.status = None;
                            continue;
                        }
                        // Host chords: Super preferred, Control fallback (WM often eats Super).
                        let modk = key.modifiers.contains(KeyModifiers::CONTROL)
                            || key.modifiers.contains(KeyModifiers::SUPER);
                        if ide_nvim::is_host_chord(key.code, key.modifiers) {
                            if modk
                                && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
                            {
                                app.request_quit().await?;
                                continue;
                            }
                            if modk
                                && !key.modifiers.contains(KeyModifiers::SHIFT)
                                && matches!(key.code, KeyCode::Char('b') | KeyCode::Char('B'))
                            {
                                app.handle_action(Action::ToggleTree, view_h, term_rows)
                                    .await?;
                                continue;
                            }
                            if modk
                                && !key.modifiers.contains(KeyModifiers::SHIFT)
                                && matches!(key.code, KeyCode::Char('`'))
                            {
                                app.handle_action(Action::OpenTerminal, view_h, term_rows)
                                    .await?;
                                continue;
                            }
                            if modk
                                && key.modifiers.contains(KeyModifiers::SHIFT)
                                && matches!(key.code, KeyCode::Char('d') | KeyCode::Char('D'))
                            {
                                // db-studio: Home ↔ Studio (not Code).
                                app.toggle_product_mode();
                                continue;
                            }
                            if modk
                                && key.modifiers.contains(KeyModifiers::SHIFT)
                                && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P'))
                            {
                                app.open_command_palette();
                                continue;
                            }
                            if modk
                                && key.modifiers.contains(KeyModifiers::SHIFT)
                                && matches!(key.code, KeyCode::Char('h') | KeyCode::Char('H'))
                            {
                                app.go_home();
                                continue;
                            }
                            if modk
                                && key.modifiers.contains(KeyModifiers::SHIFT)
                                && matches!(key.code, KeyCode::Char('f') | KeyCode::Char('F'))
                            {
                                app.handle_action(Action::OpenSearch, view_h, term_rows)
                                    .await?;
                                continue;
                            }
                            // Git / Sessions / Agents / Outline are not part of db-studio.
                            if modk
                                && key.modifiers.contains(KeyModifiers::SHIFT)
                                && matches!(
                                    key.code,
                                    KeyCode::Char('s')
                                        | KeyCode::Char('S')
                                        | KeyCode::Char('a')
                                        | KeyCode::Char('A')
                                        | KeyCode::Char('o')
                                        | KeyCode::Char('O')
                                        | KeyCode::Char('g')
                                        | KeyCode::Char('G')
                                )
                            {
                                app.set_status(
                                    "db-studio · Super-⇧H home · Super-⇧D studio · 1–4 panes",
                                    false,
                                );
                                continue;
                            }
                            if modk && !key.modifiers.contains(KeyModifiers::SHIFT) {
                                if let KeyCode::Char(d) = key.code {
                                    if matches!(d, '1' | '2' | '3' | '4') {
                                        let tab = d as u8 - b'1';
                                        app.handle_action(
                                            Action::ProductTab(tab),
                                            view_h,
                                            term_rows,
                                        )
                                        .await?;
                                        continue;
                                    }
                                }
                            }
                        }

                        // ConfirmQuit: host dialog owns keys
                        if app.ui.mode == Mode::ConfirmQuit {
                            let ctx = Keymap::context(app.ui.mode, app.ui.focus);
                            if let Some(action) = Keymap::map(ctx, key) {
                                app.handle_action(action, view_h, term_rows).await?;
                            }
                            continue;
                        }

                        // Host Alt-1…9: switch Neovim buffer tab (Wave 2)
                        if app.nvim_active()
                            && app.ui.mode != Mode::Palette
                            && key.modifiers.contains(KeyModifiers::ALT)
                            && !key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            if let KeyCode::Char(d) = key.code {
                                if d.is_ascii_digit() && d != '0' {
                                    let n = d as u8 - b'0';
                                    app.switch_nvim_buf_slot(n).await?;
                                    continue;
                                }
                            }
                        }

                        // Host chrome owns keys when focused (tree / terminal / search / git / debug)
                        if app.nvim_active()
                            && app.ui.mode == Mode::Normal
                            && matches!(
                                app.ui.focus,
                                Focus::Tree
                                    | Focus::Terminal
                                    | Focus::Search
                                    | Focus::Git
                                    | Focus::Debug
                                    | Focus::Outline
                                    | Focus::Sessions
                                    | Focus::Agents
                            )
                        {
                            // DB-style pane switch while chrome has focus.
                            if app.ui.product == ProductMode::Code
                                && app.ui.code_surface == CodeSurface::Editor
                                && !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT)
                            {
                                match key.code {
                                    KeyCode::Char('1') => {
                                        app.focus_code_pane(1).await?;
                                        continue;
                                    }
                                    KeyCode::Char('2') => {
                                        app.focus_code_pane(2).await?;
                                        continue;
                                    }
                                    KeyCode::Char('3') => {
                                        app.focus_code_pane(3).await?;
                                        continue;
                                    }
                                    KeyCode::Char('4') => {
                                        app.focus_code_pane(4).await?;
                                        continue;
                                    }
                                    _ => {}
                                }
                            }
                            let ctx = Keymap::context(app.ui.mode, app.ui.focus);
                            if let Some(action) = Keymap::map(ctx, key) {
                                app.handle_action(action, view_h, term_rows).await?;
                            } else if app.ui.focus == Focus::Terminal {
                                app.forward_terminal_key(key)?;
                            }
                            continue;
                        }

                        // Host modal modes while Neovim attached
                        if app.nvim_active()
                            && matches!(
                                app.ui.mode,
                                Mode::Palette
                                    | Mode::Search
                                    | Mode::Commit
                                    | Mode::Settings
                                    | Mode::OpenFolder
                                    | Mode::Command
                                    | Mode::SessionForm
                                    | Mode::AgentForm
                            )
                        {
                            // fall through to normal host keymap / char handling below
                        } else if app.nvim_active() {
                            // Data Studio parity: Esc / 1–4 switch panes from a file buffer.
                            if app.ui.focus == Focus::Editor
                                && !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT)
                            {
                                let mode = match app.nvim.as_ref() {
                                    Some(n) => n.mode_live().await,
                                    None => String::new(),
                                };
                                let typing = nvim_typing_mode(&mode);
                                let visual = nvim_visual_mode(&mode);
                                let normal = nvim_normal_mode(&mode);

                                match key.code {
                                    KeyCode::Esc if typing || visual => {
                                        if let Some(seq) =
                                            key_to_nvim(key.code, key.modifiers)
                                        {
                                            if let Some(nvim) = app.nvim.as_ref() {
                                                let _ = nvim.input(&seq).await;
                                            }
                                        }
                                        if typing {
                                            app.nvim_esc_armed = true;
                                            app.set_status(
                                                "Esc / 1–4 · Files · Lazygit · Editor",
                                                false,
                                            );
                                        }
                                        continue;
                                    }
                                    KeyCode::Esc if normal || app.nvim_esc_armed => {
                                        app.nvim_esc_armed = false;
                                        app.focus_code_pane(1).await?;
                                        continue;
                                    }
                                    KeyCode::Char(d @ '1'..='4')
                                        if normal || app.nvim_esc_armed =>
                                    {
                                        app.nvim_esc_armed = false;
                                        app.focus_code_pane(d as u8 - b'0').await?;
                                        continue;
                                    }
                                    _ => {
                                        app.nvim_esc_armed = false;
                                    }
                                }
                            }
                            // Neovim Code mode: forward keys
                            if let Some(seq) = key_to_nvim(key.code, key.modifiers) {
                                if let Some(nvim) = app.nvim.as_ref() {
                                    let _ = nvim.input(&seq).await;
                                }
                            }
                            continue;
                        }

                        // Code Home: Code · Database · Sessions (arrows + Enter)
                        if app.ui.product == ProductMode::Code
                            && app.ui.code_surface == CodeSurface::Home
                            && app.ui.mode == Mode::Normal
                            && !key.modifiers.contains(KeyModifiers::CONTROL)
                            && !key.modifiers.contains(KeyModifiers::SUPER)
                        {
                            match key.code {
                                KeyCode::Up => {
                                    app.ui.home_selected =
                                        app.ui.home_selected.saturating_sub(1);
                                    continue;
                                }
                                KeyCode::Down => {
                                    if app.ui.home_selected < 2 {
                                        app.ui.home_selected += 1;
                                    }
                                    continue;
                                }
                                KeyCode::Char('1') => {
                                    app.ui.home_selected = 0;
                                    app.handle_action(Action::EnterCodeEditor, view_h, term_rows)
                                        .await?;
                                    continue;
                                }
                                KeyCode::Char('2') => {
                                    app.ui.home_selected = 1;
                                    app.handle_action(Action::ProductTab(1), view_h, term_rows)
                                        .await?;
                                    continue;
                                }
                                KeyCode::Char('3') => {
                                    app.ui.home_selected = 2;
                                    app.handle_action(Action::OpenSessions, view_h, term_rows)
                                        .await?;
                                    continue;
                                }
                                KeyCode::Enter => {
                                    match app.ui.home_selected {
                                        1 => {
                                            app.handle_action(
                                                Action::ProductTab(1),
                                                view_h,
                                                term_rows,
                                            )
                                            .await?;
                                        }
                                        2 => {
                                            app.handle_action(
                                                Action::OpenSessions,
                                                view_h,
                                                term_rows,
                                            )
                                            .await?;
                                        }
                                        _ => {
                                            app.handle_action(
                                                Action::EnterCodeEditor,
                                                view_h,
                                                term_rows,
                                            )
                                            .await?;
                                        }
                                    }
                                    continue;
                                }
                                _ => {}
                            }
                        }

                        // Database Home: source list · Enter open · A add · W workspaces
                        if app.ui.product == ProductMode::Database
                            && app.ui.db_surface == DbSurface::Home
                            && app.ui.mode == Mode::Normal
                            && !key.modifiers.contains(KeyModifiers::CONTROL)
                            && !key.modifiers.contains(KeyModifiers::SUPER)
                        {
                            app.ui.home_pane = ide_ui::HomePane::Sources;
                            let n_sources = app.db.sources.len();
                            match key.code {
                                KeyCode::Char('?') => {
                                    app.ui.show_help = !app.ui.show_help;
                                    continue;
                                }
                                KeyCode::Esc if app.ui.show_help => {
                                    app.ui.show_help = false;
                                    continue;
                                }
                                KeyCode::Esc if app.ui.status.is_some() => {
                                    app.ui.status = None;
                                    continue;
                                }
                                KeyCode::Up | KeyCode::Left => {
                                    if n_sources > 0 {
                                        app.ui.home_selected =
                                            app.ui.home_selected.saturating_sub(1);
                                    }
                                    continue;
                                }
                                KeyCode::Down | KeyCode::Right => {
                                    if n_sources > 0 {
                                        let max = n_sources.saturating_sub(1);
                                        if app.ui.home_selected < max {
                                            app.ui.home_selected += 1;
                                        }
                                    }
                                    continue;
                                }
                                KeyCode::Enter => {
                                    if n_sources == 0 {
                                        app.start_add_connection();
                                    } else {
                                        app.home_enter_studio().await;
                                    }
                                    continue;
                                }
                                KeyCode::Char('a') | KeyCode::Char('A') => {
                                    app.start_add_connection();
                                    continue;
                                }
                                KeyCode::Char('w') | KeyCode::Char('W') => {
                                    app.open_db_workspaces_palette();
                                    continue;
                                }
                                _ => {}
                            }
                        }

                        // Database Studio: Tab panes; 1/2/3; F5/Ctrl-R/Ctrl-Enter run; Esc → Explorer
                        if app.ui.product == ProductMode::Database
                            && app.ui.db_surface == DbSurface::Studio
                            && matches!(app.ui.mode, Mode::Normal | Mode::Insert)
                        {
                            // Results inspector owns Enter/i/Tab/1-3/arrows while open (Esc handled above).
                            if app.ui.cell_modal
                                && app.ui.focus == Focus::Results
                                && app.ui.mode == Mode::Normal
                            {
                                match key.code {
                                    KeyCode::Enter => {
                                        if app.ui.cell_modal_drill {
                                            app.start_cell_modal_edit();
                                        } else if app.ui.cell_modal_tab == 0 {
                                            // Row → full Cell for the selected field.
                                            app.ui.cell_modal_drill = true;
                                            app.set_status(
                                                "cell · Enter/i edit · Esc → Row",
                                                false,
                                            );
                                        } else {
                                            // Plan / Chart / JSON — stay on tab.
                                            continue;
                                        }
                                        continue;
                                    }
                                    KeyCode::Char('i') => {
                                        if !app.ui.cell_modal_drill {
                                            app.ui.cell_modal_drill = true;
                                        }
                                        app.start_cell_modal_edit();
                                        continue;
                                    }
                                    KeyCode::Tab => {
                                        app.ui.cell_modal_drill = false;
                                        app.ui.cell_modal_tab =
                                            (app.ui.cell_modal_tab + 1) % 4;
                                        continue;
                                    }
                                    KeyCode::BackTab => {
                                        app.ui.cell_modal_drill = false;
                                        app.ui.cell_modal_tab = app
                                            .ui
                                            .cell_modal_tab
                                            .checked_sub(1)
                                            .unwrap_or(3);
                                        continue;
                                    }
                                    KeyCode::Char('1') => {
                                        app.ui.cell_modal_drill = false;
                                        app.ui.cell_modal_tab = 0;
                                        continue;
                                    }
                                    KeyCode::Char('2') => {
                                        app.ui.cell_modal_drill = false;
                                        app.ui.cell_modal_tab = 1;
                                        continue;
                                    }
                                    KeyCode::Char('3') => {
                                        app.ui.cell_modal_drill = false;
                                        app.ui.cell_modal_tab = 2;
                                        continue;
                                    }
                                    KeyCode::Char('4') => {
                                        app.ui.cell_modal_drill = false;
                                        app.ui.cell_modal_tab = 3;
                                        continue;
                                    }
                                    KeyCode::Up => {
                                        if let Some(r) = app.db.last_result.as_mut() {
                                            if app.ui.cell_modal_drill {
                                                r.move_cell(-1, 0);
                                            } else {
                                                r.move_cell(0, -1);
                                            }
                                        }
                                        continue;
                                    }
                                    KeyCode::Down => {
                                        if let Some(r) = app.db.last_result.as_mut() {
                                            if app.ui.cell_modal_drill {
                                                r.move_cell(1, 0);
                                            } else {
                                                r.move_cell(0, 1);
                                            }
                                        }
                                        continue;
                                    }
                                    KeyCode::Left => {
                                        if let Some(r) = app.db.last_result.as_mut() {
                                            r.move_cell(0, -1);
                                        }
                                        continue;
                                    }
                                    KeyCode::Right => {
                                        if let Some(r) = app.db.last_result.as_mut() {
                                            r.move_cell(0, 1);
                                        }
                                        continue;
                                    }
                                    KeyCode::Char('m') => {
                                        app.ui.db_monitor = !app.ui.db_monitor;
                                        continue;
                                    }
                                    _ => {}
                                }
                            }
                            if !app.ui.cell_modal
                                && app.ui.mode == Mode::Normal
                                && matches!(key.code, KeyCode::Char('m'))
                                && !key.modifiers.contains(KeyModifiers::CONTROL)
                                && app.ui.focus == Focus::Results
                            {
                                app.ui.db_monitor = !app.ui.db_monitor;
                                app.set_status(
                                    if app.ui.db_monitor {
                                        "monitor on · m close"
                                    } else {
                                        "monitor off"
                                    },
                                    false,
                                );
                                continue;
                            }
                            if app.ui.db_monitor
                                && app.ui.mode == Mode::Normal
                                && matches!(
                                    key.code,
                                    KeyCode::Esc | KeyCode::Char('m') | KeyCode::Char('q')
                                )
                            {
                                app.ui.db_monitor = false;
                                continue;
                            }
                            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL)
                                || key.modifiers.contains(KeyModifiers::SUPER);
                            if is_run_query_key(key) {
                                app.handle_action(Action::RunQuery, view_h, term_rows).await?;
                                continue;
                            }
                            let alt = key.modifiers.contains(KeyModifiers::ALT);
                            // Alt-L: cycle auto SELECT LIMIT (50 → 100 → 200 → 500 → off).
                            if alt
                                && !ctrl
                                && matches!(key.code, KeyCode::Char('l') | KeyCode::Char('L'))
                            {
                                app.handle_action(Action::CycleSelectLimit, view_h, term_rows)
                                    .await?;
                                continue;
                            }
                            // Console DB/schema: Normal uses bare b/s; Insert uses Alt-b / Alt-s
                            // so typing "select" / "schema" never opens the picker mid-edit.
                            if app.ui.focus == Focus::Editor
                                && !ctrl
                                && matches!(
                                    key.code,
                                    KeyCode::Char('b')
                                        | KeyCode::Char('B')
                                        | KeyCode::Char('s')
                                        | KeyCode::Char('S')
                                )
                            {
                                let from_insert_alt = app.ui.mode == Mode::Insert && alt;
                                let from_normal_bare = app.ui.mode == Mode::Normal && !alt;
                                if from_insert_alt || from_normal_bare {
                                    match key.code {
                                        KeyCode::Char('b') | KeyCode::Char('B') => {
                                            app.handle_action(
                                                Action::OpenConsoleDatabase,
                                                view_h,
                                                term_rows,
                                            )
                                            .await?;
                                        }
                                        _ => {
                                            app.handle_action(
                                                Action::OpenConsoleSchema,
                                                view_h,
                                                term_rows,
                                            )
                                            .await?;
                                        }
                                    }
                                    continue;
                                }
                            }
                            // SQL console tabs: Ctrl-Tab / [ ] / Alt-1..9 / t+ / x
                            if app.ui.mode == Mode::Normal
                                && matches!(
                                    app.ui.focus,
                                    Focus::Editor
                                        | Focus::Results
                                        | Focus::Database
                                        | Focus::Structure
                                )
                            {
                                let shift = key.modifiers.contains(KeyModifiers::SHIFT);

                                // Ctrl-Tab / Ctrl-Shift-Tab
                                if ctrl
                                    && matches!(
                                        key.code,
                                        KeyCode::Tab | KeyCode::BackTab
                                    )
                                {
                                    if matches!(key.code, KeyCode::BackTab) || shift {
                                        app.db.prev_tab();
                                    } else {
                                        app.db.next_tab();
                                    }
                                    app.ui.db_cursor = Cursor::default();
                                    app.ui.db_scroll_row = 0;
                                    let _ = app.db.refresh_structure_for_active_tab().await;
                                    app.set_status(
                                        format!(
                                            "tab {}/{} · {}",
                                            app.db.active_tab + 1,
                                            app.db.tabs.len(),
                                            app.db
                                                .tabs
                                                .get(app.db.active_tab)
                                                .map(|t| t.title.as_str())
                                                .unwrap_or("?")
                                        ),
                                        false,
                                    );
                                    continue;
                                }

                                // Alt-1 .. Alt-9 jump to tab
                                if alt {
                                    if let KeyCode::Char(d) = key.code {
                                        if d.is_ascii_digit() && d != '0' {
                                            let idx = (d as u8 - b'1') as usize;
                                            if idx < app.db.tabs.len() {
                                                app.db.select_tab(idx);
                                                app.ui.db_cursor = Cursor::default();
                                                app.ui.db_scroll_row = 0;
                                                let _ =
                                                    app.db.refresh_structure_for_active_tab().await;
                                                app.set_status(
                                                    format!("tab {} · {}", idx + 1, app.db.tabs[idx].title),
                                                    false,
                                                );
                                            }
                                            continue;
                                        }
                                    }
                                }

                                // In Explorer, bare `x` deletes the connection (confirm modal).
                                // Elsewhere `x` / Ctrl-W closes the SQL tab.
                                let close_tab = (!ctrl
                                    && app.ui.focus != Focus::Database
                                    && matches!(key.code, KeyCode::Char('x') | KeyCode::Char('X')))
                                    || (ctrl
                                        && matches!(
                                            key.code,
                                            KeyCode::Char('w') | KeyCode::Char('W')
                                        ));
                                if close_tab {
                                    if app.db.close_active_tab() {
                                        app.ui.db_cursor = Cursor::default();
                                        app.ui.db_scroll_row = 0;
                                        let _ = app.db.refresh_structure_for_active_tab().await;
                                        app.set_status(
                                            format!(
                                                "closed · now {}",
                                                app.db
                                                    .tabs
                                                    .get(app.db.active_tab)
                                                    .map(|t| t.title.as_str())
                                                    .unwrap_or("?")
                                            ),
                                            false,
                                        );
                                    } else {
                                        app.set_status(app.db.status.clone(), true);
                                    }
                                    continue;
                                }
                                if !ctrl
                                    && !alt
                                    && matches!(
                                        key.code,
                                        KeyCode::Char('t')
                                            | KeyCode::Char('T')
                                            | KeyCode::Char('+')
                                    )
                                {
                                    app.db.new_query_tab();
                                    app.ui.db_cursor = Cursor::default();
                                    app.ui.db_scroll_row = 0;
                                    app.ui.focus = Focus::Editor;
                                    app.set_status(app.db.status.clone(), false);
                                    continue;
                                }
                                if !ctrl
                                    && !alt
                                    && matches!(key.code, KeyCode::Char('[') | KeyCode::Char(']'))
                                {
                                    match key.code {
                                        KeyCode::Char(']') => app.db.next_tab(),
                                        _ => app.db.prev_tab(),
                                    }
                                    app.ui.db_cursor = Cursor::default();
                                    app.ui.db_scroll_row = 0;
                                    let _ = app.db.refresh_structure_for_active_tab().await;
                                    app.set_status(
                                        format!(
                                            "tab {}/{} · {}",
                                            app.db.active_tab + 1,
                                            app.db.tabs.len(),
                                            app.db
                                                .tabs
                                                .get(app.db.active_tab)
                                                .map(|t| t.title.as_str())
                                                .unwrap_or("?")
                                        ),
                                        false,
                                    );
                                    continue;
                                }

                            }
                            match key.code {
                                KeyCode::Tab
                                    if !ctrl && app.ui.mode == Mode::Normal =>
                                {
                                    app.cycle_db_pane();
                                    if app.ui.focus == Focus::Structure {
                                        let _ =
                                            app.db.refresh_structure_from_console_query().await;
                                    }
                                    continue;
                                }
                                KeyCode::Esc if app.ui.mode == Mode::Insert => {
                                    app.ui.mode = Mode::Normal;
                                    continue;
                                }
                                KeyCode::Esc
                                    if !ctrl && app.ui.mode == Mode::Normal =>
                                {
                                    if app.ui.show_help {
                                        app.ui.show_help = false;
                                        continue;
                                    }
                                    if app.ui.show_discover {
                                        app.ui.show_discover = false;
                                        continue;
                                    }
                                    if app.ui.db_fullscreen.is_some() {
                                        app.ui.db_fullscreen = None;
                                        app.set_status("fullscreen off", false);
                                        continue;
                                    }
                                    if app.db.query_running {
                                        app.db.request_query_cancel();
                                        app.set_status("cancelling query…", false);
                                        continue;
                                    }
                                    if app.ui.focus != Focus::Database {
                                        app.focus_db_explorer();
                                        app.set_status(
                                            "1 Explorer · Esc Home · Tab panes",
                                            false,
                                        );
                                        continue;
                                    }
                                    // Explorer focused → Home
                                    app.ui.busy_label = None;
                                    app.go_db_home();
                                    continue;
                                }
                                KeyCode::Char('?')
                                    if !ctrl && app.ui.mode == Mode::Normal =>
                                {
                                    app.ui.show_help = !app.ui.show_help;
                                    continue;
                                }
                                KeyCode::Char(']')
                                    if ctrl && app.ui.mode == Mode::Normal =>
                                {
                                    app.cycle_db_source(1).await;
                                    continue;
                                }
                                KeyCode::Char('[')
                                    if ctrl && app.ui.mode == Mode::Normal =>
                                {
                                    app.cycle_db_source(-1).await;
                                    continue;
                                }
                                // Resize the focused pane only (each "cajita" owns its divider).
                                KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
                                    if ctrl && app.ui.mode == Mode::Normal =>
                                {
                                    app.resize_focused_pane(key.code);
                                    continue;
                                }
                                KeyCode::Char('D')
                                    if !ctrl && app.ui.mode == Mode::Normal
                                        && app.ui.focus == Focus::Results =>
                                {
                                    app.ui.results_compact = !app.ui.results_compact;
                                    let label = if app.ui.results_compact {
                                        "compact"
                                    } else {
                                        "comfort"
                                    };
                                    app.set_status(format!("results density · {label}"), false);
                                    continue;
                                }
                                // Console Normal: Backspace/Delete always edit SQL (enter Insert).
                                KeyCode::Backspace | KeyCode::Delete
                                    if !ctrl
                                        && app.ui.mode == Mode::Normal
                                        && app.ui.focus == Focus::Editor =>
                                {
                                    app.ui.mode = Mode::Insert;
                                    match key.code {
                                        KeyCode::Backspace => app.db_backspace(),
                                        _ => app.db_delete_forward(),
                                    }
                                    app.db_ensure_cursor_visible(view_h);
                                    continue;
                                }
                                KeyCode::Delete
                                | KeyCode::Backspace
                                | KeyCode::Char('x')
                                | KeyCode::Char('X')
                                    if !ctrl
                                        && app.ui.mode == Mode::Normal
                                        && app.ui.focus == Focus::Database =>
                                {
                                    if let Some(id) = app.selected_connection_id() {
                                        app.start_delete_connection(&id);
                                    } else {
                                        app.set_status(
                                            "select a connection in Explorer (source row)",
                                            true,
                                        );
                                    }
                                    continue;
                                }
                                KeyCode::Char('f') | KeyCode::Char('F')
                                    if !ctrl && app.ui.mode == Mode::Normal =>
                                {
                                    app.toggle_db_fullscreen();
                                    continue;
                                }
                                KeyCode::Char('1') if !ctrl && app.ui.mode == Mode::Normal => {
                                    app.focus_db_pane(1);
                                    app.set_status("1 Explorer", false);
                                    continue;
                                }
                                KeyCode::Char('2') if !ctrl && app.ui.mode == Mode::Normal => {
                                    app.focus_db_pane(2);
                                    let _ = app.db.refresh_structure_from_console_query().await;
                                    app.set_status("2 Structure · from query / Explorer", false);
                                    continue;
                                }
                                KeyCode::Char('3') if !ctrl && app.ui.mode == Mode::Normal => {
                                    app.focus_db_pane(3);
                                    app.set_status("3 Console · i insert · F5 run", false);
                                    continue;
                                }
                                KeyCode::Char('4') if !ctrl && app.ui.mode == Mode::Normal => {
                                    app.focus_db_pane(4);
                                    app.set_status("4 Results · ←↑↓→ · i edit · D density", false);
                                    continue;
                                }
                                _ => {}
                            }
                        }

                        if app.ui.focus == Focus::Terminal && app.ui.mode == Mode::Normal {
                            let ctx = Keymap::context(app.ui.mode, app.ui.focus);
                            if let Some(action) = Keymap::map(ctx, key) {
                                app.handle_action(action, view_h, term_rows).await?;
                            } else {
                                app.forward_terminal_key(key)?;
                            }
                            continue;
                        }

                        let ctx = Keymap::context(app.ui.mode, app.ui.focus);
                        if let Some(action) = Keymap::map(ctx, key) {
                            if action == Action::InsertNewline && app.ui.show_completions {
                                app.handle_action(Action::AcceptCompletion, view_h, term_rows).await?;
                            } else {
                                app.handle_action(action, view_h, term_rows).await?;
                            }
                            continue;
                        }

                        match app.ui.mode {
                            Mode::Insert => {
                                if let KeyCode::Char(c) = key.code {
                                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                                        if app.editing_db_sql() {
                                            app.db_insert_char(c);
                                            app.db_ensure_cursor_visible(view_h);
                                            let line = app.db_line(app.ui.db_cursor.row);
                                            let col = app.ui.db_cursor.col;
                                            let want = c == '.'
                                                || c.is_ascii_alphanumeric()
                                                || c == '_'
                                                || (c == ' '
                                                    && ide_db::expecting_relation(&line, col));
                                            if want {
                                                app.request_db_completion().await;
                                            } else {
                                                app.ui.show_completions = false;
                                            }
                                        } else if let Some(buf) = app.active_buffer_mut() {
                                            buf.insert_char(c);
                                            buf.ensure_cursor_visible(view_h);
                                            if c == '.' {
                                                app.request_completion().await;
                                            } else {
                                                app.ui.show_completions = false;
                                            }
                                            app.sync_lsp_change().await;
                                            app.update_breadcrumbs();
                                        }
                                    }
                                }
                            }
                            Mode::Command => {
                                if let KeyCode::Char(c) = key.code {
                                    app.ui.command.push(c);
                                }
                            }
                            Mode::Rename => {
                                if let KeyCode::Char(c) = key.code {
                                    app.ui.rename_input.push(c);
                                }
                            }
                            Mode::Search => {
                                if let KeyCode::Char(c) = key.code {
                                    app.ui.search_input.push(c);
                                }
                            }
                            Mode::Find => {
                                if let KeyCode::Char(c) = key.code {
                                    app.ui.find_query.push(c);
                                }
                            }
                            Mode::EditTableFilter => {
                                if let KeyCode::Char(c) = key.code {
                                    app.ui.table_filter_draft.push(c);
                                    app.refresh_table_filter_completions();
                                }
                            }
                            Mode::Replace => {
                                if let KeyCode::Char(c) = key.code {
                                    app.ui.replace_query.push(c);
                                }
                            }
                            Mode::Commit => {
                                if let KeyCode::Char(c) = key.code {
                                    app.ui.commit_input.push(c);
                                }
                            }
                            Mode::DbConnect => {
                                if let KeyCode::Char(c) = key.code {
                                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                                        // Ctrl chords handled via keymap (e.g. Ctrl-T test).
                                    } else if c == ' '
                                        && app.ui.db_connect_form.is_cycle_field(
                                            app.ui.db_connect_form.focus,
                                        )
                                    {
                                        // Space = next option on dropdown fields only.
                                        app.handle_action(
                                            Action::DbConnectDriverNext,
                                            view_h,
                                            term_rows,
                                        )
                                        .await?;
                                    } else {
                                        app.ui.db_connect_form.push_char(c);
                                    }
                                }
                            }
                            Mode::CreateObject => {
                                if key.modifiers.contains(KeyModifiers::CONTROL) {
                                    // reserved
                                } else if let KeyCode::Char(c) = key.code {
                                    let form = &mut app.ui.create_object_form;
                                    let on_add = form.on_add_button();
                                    let on_flag = form.on_flag_like_field();
                                    let typing = form.active_mut().is_some();
                                    if form.kind == CreateObjectKind::Table
                                        && !typing
                                        && matches!(c, '+' | 'a' | 'A')
                                    {
                                        form.add_column();
                                    } else if on_add && matches!(c, '+' | 'a' | 'A' | ' ') {
                                        form.add_column();
                                    } else if form.kind == CreateObjectKind::Table
                                        && matches!(c, 'x' | 'X')
                                        && !typing
                                    {
                                        form.remove_column();
                                    } else if on_flag && (c == ' ' || c == 'y' || c == 'n') {
                                        if matches!(
                                            form.kind,
                                            CreateObjectKind::Table
                                                | CreateObjectKind::Schema
                                                | CreateObjectKind::View
                                                | CreateObjectKind::MaterializedView
                                        ) {
                                            form.toggle_flag();
                                        } else if c == 'y' || c == 'n' {
                                            form.set_bool_field(c == 'y');
                                        } else {
                                            form.toggle_flag();
                                        }
                                    } else if typing {
                                        form.push_char(c);
                                    }
                                } else if key.code == KeyCode::Delete {
                                    app.ui.create_object_form.remove_column();
                                }
                            }
                            Mode::OpenFolder => {
                                if let KeyCode::Char(c) = key.code {
                                    app.ui.open_folder_input.push(c);
                                }
                            }
                            Mode::ImportCsv
                            | Mode::RestoreDump
                            | Mode::SchemaDiff
                            | Mode::DataCompare
                            | Mode::QueryParams
                            | Mode::SaveMaterialize
                            | Mode::BrainAsk => {
                                if let KeyCode::Char(c) = key.code {
                                    app.ui.import_csv_input.push(c);
                                }
                            }
                            Mode::ConfirmDeleteSource | Mode::ConfirmQuit => {}
                            Mode::EditCell => {
                                if let KeyCode::Char(c) = key.code {
                                    app.ui.cell_edit_input.push(c);
                                }
                            }
                            Mode::Settings => {
                                if let KeyCode::Char(c) = key.code {
                                    app.ui.settings_draft.push(c);
                                }
                            }
                            Mode::SessionForm => {
                                if let KeyCode::Char(c) = key.code {
                                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                                        app.session_form_push_char(c);
                                    }
                                }
                            }
                            Mode::AgentForm => {
                                if let KeyCode::Char(c) = key.code {
                                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                                        app.agent_form_push_char(c);
                                    }
                                }
                            }
                            Mode::Palette => {
                                if let KeyCode::Char(c) = key.code {
                                    app.ui.palette.push_char(c);
                                    app.preview_palette_theme();
                                }
                            }
                            Mode::Normal => {}
                        }
                    }
                    Event::Resize(cols, rows) => {
                        if let Some(t) = app.terminal.as_mut() {
                            t.resize(cols, rows / 4);
                        }
                        if let Some(nvim) = app.nvim.as_mut() {
                            nvim.force_next_resize();
                            let status_h = if app.nvim_chrome_top == 0
                                && app.ui.sidebar != Sidebar::Agents
                                && app.ui.sidebar != Sidebar::Sessions
                            {
                                0u16
                            } else {
                                1u16
                            };
                            let _ = nvim
                                .resize(
                                    cols.saturating_sub(app.nvim_chrome_left).max(20),
                                    rows.saturating_sub(app.nvim_chrome_top + status_h)
                                        .max(5),
                                )
                                .await;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn nvim_typing_mode(mode: &str) -> bool {
    matches!(mode.chars().next(), Some('i' | 'R' | 'c' | 't' | 's')) || mode.starts_with("ni")
}

fn nvim_visual_mode(mode: &str) -> bool {
    mode.starts_with('v') || mode.starts_with('V') || mode.starts_with('\u{16}')
}

fn nvim_normal_mode(mode: &str) -> bool {
    mode.is_empty() || mode == "n" || (mode.starts_with('n') && !mode.starts_with("ni"))
}

/// Studio run chords. Plain terminals often emit Ctrl-J (LF) for Ctrl-Enter;
/// with kitty keyboard protocol we also get Enter+CONTROL / Char('\\n')+CONTROL.
/// Super is preferred; Control remains the reliable TTY fallback.
fn is_run_query_key(key: KeyEvent) -> bool {
    let modk = key.modifiers.contains(KeyModifiers::CONTROL)
        || key.modifiers.contains(KeyModifiers::SUPER);
    match key.code {
        KeyCode::F(5) => true,
        KeyCode::Enter if modk => true,
        KeyCode::Char('r' | 'R') if modk => true,
        KeyCode::Char('\n') if modk => true,
        // TTY alias only: Ctrl-Enter often arrives as Ctrl-J (not a vim nav bind).
        KeyCode::Char('j' | 'J') if modk => true,
        _ => false,
    }
}

fn expand_user_path(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if raw == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(raw)
}

#[cfg(test)]
mod typed_key_tests {
    use super::normalize_typed_key;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode, mods: KeyModifiers, state: KeyEventState) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state,
        }
    }

    fn char_of(ev: KeyEvent) -> char {
        match ev.code {
            KeyCode::Char(c) => c,
            other => panic!("expected Char, got {other:?}"),
        }
    }

    #[test]
    fn caps_lock_uppercases_letters() {
        let ev = normalize_typed_key(key(
            KeyCode::Char('s'),
            KeyModifiers::empty(),
            KeyEventState::CAPS_LOCK,
        ));
        assert_eq!(char_of(ev), 'S');
        assert!(!ev.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn shift_uppercases_letters() {
        let ev = normalize_typed_key(key(
            KeyCode::Char('a'),
            KeyModifiers::SHIFT,
            KeyEventState::empty(),
        ));
        assert_eq!(char_of(ev), 'A');
        assert!(!ev.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn caps_plus_shift_cancels_to_lowercase() {
        let ev = normalize_typed_key(key(
            KeyCode::Char('a'),
            KeyModifiers::SHIFT,
            KeyEventState::CAPS_LOCK,
        ));
        assert_eq!(char_of(ev), 'a');
    }

    #[test]
    fn legacy_uppercase_without_flags_kept() {
        let ev = normalize_typed_key(key(
            KeyCode::Char('A'),
            KeyModifiers::empty(),
            KeyEventState::empty(),
        ));
        assert_eq!(char_of(ev), 'A');
    }
}
