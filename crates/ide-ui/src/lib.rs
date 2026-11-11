//! TUI layouts, keymap, and command palette.

pub mod create_dialect;
mod create_object_form;
mod db_connect_form;
mod keymap;
mod palette;
pub mod theme;
mod widgets;

pub use create_object_form::{CreateObjectForm, CreateObjectKind};
pub use create_dialect::supports_kind as create_dialect_supports;
pub use db_connect_form::DbConnectForm;
pub use keymap::{Action, KeyContext, Keymap};
pub use palette::{Palette, PaletteMode};
// HomeRecentSlot exported via lib for app quick-jump.

use ide_buffer::BufferSet;
use ide_core::{
    BottomPanel, CodeSurface, Cursor, DbSurface, Focus, LanguageId, Mode, ProductMode, Sidebar,
    TableFilterField,
};
use ide_dap::{StackFrameView, VariableView};
use ide_db::{DbNodeKind, DbWorkspace, StructureLineKind};
use ide_fs::Workspace;
use ide_git::GitState;
use ide_lsp::{CompletionItemView, DiagnosticItem, InlayHintView, SymbolItem};
use ide_nvim::NvimGrid;
use ide_search::SearchState;
use ide_syntax::{HighlightKind, Highlighter};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    /// Quiet tip — no colored badge (pane focus, resize hints, …).
    Info,
    /// Real success — green OK badge (test/connect/health).
    Success,
    /// Failure — red ERR badge.
    Error,
}

#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub text: String,
    pub error: bool,
    pub tone: StatusTone,
    /// When reached, Home/Studio footer drops the toast and shows keybinds again.
    pub until: std::time::Instant,
}

#[derive(Debug, Clone, Default)]
pub struct SplitState {
    pub enabled: bool,
    pub secondary: Option<usize>,
    pub focus_secondary: bool,
}

pub struct UiState {
    pub product: ProductMode,
    /// Code mode: Home dashboard (ours) vs Neovim editor grid.
    pub code_surface: CodeSurface,
    /// Database mode: Home hub vs full studio.
    pub db_surface: DbSurface,
    pub mode: Mode,
    pub focus: Focus,
    pub sidebar: Sidebar,
    pub bottom: BottomPanel,
    pub show_sidebar: bool,
    /// Right outline pane (Astro-like Code chrome).
    pub show_outline: bool,
    pub command: String,
    pub rename_input: String,
    pub search_input: String,
    pub find_query: String,
    pub replace_query: String,
    pub commit_input: String,
    pub db_connect_input: String,
    pub db_connect_form: DbConnectForm,
    /// When set, connect form updates this source id instead of creating one.
    pub db_connect_edit_id: Option<String>,
    /// `n New` object create modal (not Console SQL stubs).
    pub create_object_form: CreateObjectForm,
    pub open_folder_input: String,
    pub import_csv_input: String,
    pub db_cursor: Cursor,
    pub db_scroll_row: usize,
    pub results_scroll: usize,
    /// When set, Studio shows only this pane (1–4 fullscreen).
    pub db_fullscreen: Option<Focus>,
    pub palette: Palette,
    pub status: Option<StatusMessage>,
    pub hover: Option<String>,
    pub completions: Vec<CompletionItemView>,
    pub completion_idx: usize,
    pub show_completions: bool,
    pub diag_selected: usize,
    pub outline: Vec<SymbolItem>,
    pub outline_selected: usize,
    pub inlays: Vec<InlayHintView>,
    pub show_inlays: bool,
    pub breadcrumbs: String,
    pub split: SplitState,
    pub pending_code_actions: Vec<(String, String)>,
    pub cell_edit_input: String,
    pub stack_frames: Vec<StackFrameView>,
    pub debug_frame_selected: usize,
    pub variables: Vec<VariableView>,
    pub settings_draft: String,
    /// After QueryParams confirm, run EXPLAIN instead of execute.
    pub query_params_for_explain: bool,
    /// Source id pending delete confirmation.
    pub confirm_delete_source_id: Option<String>,
    pub confirm_delete_label: String,
    /// SaveMaterialize prompt: temp table vs view.
    pub save_materialize_temp: bool,
    /// Results grid: compact (dense) vs comfortable (padded rows).
    pub results_compact: bool,
    /// Explorer pane width in columns (Studio). `[` `]` when Explorer focused.
    pub explorer_width: u16,
    /// Left stack (Explorer + Structure) total width.
    pub studio_left_width: u16,
    /// Results pane height percent (25–70).
    pub results_pct: u16,
    /// Keep first N result columns frozen while H-scrolling (0 = off).
    pub results_pinned_cols: usize,
    /// BrainAsk prompt is collecting an OAuth token instead of a RAG question.
    pub oauth_paste_pending: bool,
    /// ConfirmDeleteSource dialog is Apply UPDATEs (not delete connection).
    pub confirm_apply_pending: bool,
    /// ConfirmDeleteSource dialog will run this SQL (DROP/TRUNCATE).
    pub confirm_run_sql: Option<String>,
    /// BrainAsk / rename prompt collecting a SQL tab title.
    pub rename_tab_pending: bool,
    /// ConfirmQuit dialog message (Wave 3).
    pub confirm_quit_label: String,
    /// SessionForm field index: 0 name … 5 jump/gateway.
    pub session_form_field: usize,
    /// AgentForm field index: 0 name … 4 seed.
    pub agent_form_field: usize,
    /// Home hub selection: Code products (0..2) or DB source index (0..8).
    pub home_selected: usize,
    /// DB Home: Sources · Recents · Favorites.
    pub home_pane: HomePane,
    /// Selected index within recent history (newest-first, 0..5).
    pub home_recent_selected: usize,
    /// Selected index within pinned favorites.
    pub home_favorite_selected: usize,
    /// `?` keymap cheat overlay.
    pub show_help: bool,
    /// Transient chrome busy label (`connecting…` / `RUNNING…`).
    pub busy_label: Option<String>,
    /// Query failure modal body (Enter/Esc dismiss).
    pub query_error: Option<String>,
    /// Console Ctrl-A: entire SQL buffer selected (Backspace/Delete/type clears).
    pub db_sql_all_selected: bool,
    /// After Console DB/schema palette, restore Insert on Console.
    pub resume_console_insert: bool,
    /// EditTableFilter: which bar (WHERE vs ORDER BY).
    pub table_filter_field: TableFilterField,
    /// Draft text while editing a table filter bar.
    pub table_filter_draft: String,
}

/// Which column is active on DB Home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HomePane {
    #[default]
    Sources,
    Recents,
    Favorites,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            product: ProductMode::Database,
            code_surface: CodeSurface::Home,
            db_surface: DbSurface::Home,
            mode: Mode::Normal,
            focus: Focus::Database,
            sidebar: Sidebar::Database,
            bottom: BottomPanel::Problems,
            show_sidebar: true,
            show_outline: true,
            command: String::new(),
            rename_input: String::new(),
            search_input: String::new(),
            find_query: String::new(),
            replace_query: String::new(),
            commit_input: String::new(),
            db_connect_input: String::new(),
            db_connect_form: DbConnectForm::default(),
            db_connect_edit_id: None,
            create_object_form: CreateObjectForm::fresh(CreateObjectKind::Table, "public"),
            open_folder_input: String::new(),
            import_csv_input: String::new(),
            db_cursor: Cursor::default(),
            db_scroll_row: 0,
            results_scroll: 0,
            db_fullscreen: None,
            palette: Palette::default(),
            status: None,
            hover: None,
            completions: Vec::new(),
            completion_idx: 0,
            show_completions: false,
            diag_selected: 0,
            outline: Vec::new(),
            outline_selected: 0,
            inlays: Vec::new(),
            show_inlays: true,
            breadcrumbs: String::new(),
            split: SplitState::default(),
            pending_code_actions: Vec::new(),
            cell_edit_input: String::new(),
            stack_frames: Vec::new(),
            debug_frame_selected: 0,
            variables: Vec::new(),
            settings_draft: String::new(),
            query_params_for_explain: false,
            confirm_delete_source_id: None,
            confirm_delete_label: String::new(),
            save_materialize_temp: true,
            results_compact: true,
            explorer_width: 28,
            studio_left_width: 56,
            results_pct: 40,
            results_pinned_cols: 1,
            oauth_paste_pending: false,
            confirm_apply_pending: false,
            confirm_run_sql: None,
            rename_tab_pending: false,
            confirm_quit_label: String::new(),
            session_form_field: 0,
            agent_form_field: 0,
            home_selected: 0,
            home_pane: HomePane::Sources,
            home_recent_selected: 0,
            home_favorite_selected: 0,
            show_help: false,
            busy_label: None,
            query_error: None,
            db_sql_all_selected: false,
            resume_console_insert: false,
            table_filter_field: TableFilterField::Where,
            table_filter_draft: String::new(),
        }
    }
}

pub struct RenderCtx<'a> {
    pub ui: &'a UiState,
    pub buffers: &'a BufferSet,
    pub workspace: &'a Workspace,
    pub highlighter: &'a Highlighter,
    pub diagnostics: &'a [DiagnosticItem],
    pub git: Option<&'a GitState>,
    pub search: Option<&'a SearchState>,
    pub sessions: Option<&'a ide_sessions::SessionManager>,
    pub agents: Option<&'a ide_agents::AgentManager>,
    pub terminal_lines: Option<&'a [String]>,
    pub session_tab_labels: &'a [String],
    pub session_tab_active: usize,
    /// Bottom terminal tab strip prefix: "SSH", "Agent", or empty.
    pub terminal_tab_kind: &'a str,
    pub debug_log: Option<&'a str>,
    pub db: Option<&'a DbWorkspace>,
    /// When set in Code mode, the editor region is the Neovim UI grid (AstroNvim).
    pub nvim_grid: Option<&'a NvimGrid>,
    pub nvim_label: Option<&'a str>,
    /// Neovim listed buffers for host tab chrome (Wave 2).
    pub nvim_bufs: &'a [ide_nvim::NvimBufInfo],
    /// True when Neovim embed failed / unavailable — native fallback.
    pub nvim_fallback: bool,
    /// Neovim workspace diagnostic counts (errors, warnings).
    pub nvim_diag: (u64, u64),
    pub recent_projects: &'a [String],
    pub recent_files: &'a [String],
}

pub fn render(frame: &mut Frame, ctx: RenderCtx<'_>) {
    let size = frame.area();

    // Code mode: our Home dashboard fills the center (not NVIM intro).
    // Editor surface paints the Neovim grid.
    if ctx.ui.product == ProductMode::Code {
        match ctx.ui.code_surface {
            CodeSurface::Home => {
                // Full-bleed hub — no ribbon, no "home / AstroNvim" status strip.
                render_home_dashboard(frame, size, &ctx);
            }
            CodeSurface::Editor => {
                // Agents workspace: sidebar + full-height PTY (Cursor/Claude are full TUIs —
                // never squeeze them under Neovim).
                if ctx.ui.sidebar == Sidebar::Agents {
                    let root = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1),
                            Constraint::Min(5),
                            Constraint::Length(1),
                        ])
                        .split(size);
                    render_host_header(frame, root[0], &ctx, true);
                    let cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Length(30), Constraint::Min(24)])
                        .split(root[1]);
                    // Opaque fill so Neovim ghosts never bleed through.
                    frame.render_widget(
                        Block::default().style(Style::default().bg(Color::Black)),
                        cols[0],
                    );
                    render_agents(
                        frame,
                        cols[0],
                        ctx.agents,
                        ctx.ui.focus == Focus::Agents,
                    );
                    render_agents_main(frame, cols[1], &ctx);
                    render_nvim_host_status(
                        frame,
                        root[2],
                        ctx.nvim_label.unwrap_or("Neovim"),
                        ctx.nvim_diag,
                        ctx.git,
                    );
                } else if ctx.ui.sidebar == Sidebar::Sessions {
                    // Sessions host list + Neovim/term (product stays native).
                    let root = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1),
                            Constraint::Min(5),
                            Constraint::Length(1),
                        ])
                        .split(size);
                    render_host_header(frame, root[0], &ctx, true);
                    let cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Length(30), Constraint::Min(24)])
                        .split(root[1]);
                    frame.render_widget(
                        Block::default().style(Style::default().bg(Color::Black)),
                        cols[0],
                    );
                    render_sidebar(frame, cols[0], &ctx);
                    if let Some(grid) = ctx.nvim_grid {
                        render_nvim_grid(frame, cols[1], grid);
                    } else {
                        render_home_dashboard(frame, cols[1], &ctx);
                    }
                    render_nvim_host_status(
                        frame,
                        root[2],
                        ctx.nvim_label.unwrap_or("Neovim"),
                        ctx.nvim_diag,
                        ctx.git,
                    );
                } else if ctx.nvim_grid.is_some() {
                    // Full-bleed Astro — same calm as Home (no host ribbon/status dump).
                    if let Some(grid) = ctx.nvim_grid {
                        render_nvim_grid(frame, size, grid);
                    }
                } else {
                // Native fallback chrome (no Neovim): files | editor | outline
                let show_tabs = !ctx.nvim_bufs.is_empty();
                let show_bottom = matches!(
                    ctx.ui.bottom,
                    BottomPanel::Terminal | BottomPanel::Debug
                );
                let show_outline = ctx.ui.show_outline && ctx.ui.show_sidebar;
                let left_w = if ctx.ui.show_sidebar { 26u16 } else { 0 };
                let right_w = if show_outline { 28u16 } else { 0 };

                let editor_root = match (show_tabs, show_bottom) {
                    (true, true) => Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1), // product ribbon
                            Constraint::Length(1), // buffers + crumbs
                            Constraint::Min(3),
                            Constraint::Length(10),
                            Constraint::Length(1),
                        ])
                        .split(size),
                    (true, false) => Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1),
                            Constraint::Length(1),
                            Constraint::Min(3),
                            Constraint::Length(1),
                        ])
                        .split(size),
                    (false, true) => Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1),
                            Constraint::Min(3),
                            Constraint::Length(10),
                            Constraint::Length(1),
                        ])
                        .split(size),
                    (false, false) => Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1),
                            Constraint::Min(3),
                            Constraint::Length(1),
                        ])
                        .split(size),
                };
                render_host_header(frame, editor_root[0], &ctx, true);
                let (body, bottom_area, status) = if show_tabs && show_bottom {
                    render_nvim_buf_tabs(frame, editor_root[1], ctx.nvim_bufs, &ctx.ui.breadcrumbs);
                    (editor_root[2], Some(editor_root[3]), editor_root[4])
                } else if show_tabs {
                    render_nvim_buf_tabs(frame, editor_root[1], ctx.nvim_bufs, &ctx.ui.breadcrumbs);
                    (editor_root[2], None, editor_root[3])
                } else if show_bottom {
                    (editor_root[1], Some(editor_root[2]), editor_root[3])
                } else {
                    (editor_root[1], None, editor_root[2])
                };

                let mut constraints = Vec::new();
                if left_w > 0 {
                    constraints.push(Constraint::Length(left_w));
                }
                constraints.push(Constraint::Min(20));
                if right_w > 0 {
                    constraints.push(Constraint::Length(right_w));
                }
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints(constraints)
                    .split(body);
                let mut idx = 0usize;
                if left_w > 0 {
                    frame.render_widget(
                        Block::default().style(Style::default().bg(theme::surface())),
                        cols[idx],
                    );
                    render_sidebar(frame, cols[idx], &ctx);
                    idx += 1;
                }
                let editor_area = cols[idx];
                idx += 1;
                render_home_dashboard(frame, editor_area, &ctx);
                if right_w > 0 {
                    frame.render_widget(
                        Block::default().style(Style::default().bg(theme::surface())),
                        cols[idx],
                    );
                    render_outline(frame, cols[idx], ctx.ui);
                }

                if let Some(area) = bottom_area {
                    render_bottom(frame, area, &ctx);
                }
                render_nvim_host_status(
                    frame,
                    status,
                    ctx.nvim_label.unwrap_or("Neovim"),
                    ctx.nvim_diag,
                    ctx.git,
                );
                } // end else (native fallback / non-Astro)
            }
        }
        if ctx.ui.mode == Mode::Palette {
            palette::render_palette(frame, size, &ctx.ui.palette);
        }
        if ctx.ui.mode == Mode::OpenFolder {
            palette::render_open_folder(frame, size, &ctx.ui.open_folder_input);
        }
        if ctx.ui.mode == Mode::ImportCsv {
            match ctx.ui.confirm_run_sql.as_deref() {
                Some("__alter_add__") => {
                    palette::render_alter_column(frame, size, &ctx.ui.import_csv_input, "add")
                }
                Some("__alter_drop__") => {
                    palette::render_alter_column(frame, size, &ctx.ui.import_csv_input, "drop")
                }
                Some("__alter_rename__") => {
                    palette::render_alter_column(frame, size, &ctx.ui.import_csv_input, "rename")
                }
                _ => palette::render_import_csv(frame, size, &ctx.ui.import_csv_input),
            }
        }
        if ctx.ui.mode == Mode::RestoreDump {
            palette::render_restore_dump(frame, size, &ctx.ui.import_csv_input);
        }
        if ctx.ui.mode == Mode::SchemaDiff {
            palette::render_schema_diff(frame, size, &ctx.ui.import_csv_input);
        }
        if ctx.ui.mode == Mode::DataCompare {
            palette::render_data_compare(frame, size, &ctx.ui.import_csv_input);
        }
        if ctx.ui.mode == Mode::QueryParams {
            palette::render_query_params(frame, size, &ctx.ui.import_csv_input);
        }
        if ctx.ui.mode == Mode::SaveMaterialize {
            palette::render_save_materialize(
                frame,
                size,
                &ctx.ui.import_csv_input,
                ctx.ui.save_materialize_temp,
            );
        }
        if ctx.ui.mode == Mode::BrainAsk {
            palette::render_brain_ask(frame, size, &ctx.ui.import_csv_input);
        }
        if ctx.ui.mode == Mode::SessionForm {
            if let Some(s) = ctx.sessions {
                palette::render_session_form(frame, size, s, ctx.ui.session_form_field);
            }
        }
        if ctx.ui.mode == Mode::AgentForm {
            if let Some(a) = ctx.agents {
                palette::render_agent_form(frame, size, a, ctx.ui.agent_form_field);
            }
        }
        if ctx.ui.mode == Mode::ConfirmDeleteSource {
            if ctx.ui.confirm_apply_pending {
                palette::render_confirm_dialog(
                    frame,
                    size,
                    " apply updates ",
                    &ctx.ui.confirm_delete_label,
                    "apply",
                );
            } else {
                palette::render_confirm_delete(
                    frame,
                    size,
                    " delete connection ",
                    &ctx.ui.confirm_delete_label,
                );
            }
        }
        if ctx.ui.mode == Mode::ConfirmQuit {
            palette::render_confirm_dialog(
                frame,
                size,
                " unsaved buffers ",
                &ctx.ui.confirm_quit_label,
                "save & leave",
            );
        }
        return;
    }

    // Database: Home hub first, then full studio.
    if ctx.ui.product == ProductMode::Database {
        match ctx.ui.db_surface {
            DbSurface::Home => {
                let root = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Min(3),
                        Constraint::Length(1),
                    ])
                    .split(size);
                render_host_header(frame, root[0], &ctx, false);
                render_db_home(frame, root[1], &ctx);
                render_db_home_status(frame, root[2], &ctx);
            }
            DbSurface::Studio => {
                render_database_studio(frame, size, &ctx);
            }
        }
        if ctx.ui.mode == Mode::Palette {
            palette::render_palette(frame, size, &ctx.ui.palette);
        }
        // EditCell is inline in the Results grid — never steal the bottom status bar.
        if matches!(
            ctx.ui.mode,
            Mode::Command | Mode::Settings | Mode::Search | Mode::Find
        ) && ctx.ui.db_surface == DbSurface::Studio
            && !matches!(ctx.ui.mode, Mode::Settings)
        {
            let prompt_area = Rect {
                x: size.x,
                y: size.y.saturating_add(size.height.saturating_sub(1)),
                width: size.width,
                height: 1,
            };
            let (prefix, text) = match ctx.ui.mode {
                Mode::Command => (":", ctx.ui.command.as_str()),
                Mode::Search => ("search> ", ctx.ui.search_input.as_str()),
                Mode::Find => ("find> ", ctx.ui.find_query.as_str()),
                _ => ("", ""),
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(theme::warn())),
                    Span::styled(text, theme::style_text()),
                ]))
                .style(Style::default().bg(theme::surface())),
                prompt_area,
            );
        }
        if ctx.ui.mode == Mode::DbConnect {
            db_connect_form::render_db_connect_form(frame, size, &ctx.ui.db_connect_form);
        }
        if ctx.ui.mode == Mode::CreateObject {
            create_object_form::render_create_object_form(
                frame,
                size,
                &ctx.ui.create_object_form,
            );
        }
        if ctx.ui.mode == Mode::Settings {
            palette::render_settings_modal(frame, size, &ctx.ui.settings_draft);
        }
        if ctx.ui.mode == Mode::OpenFolder {
            palette::render_open_folder(frame, size, &ctx.ui.open_folder_input);
        }
        if ctx.ui.mode == Mode::ImportCsv {
            match ctx.ui.confirm_run_sql.as_deref() {
                Some("__alter_add__") => {
                    palette::render_alter_column(frame, size, &ctx.ui.import_csv_input, "add")
                }
                Some("__alter_drop__") => {
                    palette::render_alter_column(frame, size, &ctx.ui.import_csv_input, "drop")
                }
                Some("__alter_rename__") => {
                    palette::render_alter_column(frame, size, &ctx.ui.import_csv_input, "rename")
                }
                _ => palette::render_import_csv(frame, size, &ctx.ui.import_csv_input),
            }
        }
        if ctx.ui.mode == Mode::RestoreDump {
            palette::render_restore_dump(frame, size, &ctx.ui.import_csv_input);
        }
        if ctx.ui.mode == Mode::SchemaDiff {
            palette::render_schema_diff(frame, size, &ctx.ui.import_csv_input);
        }
        if ctx.ui.mode == Mode::DataCompare {
            palette::render_data_compare(frame, size, &ctx.ui.import_csv_input);
        }
        if ctx.ui.mode == Mode::QueryParams {
            palette::render_query_params(frame, size, &ctx.ui.import_csv_input);
        }
        if ctx.ui.mode == Mode::SaveMaterialize {
            palette::render_save_materialize(
                frame,
                size,
                &ctx.ui.import_csv_input,
                ctx.ui.save_materialize_temp,
            );
        }
        if ctx.ui.mode == Mode::BrainAsk {
            palette::render_brain_ask(frame, size, &ctx.ui.import_csv_input);
        }
        if ctx.ui.mode == Mode::SessionForm {
            if let Some(s) = ctx.sessions {
                palette::render_session_form(frame, size, s, ctx.ui.session_form_field);
            }
        }
        if ctx.ui.mode == Mode::AgentForm {
            if let Some(a) = ctx.agents {
                palette::render_agent_form(frame, size, a, ctx.ui.agent_form_field);
            }
        }
        if ctx.ui.mode == Mode::ConfirmDeleteSource {
            if ctx.ui.confirm_apply_pending {
                palette::render_confirm_dialog(
                    frame,
                    size,
                    " apply updates ",
                    &ctx.ui.confirm_delete_label,
                    "apply",
                );
            } else {
                palette::render_confirm_delete(
                    frame,
                    size,
                    " delete connection ",
                    &ctx.ui.confirm_delete_label,
                );
            }
        }
        if ctx.ui.mode == Mode::ConfirmQuit {
            palette::render_confirm_dialog(
                frame,
                size,
                " unsaved buffers ",
                &ctx.ui.confirm_quit_label,
                "save & leave",
            );
        }
        if ctx.ui.show_help {
            render_db_help_overlay(frame, size, ctx.ui.db_surface);
        }
        if ctx.ui.show_completions && !ctx.ui.completions.is_empty() {
            // Anchor popup to the Console (right) pane of Studio.
            let console = studio_console_area(size, ctx.ui);
            render_completion_popup(frame, console, ctx.ui, ctx.buffers, ctx.db);
        }
        if let Some(err) = ctx.ui.query_error.as_deref() {
            palette::render_error_dialog(frame, size, " query error ", err);
        }
        return;
    }

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // breadcrumbs
            Constraint::Length(1), // tabs
            Constraint::Min(3),
            Constraint::Length(1), // status
            Constraint::Length(if matches!(
                ctx.ui.mode,
                Mode::Command
                    | Mode::Rename
                    | Mode::Search
                    | Mode::Find
                    | Mode::Replace
                    | Mode::Commit
                    | Mode::DbConnect
                    | Mode::EditCell
                    | Mode::Settings
            ) {
                1
            } else {
                0
            }),
        ])
        .split(size);

    render_breadcrumbs(frame, root[0], ctx.ui);
    render_tabs(frame, root[1], ctx.buffers, &ctx.ui.split);

    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
        .split(root[2]);

    let main = if ctx.ui.show_sidebar {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(30), Constraint::Min(20)])
            .split(body[0])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(0), Constraint::Min(20)])
            .split(body[0])
    };

    if ctx.ui.show_sidebar {
        render_sidebar(frame, main[0], &ctx);
    }

    if ctx.ui.product == ProductMode::Database {
        render_sql_editor(frame, main[1], &ctx);
    } else if ctx.ui.split.enabled {
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(main[1]);
        render_editor(frame, halves[0], &ctx, false);
        render_editor(frame, halves[1], &ctx, true);
    } else {
        render_editor(frame, main[1], &ctx, false);
    }

    render_bottom(frame, body[1], &ctx);
    render_status(frame, root[3], &ctx);

    if matches!(
        ctx.ui.mode,
        Mode::Command
            | Mode::Rename
            | Mode::Search
            | Mode::Find
            | Mode::Replace
            | Mode::Commit
            | Mode::EditCell
            | Mode::Settings
    ) {
        let (prefix, text) = match ctx.ui.mode {
            Mode::Command => (":", ctx.ui.command.as_str()),
            Mode::Rename => ("rename> ", ctx.ui.rename_input.as_str()),
            Mode::Search => ("search> ", ctx.ui.search_input.as_str()),
            Mode::Find => ("find> ", ctx.ui.find_query.as_str()),
            Mode::Replace => ("replace> ", ctx.ui.replace_query.as_str()),
            Mode::Commit => ("commit> ", ctx.ui.commit_input.as_str()),
            Mode::EditCell => ("cell> ", ctx.ui.cell_edit_input.as_str()),
            Mode::Settings => ("settings> ", ctx.ui.settings_draft.as_str()),
            _ => ("", ""),
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(prefix, Style::default().fg(Color::Yellow)),
                Span::raw(text),
            ])),
            root[4],
        );
    }

    if ctx.ui.mode == Mode::Palette {
        palette::render_palette(frame, size, &ctx.ui.palette);
    }
    if ctx.ui.mode == Mode::DbConnect {
        db_connect_form::render_db_connect_form(frame, size, &ctx.ui.db_connect_form);
    }
    if ctx.ui.mode == Mode::CreateObject {
        create_object_form::render_create_object_form(frame, size, &ctx.ui.create_object_form);
    }
    if ctx.ui.mode == Mode::OpenFolder {
        palette::render_open_folder(frame, size, &ctx.ui.open_folder_input);
    }
    if ctx.ui.mode == Mode::ImportCsv {
        match ctx.ui.confirm_run_sql.as_deref() {
            Some("__alter_add__") => {
                palette::render_alter_column(frame, size, &ctx.ui.import_csv_input, "add")
            }
            Some("__alter_drop__") => {
                palette::render_alter_column(frame, size, &ctx.ui.import_csv_input, "drop")
            }
            Some("__alter_rename__") => {
                palette::render_alter_column(frame, size, &ctx.ui.import_csv_input, "rename")
            }
            _ => palette::render_import_csv(frame, size, &ctx.ui.import_csv_input),
        }
    }
    if ctx.ui.mode == Mode::RestoreDump {
        palette::render_restore_dump(frame, size, &ctx.ui.import_csv_input);
    }
    if ctx.ui.mode == Mode::SchemaDiff {
        palette::render_schema_diff(frame, size, &ctx.ui.import_csv_input);
    }
    if ctx.ui.mode == Mode::DataCompare {
        palette::render_data_compare(frame, size, &ctx.ui.import_csv_input);
    }
    if ctx.ui.mode == Mode::QueryParams {
        palette::render_query_params(frame, size, &ctx.ui.import_csv_input);
    }
    if ctx.ui.mode == Mode::SaveMaterialize {
        palette::render_save_materialize(
            frame,
            size,
            &ctx.ui.import_csv_input,
            ctx.ui.save_materialize_temp,
        );
    }
    if ctx.ui.mode == Mode::BrainAsk {
        palette::render_brain_ask(frame, size, &ctx.ui.import_csv_input);
    }
    if ctx.ui.mode == Mode::SessionForm {
        if let Some(s) = ctx.sessions {
            palette::render_session_form(frame, size, s, ctx.ui.session_form_field);
        }
    }
    if ctx.ui.mode == Mode::AgentForm {
        if let Some(a) = ctx.agents {
            palette::render_agent_form(frame, size, a, ctx.ui.agent_form_field);
        }
    }
    if ctx.ui.mode == Mode::ConfirmDeleteSource {
        if ctx.ui.confirm_apply_pending {
            palette::render_confirm_dialog(
                frame,
                size,
                " apply updates ",
                &ctx.ui.confirm_delete_label,
                "apply",
            );
        } else {
            palette::render_confirm_delete(
                frame,
                size,
                " delete connection ",
                &ctx.ui.confirm_delete_label,
            );
        }
    }
    if ctx.ui.mode == Mode::ConfirmQuit {
        palette::render_confirm_dialog(
            frame,
            size,
            " unsaved buffers ",
            &ctx.ui.confirm_quit_label,
            "save & leave",
        );
    }
    if ctx.ui.show_completions && !ctx.ui.completions.is_empty() {
        render_completion_popup(frame, main[1], ctx.ui, ctx.buffers, ctx.db);
    }
    if let Some(hover) = &ctx.ui.hover {
        render_hover(frame, main[1], hover);
    }
}

fn render_breadcrumbs(frame: &mut Frame, area: Rect, ui: &UiState) {
    let text = if ui.breadcrumbs.is_empty() {
        " tui-ide "
    } else {
        ui.breadcrumbs.as_str()
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(" {text} "),
            Style::default().fg(Color::DarkGray),
        )),
        area,
    );
}

fn render_tabs(frame: &mut Frame, area: Rect, buffers: &BufferSet, split: &SplitState) {
    let mut spans = Vec::new();
    for (i, buf) in buffers.iter().enumerate() {
        let mut title = buf.title();
        if buf.dirty {
            title.push('*');
        }
        let active = if split.enabled && split.focus_secondary {
            split.secondary == Some(i)
        } else {
            i == buffers.active_index()
        };
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(format!(" {title} "), style));
        spans.push(Span::raw(" "));
    }
    if spans.is_empty() {
        spans.push(Span::styled(
            " (no file) ",
            Style::default().fg(Color::DarkGray),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_sidebar(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    match ctx.ui.sidebar {
        Sidebar::Files => render_tree(frame, area, ctx.workspace, ctx.ui.focus == Focus::Tree),
        Sidebar::Git => render_git(frame, area, ctx.git, ctx.ui.focus == Focus::Git),
        Sidebar::Search => render_search(frame, area, ctx.search, ctx.ui.focus == Focus::Search),
        Sidebar::Outline => render_outline(frame, area, ctx.ui),
        Sidebar::Debug => render_debug_side(frame, area, ctx),
        Sidebar::Sessions => {
            render_sessions(frame, area, ctx.sessions, ctx.ui.focus == Focus::Sessions)
        }
        Sidebar::Agents => {
            render_agents(frame, area, ctx.agents, ctx.ui.focus == Focus::Agents)
        }
        Sidebar::Database => render_db_tree(frame, area, ctx.db, ctx.ui.focus == Focus::Database),
    }
}

/// Whether a later flat sibling exists at `depth` before climbing above it.
fn explorer_has_following_sibling(flat: &[ide_db::FlatDbEntry], idx: usize, depth: usize) -> bool {
    for e in flat.iter().skip(idx + 1) {
        if e.depth < depth {
            return false;
        }
        if e.depth == depth {
            return true;
        }
    }
    false
}

/// Box-drawing indent: `│  ├─ └─` (pack E). Width-1 only.
fn explorer_tree_prefix(flat: &[ide_db::FlatDbEntry], idx: usize) -> String {
    let depth = flat[idx].depth;
    if depth == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(depth * 3);
    for d in 0..depth {
        if d + 1 < depth {
            if explorer_has_following_sibling(flat, idx, d + 1) {
                out.push_str("│  ");
            } else {
                out.push_str("   ");
            }
        } else if explorer_has_following_sibling(flat, idx, depth) {
            out.push_str("├─ ");
        } else {
            out.push_str("└─ ");
        }
    }
    out
}

fn render_db_tree(frame: &mut Frame, area: Rect, db: Option<&DbWorkspace>, focused: bool) {
    let border = theme::border(focused);
    let inner_h = area.height.saturating_sub(2) as usize;
    let scroll = db.map(|ws| ws.explorer_scroll).unwrap_or(0);
    let items: Vec<ListItem> = match db {
        Some(ws) if !ws.flat.is_empty() => ws
            .flat
            .iter()
            .enumerate()
            .skip(scroll)
            .take(inner_h.max(1))
            .map(|(i, e)| {
                // Pack E only: box-drawing tree + label. No kind/health glyphs.
                let tree = explorer_tree_prefix(&ws.flat, i);
                let selected = i == ws.selected;
                let row_style = if selected {
                    theme::style_selected()
                } else {
                    Style::default()
                };
                let label_style = if selected {
                    theme::style_selected()
                } else {
                    Style::default().fg(theme::text())
                };
                let inner_w = area.width.saturating_sub(2) as usize;
                let used = tree.chars().count();
                let label_max = inner_w.saturating_sub(used).max(4);
                let role = if matches!(e.kind, DbNodeKind::Source) {
                    ws.sources
                        .iter()
                        .find(|s| s.id == e.source_id)
                        .map(|s| s.role.label())
                } else {
                    None
                };
                // Drop whole tokens (driver, then role) before ellipsizing — never "[ad…]".
                let mut label = if matches!(e.kind, DbNodeKind::Source) {
                    fit_explorer_source_label(&e.label, role, label_max)
                } else {
                    truncate_chars(&e.label, label_max)
                };
                let pad = inner_w.saturating_sub(used + label.chars().count());
                if pad > 0 {
                    label.push_str(&" ".repeat(pad));
                }
                ListItem::new(Line::from(vec![
                    Span::styled(
                        tree,
                        if selected {
                            row_style
                        } else {
                            theme::style_faint()
                        },
                    ),
                    Span::styled(label, label_style),
                ]))
            })
            .collect(),
        Some(ws) => {
            let loading = ws.status.to_lowercase().contains("load")
                || ws.status.to_lowercase().contains("fetch")
                || ws.status.to_lowercase().contains("expand");
            let mut items = vec![ListItem::new(Span::styled(
                format!(" {} ", ws.status),
                theme::style_muted(),
            ))];
            if loading {
                items.push(ListItem::new(Span::styled(
                    " loading schemas… ",
                    theme::style_accent(),
                )));
            } else {
                items.push(ListItem::new(Span::styled(
                    " expand a source · Enter on a table ",
                    theme::style_faint(),
                )));
                items.push(ListItem::new(Span::styled(
                    " empty schema? check grants / database field ",
                    theme::style_faint(),
                )));
            }
            items
        }
        None => vec![ListItem::new(Span::styled(
            " no sources — press a to connect ",
            theme::style_faint(),
        ))],
    };
    let title = if focused { " Explorer " } else { " explorer " };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(border))
                .style(Style::default().bg(theme::bg())),
        ),
        area,
    );
}

fn render_db_structure(frame: &mut Frame, area: Rect, db: Option<&DbWorkspace>, focused: bool) {
    let border = theme::border(focused);
    // Gray properties strip (requested franja) — keyed off Explorer selection.
    let strip = db.and_then(|w| w.selection_properties_strip());
    let (list_area, strip_area) = if strip.is_some() && area.height >= 5 {
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(1)])
            .split(area);
        (parts[0], Some(parts[1]))
    } else {
        (area, None)
    };
    let area = list_area;
    let inner_h = area.height.saturating_sub(2) as usize;
    let (title, items) = match db {
        Some(ws) if !ws.structure_lines.is_empty() => {
            let title_budget = area.width.saturating_sub(14).max(6) as usize;
            let fitted = fit_dot_title(&ws.structure_title, title_budget);
            let title = if focused {
                if fitted.is_empty() {
                    " Structure ".into()
                } else {
                    format!(" Structure · {fitted} ")
                }
            } else if fitted.is_empty() {
                " structure ".into()
            } else {
                format!(" structure · {fitted} ")
            };
            let scroll = ws.structure_scroll.min(ws.structure_lines.len().saturating_sub(1));
            let inner_w = area.width.saturating_sub(2) as usize;
            let items: Vec<ListItem> = ws
                .structure_lines
                .iter()
                .skip(scroll)
                .take(inner_h.max(1))
                .map(|line| {
                    let (prefix, style) = match line.kind {
                        StructureLineKind::Section => (
                            "",
                            theme::style_text().add_modifier(Modifier::BOLD),
                        ),
                        StructureLineKind::Item => ("  ", theme::style_text()),
                        StructureLineKind::Ddl => ("  ", theme::style_muted()),
                    };
                    let max = inner_w.saturating_sub(prefix.chars().count()).max(4);
                    let mut text = format!("{prefix}{}", truncate_chars(&line.text, max));
                    let pad = inner_w.saturating_sub(text.chars().count());
                    if pad > 0 {
                        text.push_str(&" ".repeat(pad));
                    }
                    ListItem::new(Line::from(Span::styled(text, style)))
                })
                .collect();
            (title, items)
        }
        Some(ws) => {
            let title = if focused {
                " Structure ".into()
            } else {
                " structure ".into()
            };
            let dead_msg = ws.flat.get(ws.selected).and_then(|e| {
                if matches!(e.kind, DbNodeKind::Source) {
                    ws.connection_health
                        .get(&e.source_id)
                        .and_then(|h| h.error_message())
                        .map(|m| m.to_string())
                } else {
                    None
                }
            });
            let inner_w = area.width.saturating_sub(2) as usize;
            let items = if let Some(msg) = dead_msg {
                let short = if msg.chars().count() > inner_w.saturating_sub(2).max(12) {
                    format!(
                        "{}…",
                        msg.chars()
                            .take(inner_w.saturating_sub(3).max(8))
                            .collect::<String>()
                    )
                } else {
                    msg
                };
                vec![
                    ListItem::new(Span::styled(
                        truncate_chars(" connection failed ", inner_w),
                        theme::style_text().add_modifier(Modifier::BOLD),
                    )),
                    ListItem::new(Span::styled(
                        truncate_chars(&format!(" {short} "), inner_w),
                        theme::style_muted(),
                    )),
                    ListItem::new(Span::styled(
                        truncate_chars(" Enter · e · Del ", inner_w),
                        theme::style_faint(),
                    )),
                ]
            } else {
                // Width-aware empty state — avoid "Structure follo…" / "FROM s…" clips.
                let (h1, h2, h3) = if inner_w < 18 {
                    (" Follows SQL ", " FROM→cols ", " Enter table ")
                } else if inner_w < 28 {
                    (" Follows query ", " FROM → columns ", " Enter a table ")
                } else {
                    (
                        " Structure follows the query ",
                        " Console: FROM schema.table → columns ",
                        " Or Enter a table in Explorer ",
                    )
                };
                vec![
                    ListItem::new(Span::styled(
                        truncate_chars(h1, inner_w),
                        theme::style_text().add_modifier(Modifier::BOLD),
                    )),
                    ListItem::new(Span::styled(
                        truncate_chars(h2, inner_w),
                        theme::style_muted(),
                    )),
                    ListItem::new(Span::styled(
                        truncate_chars(h3, inner_w),
                        theme::style_faint(),
                    )),
                ]
            };
            (title, items)
        }
        None => (
            " Structure ".into(),
            vec![ListItem::new(Span::styled(" — ", theme::style_faint()))],
        ),
    };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(border))
                .style(Style::default().bg(theme::bg())),
        ),
        area,
    );
    if let (Some(strip_area), Some(text)) = (strip_area, strip) {
        let shown = truncate_chars(&text, strip_area.width as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(shown, theme::style_muted())))
                .style(Style::default().bg(theme::surface_2())),
            strip_area,
        );
    }
}

fn render_sql_editor(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    let focused = ctx.ui.focus == Focus::Editor;
    let border = theme::border(focused);
    let title = if focused { " Console " } else { " console " };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(theme::bg()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut y = inner.y;
    let mut h = inner.height;

    // SQL tabs (Query / table data tabs) — numbered for Alt-1..9 / Ctrl-Tab
    if let Some(db) = ctx.db {
        if !db.tabs.is_empty() && h >= 1 {
            let mut spans = Vec::new();
            for (i, tab) in db.tabs.iter().enumerate() {
                let active = i == db.active_tab;
                let style = if active {
                    Style::default()
                        .fg(theme::bg())
                        .bg(theme::accent())
                        .add_modifier(Modifier::BOLD)
                } else {
                    theme::style_muted()
                };
                let title = tab.display_title();
                let label = if i < 9 {
                    format!(" {}:{} ", i + 1, title)
                } else {
                    format!(" {} ", title)
                };
                spans.push(Span::styled(label, style));
            }
            frame.render_widget(
                Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::bg())),
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                },
            );
            y = y.saturating_add(1);
            h = h.saturating_sub(1);
        }
    }

    // Quiet toolbar — drop whole tokens when narrow (never mid-word like "publi").
    if h >= 2 {
        let running = ctx.db.map(|d| d.query_running).unwrap_or(false)
            || ctx.ui.busy_label.as_deref() == Some("RUNNING…");
        let db_label = ctx
            .db
            .map(|d| d.console_database_label())
            .unwrap_or_else(|| "database".into());
        let schema_label = ctx
            .db
            .map(|d| d.console_schema_label())
            .unwrap_or_else(|| "schema".into());
        let lim = match ctx.db.map(|d| d.select_limit).unwrap_or(50) {
            0 => "lim off".into(),
            n => format!("lim {n}"),
        };
        let spans = console_toolbar_spans(running, &db_label, &schema_label, &lim, inner.width);
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::bg())),
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            },
        );
        y = y.saturating_add(1);
        h = h.saturating_sub(1);
    }
    let inner = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: h,
    };
    if inner.height == 0 {
        return;
    }

    let sql = ctx.db.map(|d| d.sql.as_str()).unwrap_or("-- SQL");
    let height = inner.height as usize;
    let width = inner.width as usize;
    let lines: Vec<&str> = sql.lines().collect();
    let line_count = lines.len().max(1);
    let gutter = line_count.ilog10() as usize + 1;
    let text_cols = width.saturating_sub(gutter + 2);
    let start = ctx.ui.db_scroll_row;
    let end = (start + height).min(line_count);

    let highlights = ctx
        .highlighter
        .highlight_lines(LanguageId::Sql, sql, start, end);

    let mut out_lines = Vec::new();
    for row in start..end {
        let rel = row - start;
        let text = lines.get(row).copied().unwrap_or("");
        let mut spans = vec![Span::styled(
            format!("{:>width$} ", row + 1, width = gutter),
            Style::default().fg(Color::DarkGray),
        )];
        let hl = highlights.get(rel).cloned().unwrap_or_default();
        let chars: Vec<char> = text.chars().collect();
        let visible_end = chars.len().min(text_cols);
        let mut col = 0;
        while col < visible_end {
            let kind = hl
                .iter()
                .find(|s| col >= s.start_col && col < s.end_col)
                .map(|s| s.kind)
                .unwrap_or(HighlightKind::Default);
            let run_end = hl
                .iter()
                .find(|s| col >= s.start_col && col < s.end_col)
                .map(|s| s.end_col.min(visible_end))
                .unwrap_or_else(|| {
                    hl.iter()
                        .filter(|s| s.start_col > col)
                        .map(|s| s.start_col)
                        .min()
                        .unwrap_or(visible_end)
                        .min(visible_end)
                });
            let slice: String = chars[col..run_end].iter().collect();
            let style = if ctx.ui.db_sql_all_selected {
                Style::default()
                    .fg(theme::bg())
                    .bg(theme::accent())
            } else {
                style_for(kind)
            };
            spans.push(Span::styled(slice, style));
            col = run_end;
        }
        // Empty selected lines still need a visible bar.
        if ctx.ui.db_sql_all_selected && visible_end == 0 {
            spans.push(Span::styled(
                " ",
                Style::default().fg(theme::bg()).bg(theme::accent()),
            ));
        }
        out_lines.push(Line::from(spans));
    }
    // Idle plane stays empty — shortcuts live on the toolbar (and footer on focus).
    // No faint duplicate strip at the bottom of Console.
    frame.render_widget(Paragraph::new(out_lines), inner);

    if focused
        && !matches!(
            ctx.ui.mode,
            Mode::Palette | Mode::Command | Mode::Search | Mode::Rename | Mode::Find | Mode::Replace
                | Mode::Commit | Mode::DbConnect
        )
    {
        let row = ctx.ui.db_cursor.row.saturating_sub(ctx.ui.db_scroll_row);
        if row < height {
            let line_len = lines
                .get(ctx.ui.db_cursor.row)
                .map(|l| l.chars().count())
                .unwrap_or(0);
            let x = inner.x + (gutter as u16) + 1 + ctx.ui.db_cursor.col.min(line_len).min(text_cols) as u16;
            let y = inner.y + row as u16;
            if x < inner.x + inner.width && y < inner.y + inner.height {
                frame.set_cursor_position((x, y));
            }
        }
    }
}

/// File/folder glyphs (Nerd Font / Unicode). Needs a Nerd Font in the host terminal
/// (e.g. JetBrainsMono Nerd Font). ASCII twist stays single-width.
fn file_tree_icon(name: &str, is_dir: bool, expanded: bool) -> (&'static str, Color) {
    // Devicon / Nerd Font PUA — host terminal needs a Nerd Font (JetBrainsMono NF, etc.).
    if is_dir {
        let icon = if expanded { "\u{e5fe}" } else { "\u{e5ff}" }; // folder-open / folder
        return (icon, theme::muted());
    }
    let lower = name.to_ascii_lowercase();
    let (icon, color) = match lower.as_str() {
        "cargo.toml" | "cargo.lock" => ("\u{e7a8}", theme::text()),
        "readme.md" | "license" | "license.md" => ("\u{f48a}", theme::muted()),
        ".gitignore" | ".gitattributes" => ("\u{f1d3}", theme::faint()),
        _ => {
            if let Some(ext) = lower.rsplit_once('.').map(|(_, e)| e) {
                match ext {
                    "rs" => ("\u{e7a8}", theme::text()),
                    "toml" => ("\u{e6b2}", theme::muted()),
                    "md" | "markdown" => ("\u{f48a}", theme::muted()),
                    "json" | "jsonc" => ("\u{e60b}", theme::muted()),
                    "yml" | "yaml" => ("\u{f481}", theme::muted()),
                    "ts" | "tsx" => ("\u{e628}", theme::muted()),
                    "js" | "jsx" | "mjs" => ("\u{e74e}", theme::muted()),
                    "py" => ("\u{e73c}", theme::muted()),
                    "go" => ("\u{e724}", theme::muted()),
                    "lua" => ("\u{e620}", theme::muted()),
                    "sh" | "bash" | "zsh" => ("\u{f489}", theme::muted()),
                    "sql" => ("\u{e706}", theme::muted()),
                    "html" | "htm" => ("\u{e736}", theme::muted()),
                    "css" | "scss" => ("\u{e749}", theme::muted()),
                    "svg" | "png" | "jpg" | "jpeg" | "webp" | "gif" => ("\u{f1c5}", theme::faint()),
                    "lock" => ("\u{f023}", theme::faint()),
                    _ => ("\u{f016}", theme::faint()),
                }
            } else {
                ("\u{f016}", theme::faint())
            }
        }
    };
    (icon, color)
}

fn render_tree(frame: &mut Frame, area: Rect, ws: &Workspace, focused: bool) {
    let border = theme::border(focused);
    let items: Vec<ListItem> = ws
        .flat
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let twist = if e.is_dir {
                if e.expanded {
                    "▾"
                } else {
                    "▸"
                }
            } else {
                " "
            };
            let (icon, icon_color) = file_tree_icon(&e.name, e.is_dir, e.expanded);
            let indent = "  ".repeat(e.depth);
            let selected = i == ws.selected;
            let twist_style = if selected {
                theme::style_selected()
            } else {
                theme::style_faint()
            };
            let icon_style = if selected {
                theme::style_selected()
            } else {
                Style::default().fg(icon_color)
            };
            let name_style = if selected {
                theme::style_selected()
            } else if e.is_dir {
                theme::style_text().add_modifier(Modifier::BOLD)
            } else {
                theme::style_muted()
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{indent}{twist} "), twist_style),
                Span::styled(format!("{icon} "), icon_style),
                Span::styled(e.name.clone(), name_style),
            ]))
        })
        .collect();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::RIGHT)
                .title(" Files ")
                .border_style(Style::default().fg(border))
                .style(Style::default().bg(theme::surface())),
        ),
        area,
    );
}

/// Hit-test product ribbon tabs on row 0. db-studio has no multi-product ribbon.
pub fn product_tab_at_col(_col: u16) -> Option<u8> {
    None
}

fn render_git(frame: &mut Frame, area: Rect, git: Option<&GitState>, focused: bool) {
    let border = if focused { Color::Cyan } else { Color::DarkGray };
    let (title, items) = match git {
        Some(g) => {
            let checkout = g
                .branches
                .get(g.branch_selected)
                .map(|b| b.as_str())
                .unwrap_or("?");
            let title = format!(" Git ({}) → {checkout} ", g.branch);
            let mut items: Vec<ListItem> = g
                .branches
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let mark = if i == g.branch_selected { "›" } else { " " };
                    let style = if i == g.branch_selected {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    ListItem::new(Span::styled(format!(" {mark} {b}"), style))
                })
                .collect();
            items.push(ListItem::new(Span::styled(
                " ── files ── ",
                Style::default().fg(Color::DarkGray),
            )));
            if g.files.is_empty() {
                items.push(ListItem::new(Span::styled(
                    " clean working tree ",
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                items.extend(g.files.iter().enumerate().map(|(i, f)| {
                    let mark = if f.staged { "S" } else { " " };
                    let style = if i == g.selected {
                        theme::style_selected()
                    } else {
                        Style::default()
                    };
                    ListItem::new(Span::styled(
                        format!("[{mark}] {:?} {}", f.status, f.path.display()),
                        style,
                    ))
                }));
            }
            (title, items)
        }
        None => (
            " Git ".into(),
            vec![ListItem::new(" not a git repo ".to_string())],
        ),
    };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(border)),
        ),
        area,
    );
}

fn render_sessions(
    frame: &mut Frame,
    area: Rect,
    sessions: Option<&ide_sessions::SessionManager>,
    focused: bool,
) {
    let border = if focused { Color::Cyan } else { Color::DarkGray };
    let mut items: Vec<ListItem> = Vec::new();
    match sessions {
        Some(s) if !s.profiles.is_empty() => {
            items.push(ListItem::new(Span::styled(
                " Enter connect · a SSH · A RDP · e edit · x del ",
                Style::default().fg(Color::DarkGray),
            )));
            for (i, p) in s.profiles.iter().enumerate() {
                let style = if i == s.selected {
                    theme::style_selected()
                } else {
                    Style::default()
                };
                let live = s.live_ssh.iter().any(|l| l.profile_id == p.id)
                    || s.live_rdp.iter().any(|l| l.profile_id == p.id);
                let mark = if live { "*" } else { " " };
                items.push(ListItem::new(Span::styled(
                    format!("{mark} {}", p.display_label()),
                    style,
                )));
            }
            if !s.live_rdp.is_empty() {
                items.push(ListItem::new(Span::styled(
                    format!(" ── rdp running: {} ── ", s.live_rdp.len()),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        _ => {
            items.push(ListItem::new(Span::styled(
                " No profiles — a add SSH · A add RDP ",
                Style::default().fg(Color::DarkGray),
            )));
            items.push(ListItem::new(Span::styled(
                " Ctrl-Shift-S Sessions · ~/.config/tui-ide/sessions.toml ",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Sessions ")
                .border_style(Style::default().fg(border)),
        ),
        area,
    );
}

fn render_agents(
    frame: &mut Frame,
    area: Rect,
    agents: Option<&ide_agents::AgentManager>,
    focused: bool,
) {
    let border = if focused { Color::Cyan } else { Color::DarkGray };
    let mut items: Vec<ListItem> = Vec::new();
    match agents {
        Some(a) if !a.profiles.is_empty() => {
            items.push(ListItem::new(Span::styled(
                " Enter · a add · e · x ",
                Style::default().fg(Color::DarkGray).bg(Color::Black),
            )));
            for (i, p) in a.profiles.iter().enumerate() {
                let live = a.live.iter().any(|l| l.profile_id == p.id);
                let style = if i == a.selected {
                    theme::style_selected()
                } else {
                    Style::default().fg(Color::Gray).bg(Color::Black)
                };
                let mark = if live { "▸" } else { " " };
                items.push(ListItem::new(Span::styled(
                    format!("{mark} {}", p.list_label()),
                    style,
                )));
            }
            if !a.live.is_empty() {
                items.push(ListItem::new(Span::styled(
                    format!(" {} live · Ctrl-] tabs ", a.live.len()),
                    Style::default().fg(Color::DarkGray).bg(Color::Black),
                )));
            }
        }
        _ => {
            items.push(ListItem::new(Span::styled(
                " a add profile ",
                Style::default().fg(Color::DarkGray).bg(Color::Black),
            )));
            items.push(ListItem::new(Span::styled(
                " Ctrl-Shift-A · agents.toml ",
                Style::default().fg(Color::DarkGray).bg(Color::Black),
            )));
        }
    }
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Agents ")
                .border_style(Style::default().fg(border))
                .style(Style::default().bg(Color::Black)),
        ),
        area,
    );
}

fn render_agents_main(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    let focused = ctx.ui.focus == Focus::Terminal || ctx.ui.focus == Focus::Agents;
    let border = if focused { Color::Cyan } else { Color::DarkGray };
    let title = if ctx.session_tab_labels.is_empty() {
        " Agent · Enter on profile to launch ".to_string()
    } else {
        let tabs: String = ctx
            .session_tab_labels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if i == ctx.session_tab_active {
                    format!("[{l}]")
                } else {
                    format!(" {l} ")
                }
            })
            .collect::<Vec<_>>()
            .join("│");
        format!(" Agent {tabs} · Ctrl-] next · Ctrl-W close ")
    };
    let lines = ctx.terminal_lines.unwrap_or(&[]);
    if lines.is_empty() {
        let hint = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Launch a profile from the left (Enter).",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Cursor Agent / Claude Code / OpenCode run as real CLIs here.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  API keys stay in env — never in agents.toml.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        frame.render_widget(
            Paragraph::new(hint).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(border))
                    .style(Style::default().bg(Color::Black)),
            ),
            area,
        );
        return;
    }
    let text: Vec<Line> = lines
        .iter()
        .map(|l| Line::from(l.clone()))
        .collect();
    frame.render_widget(
        Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(border))
                .style(Style::default().bg(Color::Black)),
        ),
        area,
    );
}

fn render_search(frame: &mut Frame, area: Rect, search: Option<&SearchState>, focused: bool) {
    let border = if focused { Color::Cyan } else { Color::DarkGray };
    let items: Vec<ListItem> = match search {
        Some(s) if !s.hits.is_empty() => s
            .hits
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let style = if i == s.selected {
                    theme::style_selected()
                } else {
                    Style::default().fg(Color::Gray)
                };
                let name = h
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?");
                ListItem::new(Span::styled(
                    format!("{name}:{}:{} {}", h.line, h.column, h.text),
                    style,
                ))
            })
            .collect(),
        Some(s) if s.error.is_some() => {
            vec![ListItem::new(s.error.clone().unwrap_or_default())]
        }
        _ => vec![ListItem::new(Span::styled(
            " Ctrl-Shift-F / 5 to search ",
            Style::default().fg(Color::DarkGray),
        ))],
    };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Search ")
                .border_style(Style::default().fg(border)),
        ),
        area,
    );
}

fn render_outline(frame: &mut Frame, area: Rect, ui: &UiState) {
    let focused = ui.focus == Focus::Outline;
    let border = theme::border(focused);
    let items: Vec<ListItem> = if ui.outline.is_empty() {
        vec![ListItem::new(Span::styled(
            " symbols · Ctrl-⇧-O ",
            theme::style_faint(),
        ))]
    } else {
        ui.outline
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let style = if i == ui.outline_selected {
                    theme::style_selected()
                } else {
                    theme::style_muted()
                };
                ListItem::new(Span::styled(
                    format!(" {}  {}", s.kind.chars().next().unwrap_or('·'), s.name),
                    style,
                ))
            })
            .collect()
    };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::LEFT)
                .title(" Outline ")
                .border_style(Style::default().fg(border))
                .style(Style::default().bg(theme::surface())),
        ),
        area,
    );
}

fn render_debug_side(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    let focused = ctx.ui.focus == Focus::Debug;
    let border = if focused { Color::Cyan } else { Color::DarkGray };
    let mut lines: Vec<Line> = Vec::new();
    if ctx.ui.stack_frames.is_empty() {
        lines.push(Line::from(Span::styled(
            " (no stack — F5 continue) ",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            " Stack ",
            Style::default().fg(Color::Yellow),
        )));
        for (i, f) in ctx.ui.stack_frames.iter().enumerate() {
            let loc = f
                .path
                .as_deref()
                .and_then(|p| p.rsplit('/').next())
                .unwrap_or("?");
            let style = if i == ctx.ui.debug_frame_selected {
                theme::style_selected()
            } else {
                Style::default().fg(Color::Gray)
            };
            lines.push(Line::from(Span::styled(
                format!("  {}:{} {} ({loc})", f.line, f.name, i),
                style,
            )));
        }
    }
    if !ctx.ui.variables.is_empty() {
        lines.push(Line::from(Span::styled(
            " Locals ",
            Style::default().fg(Color::Yellow),
        )));
        for v in ctx.ui.variables.iter().take(24) {
            let ty = v.ty.as_deref().unwrap_or("");
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", v.name), Style::default().fg(Color::Cyan)),
                Span::raw(format!("= {} {ty}", v.value)),
            ]));
        }
    }
    if let Some(log) = ctx.debug_log {
        if !log.is_empty() {
            lines.push(Line::from(Span::styled(
                " Log ",
                Style::default().fg(Color::Yellow),
            )));
            for l in log.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
                lines.push(Line::from(Span::styled(
                    format!("  {l}"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(" F9 breakpoint · F5 continue · F10 step · r refresh "));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Debug ")
                    .border_style(Style::default().fg(border)),
            ),
        area,
    );
}

fn render_editor(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>, secondary: bool) {
    let focused = ctx.ui.focus == Focus::Editor
        && ctx.ui.split.focus_secondary == secondary
        && (ctx.ui.split.enabled || !secondary);
    let border = if focused { Color::Cyan } else { Color::DarkGray };

    let buf_idx = if secondary {
        ctx.ui.split.secondary.unwrap_or(ctx.buffers.active_index())
    } else {
        ctx.buffers.active_index()
    };

    let title = if secondary {
        " Editor (split) "
    } else {
        " Editor "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(buf) = ctx.buffers.get(buf_idx) else {
        frame.render_widget(Paragraph::new("Open a file (Enter / Ctrl-P)"), inner);
        return;
    };

    let height = inner.height as usize;
    let width = inner.width as usize;
    let gutter = ((buf.line_count()).max(1).ilog10() as usize) + 1;
    let text_cols = width.saturating_sub(gutter + 2);
    let start = buf.scroll_row;
    let end = (start + height).min(buf.line_count());
    let source = buf.text();
    let highlights = ctx
        .highlighter
        .highlight_lines(buf.language, &source, start, end);

    let path_diags: Vec<&DiagnosticItem> = ctx
        .diagnostics
        .iter()
        .filter(|d| buf.path.as_ref() == Some(&d.path))
        .collect();

    let mut lines = Vec::new();
    for row in start..end {
        let rel = row - start;
        let text = buf.line(row);
        let has_diag = path_diags.iter().any(|d| d.line as usize == row);
        let mut spans = vec![Span::styled(
            format!("{:>width$} ", row + 1, width = gutter),
            if has_diag {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        )];

        let hl = highlights.get(rel).cloned().unwrap_or_default();
        let chars: Vec<char> = text.chars().collect();
        let visible_end = chars.len().min(text_cols);
        let mut col = 0;
        while col < visible_end {
            let kind = hl
                .iter()
                .find(|s| col >= s.start_col && col < s.end_col)
                .map(|s| s.kind)
                .unwrap_or(HighlightKind::Default);
            let run_end = hl
                .iter()
                .find(|s| col >= s.start_col && col < s.end_col)
                .map(|s| s.end_col.min(visible_end))
                .unwrap_or_else(|| {
                    hl.iter()
                        .filter(|s| s.start_col > col)
                        .map(|s| s.start_col)
                        .min()
                        .unwrap_or(visible_end)
                        .min(visible_end)
                });
            let slice: String = chars[col..run_end].iter().collect();
            spans.push(Span::styled(slice, style_for(kind)));
            col = run_end;
        }

        if ctx.ui.show_inlays {
            for hint in ctx.ui.inlays.iter().filter(|h| h.line as usize == row) {
                spans.push(Span::styled(
                    format!(" {}", hint.label),
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                ));
            }
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), inner);

    if focused && !matches!(ctx.ui.mode, Mode::Palette | Mode::Command | Mode::Search | Mode::Rename)
    {
        let row = buf.cursor.row.saturating_sub(buf.scroll_row);
        if row < height {
            let x = inner.x + (gutter as u16) + 1 + buf.cursor.col.min(text_cols) as u16;
            let y = inner.y + row as u16;
            if x < inner.x + inner.width && y < inner.y + inner.height {
                frame.set_cursor_position((x, y));
            }
        }
    }
}

fn style_for(kind: HighlightKind) -> Style {
    // Mono terminal: ACCENT≈TEXT, so contrast comes from dimming non-keywords.
    match kind {
        HighlightKind::Keyword => Style::default()
            .fg(theme::text())
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        HighlightKind::Type | HighlightKind::Property => Style::default()
            .fg(theme::text())
            .add_modifier(Modifier::BOLD),
        HighlightKind::Function => Style::default()
            .fg(theme::text())
            .add_modifier(Modifier::BOLD),
        HighlightKind::String => Style::default().fg(theme::faint()),
        HighlightKind::Comment => Style::default()
            .fg(theme::faint())
            .add_modifier(Modifier::ITALIC),
        HighlightKind::Number | HighlightKind::Constant => Style::default()
            .fg(theme::text())
            .add_modifier(Modifier::BOLD),
        HighlightKind::Variable => Style::default().fg(theme::muted()),
        HighlightKind::Operator | HighlightKind::Punctuation => Style::default().fg(theme::faint()),
        HighlightKind::Default => Style::default().fg(theme::muted()),
    }
}

fn render_bottom(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    match ctx.ui.bottom {
        BottomPanel::Problems => render_diagnostics(frame, area, ctx.diagnostics, ctx.ui),
        BottomPanel::Terminal => {
            let border = if ctx.ui.focus == Focus::Terminal {
                Color::Cyan
            } else {
                Color::DarkGray
            };
            let title = if ctx.session_tab_labels.is_empty() {
                " Terminal ".to_string()
            } else {
                let tabs: String = ctx
                    .session_tab_labels
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        if i == ctx.session_tab_active {
                            format!("[{l}]")
                        } else {
                            format!(" {l} ")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("|");
                let kind = if ctx.terminal_tab_kind.is_empty() {
                    "Term"
                } else {
                    ctx.terminal_tab_kind
                };
                format!(" {kind} {tabs} ")
            };
            let lines = ctx.terminal_lines.unwrap_or(&[]);
            let text: Vec<Line> = lines
                .iter()
                .map(|l| Line::from(l.clone()))
                .collect();
            frame.render_widget(
                Paragraph::new(text).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(title)
                        .border_style(Style::default().fg(border)),
                ),
                area,
            );
        }
        BottomPanel::Debug => {
            let text = ctx.debug_log.unwrap_or("");
            frame.render_widget(
                Paragraph::new(text).wrap(Wrap { trim: false }).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Debug Console "),
                ),
                area,
            );
        }
        BottomPanel::Results => render_results_grid(frame, area, ctx),
    }
}

fn render_table_filter_bars(
    frame: &mut Frame,
    area: Rect,
    ctx: &RenderCtx<'_>,
    db: &DbWorkspace,
) {
    let editing = ctx.ui.mode == Mode::EditTableFilter && ctx.ui.focus == Focus::Results;
    let where_val = if editing && ctx.ui.table_filter_field == TableFilterField::Where {
        format!("{}▌", ctx.ui.table_filter_draft)
    } else {
        db.active_table_where().to_string()
    };
    let order_val = if editing && ctx.ui.table_filter_field == TableFilterField::OrderBy {
        format!("{}▌", ctx.ui.table_filter_draft)
    } else {
        db.active_table_order_by().to_string()
    };
    let where_focus = editing && ctx.ui.table_filter_field == TableFilterField::Where;
    let order_focus = editing && ctx.ui.table_filter_field == TableFilterField::OrderBy;
    let where_style = if where_focus {
        Style::default()
            .fg(theme::bg())
            .bg(theme::accent())
            .add_modifier(Modifier::BOLD)
    } else {
        theme::style_text()
    };
    let order_style = if order_focus {
        Style::default()
            .fg(theme::bg())
            .bg(theme::accent())
            .add_modifier(Modifier::BOLD)
    } else {
        theme::style_text()
    };
    // One compact row: WHERE … │ ORDER BY …
    let label_w = " WHERE ".chars().count() + " │ ORDER BY ".chars().count();
    let budget = (area.width as usize).saturating_sub(label_w).max(4);
    let where_budget = (budget * 3 / 5).max(2);
    let order_budget = budget.saturating_sub(where_budget).max(2);
    let where_shown = truncate_chars(&where_val, where_budget);
    let order_shown = truncate_chars(&order_val, order_budget);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" WHERE ", theme::style_muted().add_modifier(Modifier::BOLD)),
            Span::styled(where_shown, where_style),
            Span::styled(" │ ORDER BY ", theme::style_muted().add_modifier(Modifier::BOLD)),
            Span::styled(order_shown, order_style),
        ]))
        .style(Style::default().bg(theme::surface())),
        area,
    );
}

fn render_results_grid(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    let focused = ctx.ui.focus == Focus::Results;
    let border = theme::border(focused);
    let Some(db) = ctx.db else {
        frame.render_widget(
            Paragraph::new(Span::styled(" — ", theme::style_faint())).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(if focused { " Results " } else { " results " })
                    .border_style(Style::default().fg(border))
                    .style(Style::default().bg(theme::bg())),
            ),
            area,
        );
        return;
    };
    let table_tab = db.active_is_table_tab();
    let Some(result) = &db.last_result else {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(if focused { " Results " } else { " results " })
            .border_style(Style::default().fg(border))
            .style(Style::default().bg(theme::bg()));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if table_tab && inner.height >= 2 {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .split(inner);
            render_table_filter_bars(frame, chunks[0], ctx, db);
            frame.render_widget(
                Paragraph::new(Span::styled("  F5 / Enter run ", theme::style_faint())),
                chunks[1],
            );
        } else {
            frame.render_widget(
                Paragraph::new(Span::styled("  F5 run ", theme::style_faint())),
                inner,
            );
        }
        return;
    };

    let table_label = match (&result.source_schema, &result.source_table) {
        (Some(s), Some(t)) => format!("{s}.{t}"),
        (None, Some(t)) => t.clone(),
        _ => String::new(),
    };
    let pk_label = if result.primary_keys.is_empty() {
        String::new()
    } else {
        format!(" PK:{} ", result.primary_keys.join("+"))
    };
    let dirty_n = result.dirty_rows().len();
    let dirty_label = if dirty_n > 0 {
        format!(" *{dirty_n} dirty ")
    } else {
        String::new()
    };
    let nrows = result.rows.len();
    let ncols = result.columns.len();
    let compact = ctx.ui.results_compact;
    let density_tag = if compact { "compact" } else { "comfort" };
    // Border title: short label only. All metrics live on the meta line.
    let title = if focused {
        " Results ".to_string()
    } else {
        " results ".to_string()
    };
    let mut meta_bits: Vec<String> = vec![
        format!("{nrows}×{ncols}"),
        density_tag.to_string(),
        format!("{} ms", result.elapsed_ms),
    ];
    if !table_label.is_empty() {
        meta_bits.push(table_label);
    }
    if !pk_label.trim().is_empty() {
        meta_bits.push(pk_label.trim().to_string());
    }
    if result.truncated {
        meta_bits.push("truncated".into());
    }
    if result.has_more {
        meta_bits.push(format!("page {}+", result.page + 1));
    } else if result.page > 0 {
        meta_bits.push(format!("page {}", result.page + 1));
    }
    if db.select_limit > 0 {
        meta_bits.push(format!("lim {}", db.select_limit));
    }
    if !dirty_label.trim().is_empty() {
        meta_bits.push(dirty_label.trim().to_string());
    }
    if !db.results_filter.trim().is_empty() {
        meta_bits.push(format!("/{}", db.results_filter.trim()));
    }
    let meta_line = format!(" {} ", meta_bits.join(" · "));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(theme::bg()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Meta strip under the border title (always when we have height).
    let inner = if inner.height >= 3 {
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(2)])
            .split(inner);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_chars(&meta_line, parts[0].width as usize),
                theme::style_faint(),
            )))
            .style(Style::default().bg(theme::bg())),
            parts[0],
        );
        parts[1]
    } else {
        inner
    };

    let inner = if table_tab && inner.height >= 3 {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(2)])
            .split(inner);
        render_table_filter_bars(frame, chunks[0], ctx, db);
        chunks[1]
    } else {
        inner
    };

    // Full cell value lives in a Hover card (not a permanent bottom strip).
    let grid_area = inner;

    // compact = scan density; comfort = readable text (body/json) without leaving the grid.
    let max_col_width = if compact { 14usize } else { 30usize };
    let row_stride = if compact { 1usize } else { 2usize };
    let cell_stride = max_col_width + 2; // leading/trailing space
    let vis_cols = (grid_area.width as usize / cell_stride).max(1);
    let pinned = ctx.ui.results_pinned_cols.min(ncols).min(vis_cols.saturating_sub(1));
    let scroll_budget = vis_cols.saturating_sub(pinned).max(1);
    // Horizontal window follows the cursor column; pinned cols stay visible.
    let scroll_start = if result.cursor_col >= pinned + scroll_budget {
        result.cursor_col + 1 - scroll_budget
    } else {
        pinned
    };
    let col_start = scroll_start.max(pinned);
    let col_end = (col_start + scroll_budget).min(ncols);
    let editing = focused && ctx.ui.mode == Mode::EditCell;

    let col_indices: Vec<usize> = (0..pinned).chain(col_start..col_end).collect();

    let header: Line = Line::from(
        col_indices
            .iter()
            .map(|&ci| {
                let c = &result.columns[ci];
                let t = truncate_chars(c, max_col_width);
                let style = if ci == result.cursor_col {
                    Style::default()
                        .fg(theme::text())
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                } else {
                    Style::default().fg(theme::muted()).add_modifier(Modifier::BOLD)
                };
                Span::styled(
                    format!(" {t:<max_col_width$} ", max_col_width = max_col_width),
                    style,
                )
            })
            .collect::<Vec<_>>(),
    );

    let filter = db.results_filter.clone();
    let filtered_idx: Vec<usize> = result
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| ide_db::row_matches_filter(row, &filter))
        .map(|(i, _)| i)
        .collect();
    let body_h = grid_area.height.saturating_sub(2) as usize;
    let visible_rows = (body_h / row_stride).max(1);
    let cursor_in_filtered = filtered_idx
        .iter()
        .position(|&i| i == result.cursor_row)
        .unwrap_or(0);
    let scroll = cursor_in_filtered.saturating_sub(visible_rows.saturating_sub(1) / 2);
    let dirty = result.dirty_rows();
    let mut rows: Vec<Line> = Vec::new();
    for &ri in filtered_idx.iter().skip(scroll).take(visible_rows) {
        let row = &result.rows[ri];
        let row_dirty = dirty.contains(&ri);
        rows.push(Line::from(
            col_indices
                .iter()
                .filter_map(|&ci| row.get(ci).map(|cell| (ci, cell)))
                .map(|(ci, cell)| {
                    let is_cursor = ri == result.cursor_row && ci == result.cursor_col;
                    let is_null = cell == "NULL";
                    let mut t = if is_cursor && editing {
                        // Inline editor — show draft + block cursor inside the cell.
                        let mut draft = ctx.ui.cell_edit_input.clone();
                        draft.push('▌');
                        let chars: Vec<char> = draft.chars().collect();
                        if chars.len() > max_col_width {
                            chars[chars.len() - max_col_width..].iter().collect()
                        } else {
                            draft
                        }
                    } else if is_null {
                        truncate_chars("NULL", max_col_width)
                    } else {
                        truncate_chars(cell, max_col_width)
                    };
                    if row_dirty && !(is_cursor && editing) {
                        t.push('*');
                        if t.chars().count() > max_col_width {
                            t = truncate_chars(&t, max_col_width);
                        }
                    }
                    let style = if is_cursor && editing {
                        Style::default()
                            .fg(theme::bg())
                            .bg(theme::accent())
                            .add_modifier(Modifier::BOLD)
                    } else if is_cursor {
                        theme::style_selected()
                    } else if row_dirty {
                        Style::default().fg(theme::accent())
                    } else if ci < pinned {
                        Style::default().fg(theme::text()).add_modifier(Modifier::BOLD)
                    } else if is_null {
                        theme::style_faint().add_modifier(Modifier::ITALIC)
                    } else {
                        theme::style_text()
                    };
                    Span::styled(
                        format!(" {t:<max_col_width$} ", max_col_width = max_col_width),
                        style,
                    )
                })
                .collect::<Vec<_>>(),
        ));
        if !compact {
            rows.push(Line::from(Span::styled(
                " ".repeat(grid_area.width as usize),
                theme::style_faint(),
            )));
        }
    }
    let h_hint = if ncols > vis_cols {
        format!(" cols {}-{} / {ncols}", col_start + 1, col_end)
    } else {
        String::new()
    };
    let rule_w = (grid_area.width as usize).saturating_sub(h_hint.chars().count());
    let rule = Line::from(Span::styled(
        format!("{}{}", "─".repeat(rule_w), h_hint),
        theme::style_faint(),
    ));
    let mut lines = vec![header, rule];
    if nrows == 0 {
        lines.push(Line::from(Span::styled(
            "  (0 rows)",
            theme::style_muted(),
        )));
    }
    lines.extend(rows);
    if let Some(chart) = &result.chart {
        lines.push(Line::from(Span::styled(
            "─".repeat(grid_area.width as usize),
            theme::style_faint(),
        )));
        for cl in chart.lines().take(8) {
            lines.push(Line::from(Span::styled(
                format!(" {cl}"),
                theme::style_muted(),
            )));
        }
    } else if let Some(ex) = &result.explain {
        lines.push(Line::from(Span::styled(
            truncate_chars(ex, grid_area.width as usize),
            theme::style_faint(),
        )));
    }
    frame.render_widget(Paragraph::new(lines), grid_area);

    if focused {
        let col_name = result
            .columns
            .get(result.cursor_col)
            .map(|s| s.as_str())
            .unwrap_or("?");
        let raw = if editing {
            ctx.ui.cell_edit_input.as_str()
        } else {
            result
                .rows
                .get(result.cursor_row)
                .and_then(|r| r.get(result.cursor_col))
                .map(|s| s.as_str())
                .unwrap_or("")
        };
        // Only float when the grid cell is truncated or the value is long / NULL.
        let cell_shown_w = max_col_width;
        let needs_hover = raw == "NULL"
            || raw.chars().count() > cell_shown_w
            || editing;
        if needs_hover {
            render_cell_hover(frame, grid_area, col_name, raw);
        }
    }
}

fn render_diagnostics(frame: &mut Frame, area: Rect, diags: &[DiagnosticItem], ui: &UiState) {
    let focused = ui.focus == Focus::Diagnostics;
    let border = if focused { Color::Cyan } else { Color::DarkGray };
    let items: Vec<ListItem> = if diags.is_empty() {
        vec![ListItem::new(Span::styled(
            " No diagnostics ",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        diags
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let sev = match d.severity {
                    lsp_types::DiagnosticSeverity::ERROR => "E",
                    lsp_types::DiagnosticSeverity::WARNING => "W",
                    lsp_types::DiagnosticSeverity::INFORMATION => "I",
                    lsp_types::DiagnosticSeverity::HINT => "H",
                    _ => "?",
                };
                let name = d
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?");
                let text = format!(
                    "[{sev}] {name}:{}:{} {}  [. quickfix]",
                    d.line + 1,
                    d.character + 1,
                    d.message
                );
                let style = if i == ui.diag_selected && focused {
                    theme::style_selected()
                } else if sev == "E" {
                    Style::default().fg(Color::Red)
                } else if sev == "W" {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::Gray)
                };
                ListItem::new(Span::styled(text, style))
            })
            .collect()
    };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Problems ")
                .border_style(Style::default().fg(border)),
        ),
        area,
    );
}

fn render_status(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    let mode = ctx.ui.mode.label();
    let product = match ctx.ui.product {
        ProductMode::Code => "CODE",
        ProductMode::Database => "DB",
    };
    let (path, cursor, dirty) = if ctx.ui.product == ProductMode::Database {
        (
            "sql".into(),
            format!(
                "{}:{}",
                ctx.ui.db_cursor.row + 1,
                ctx.ui.db_cursor.col + 1
            ),
            String::new(),
        )
    } else {
        match ctx.buffers.active() {
        Some(b) => (
            b.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "untitled".into()),
            format!("{}:{}", b.cursor.row + 1, b.cursor.col + 1),
            if b.dirty {
                "[+]".into()
            } else {
                String::new()
            },
        ),
            None => ("—".into(), "1:1".into(), String::new()),
        }
    };
    let mut spans = if ctx.ui.product == ProductMode::Database {
        vec![
            Span::styled(
                " DB ",
                Style::default()
                    .fg(theme::bg())
                    .bg(theme::text())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {mode} "), theme::style_muted()),
            Span::styled(format!(" {cursor} "), theme::style_faint()),
        ]
    } else {
        vec![
            Span::styled(
                format!(" {product} "),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {mode} "),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {path} {dirty}  {cursor} ")),
        ]
    };
    if ctx.ui.product == ProductMode::Database {
        if let Some(db) = ctx.db {
            if db.in_txn {
                spans.push(Span::styled(
                    " TXN ",
                    Style::default()
                        .fg(theme::bg())
                        .bg(theme::text())
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if let Some(role) = db
                .active_source
                .as_ref()
                .and_then(|id| db.sources.iter().find(|s| &s.id == id))
                .map(|s| s.role.label())
            {
                spans.push(Span::styled(
                    format!(" {role} "),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ));
            }
        }
    }
    if let Some(msg) = &ctx.ui.status {
        let style = match msg.tone {
            StatusTone::Success => theme::style_ok(),
            StatusTone::Error => theme::style_err_badge(),
            StatusTone::Info => theme::style_muted(),
        };
        spans.push(Span::styled(format!(" | {} ", msg.text), style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::bg())),
        area,
    );
}

fn render_completion_popup(
    frame: &mut Frame,
    editor_area: Rect,
    ui: &UiState,
    buffers: &BufferSet,
    db: Option<&DbWorkspace>,
) {
    if editor_area.width < 12 || editor_area.height < 3 {
        return;
    }
    let height = (ui.completions.len().min(10) as u16)
        .saturating_add(2)
        .min(editor_area.height.saturating_sub(1));
    let width = 48u16
        .min(editor_area.width.saturating_sub(2))
        .max(24)
        .min(editor_area.width);

    // Caret in screen coords. Console chrome = border + tabs + toolbar.
    let (caret_x, caret_y) = if ui.product == ProductMode::Database {
        let has_tabs = db.map(|d| !d.tabs.is_empty()).unwrap_or(false);
        let chrome_y = 1u16 // top border
            + u16::from(has_tabs)
            + 1; // F5 / db ▾ / schema ▾ toolbar
        let sql = db.map(|d| d.sql.as_str()).unwrap_or("");
        let line_count = sql.lines().count().max(1);
        let gutter = (line_count.ilog10() as u16) + 1;
        let rel = ui.db_cursor.row.saturating_sub(ui.db_scroll_row) as u16;
        let col = ui.db_cursor.col as u16;
        let x = editor_area
            .x
            .saturating_add(1) // left border
            .saturating_add(gutter)
            .saturating_add(1) // gutter space
            .saturating_add(col);
        let y = editor_area.y.saturating_add(chrome_y).saturating_add(rel);
        (x, y)
    } else if let Some(buf) = buffers.active() {
        let rel = buf.cursor.row.saturating_sub(buf.scroll_row) as u16;
        let x = editor_area
            .x
            .saturating_add(6)
            .saturating_add(buf.cursor.col as u16);
        let y = editor_area.y.saturating_add(1).saturating_add(rel);
        (x, y)
    } else {
        return;
    };

    let top = editor_area.y;
    let bottom = editor_area.y.saturating_add(editor_area.height);
    let right = editor_area.x.saturating_add(editor_area.width);

    // Prefer BELOW the caret so typed text stays visible; flip ABOVE if needed.
    // Never sit on the Console toolbar strip (first chrome rows inside the pane).
    let toolbar_floor = editor_area.y.saturating_add(3);
    let mut y = if caret_y.saturating_add(1).saturating_add(height) <= bottom {
        caret_y.saturating_add(1)
    } else if caret_y.saturating_sub(toolbar_floor.max(top)) >= height {
        caret_y.saturating_sub(height)
    } else {
        bottom.saturating_sub(height).max(toolbar_floor.min(bottom.saturating_sub(height)))
    };
    if y < toolbar_floor && toolbar_floor.saturating_add(height) <= bottom {
        y = toolbar_floor;
    }

    // Prefer to the RIGHT of the caret (prefix stays readable); else left; else clamp.
    let x = if caret_x.saturating_add(2).saturating_add(width) <= right {
        caret_x.saturating_add(2)
    } else if caret_x > editor_area.x.saturating_add(width.saturating_add(1)) {
        caret_x.saturating_sub(width.saturating_add(1))
    } else {
        right.saturating_sub(width).max(editor_area.x)
    };

    let area = Rect {
        x,
        y,
        width,
        height,
    };
    let items: Vec<ListItem> = ui
        .completions
        .iter()
        .enumerate()
        .take(10)
        .map(|(i, c)| {
            let detail = c.detail.as_deref().unwrap_or("");
            let selected = i == ui.completion_idx;
            let label_style = if selected {
                theme::style_selected()
            } else {
                theme::style_text()
            };
            let detail_style = if selected {
                theme::style_selected()
            } else {
                theme::style_faint()
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {} ", c.label), label_style),
                Span::styled(format!("{detail} "), detail_style),
            ]))
        })
        .collect();
    frame.render_widget(Clear, area);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::border_idle()))
                .style(Style::default().bg(theme::bg()))
                .title(Span::styled(
                    " Completions ",
                    Style::default()
                        .fg(theme::bg())
                        .bg(theme::text())
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

fn render_hover(frame: &mut Frame, editor_area: Rect, hover: &str) {
    let width = editor_area.width.saturating_sub(4).min(70).max(24);
    let wrapped = hover_wrap_lines(hover, (width as usize).saturating_sub(4).max(12));
    let height = (wrapped.len() as u16).saturating_add(2).min(14);
    let area = Rect {
        x: editor_area.x + 2,
        y: editor_area.y + 1,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let body: Vec<Line> = wrapped
        .into_iter()
        .map(|l| Line::from(Span::styled(l, theme::style_text())))
        .collect();
    frame.render_widget(
        Paragraph::new(body).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::border_idle()))
                .style(Style::default().bg(theme::bg()))
                .title(Span::styled(
                    " Hover ",
                    Style::default()
                        .fg(theme::bg())
                        .bg(theme::text())
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

/// Results cell full-value card — replaces the old bottom gray preview strip.
fn render_cell_hover(frame: &mut Frame, grid_area: Rect, col_name: &str, raw: &str) {
    if grid_area.width < 16 || grid_area.height < 4 {
        return;
    }
    let width = grid_area.width.saturating_sub(4).min(72).max(28);
    let inner_w = (width as usize).saturating_sub(4).max(12);
    let body_lines = hover_wrap_lines(raw, inner_w);
    let height = (body_lines.len() as u16)
        .saturating_add(2)
        .min(10)
        .min(grid_area.height.saturating_sub(1));
    // Prefer upper-right of the Results pane so the cursor row stays visible.
    let x = grid_area
        .x
        .saturating_add(grid_area.width.saturating_sub(width).saturating_sub(1));
    let y = grid_area.y.saturating_add(1);
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let title = format!(" {} ", truncate_chars(col_name, 24));
    let body: Vec<Line> = body_lines
        .into_iter()
        .map(|l| {
            let style = if raw == "NULL" {
                theme::style_faint().add_modifier(Modifier::ITALIC)
            } else {
                theme::style_text()
            };
            Line::from(Span::styled(l, style))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(body).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::border_idle()))
                .style(Style::default().bg(theme::bg()))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(theme::bg())
                        .bg(theme::text())
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

fn hover_wrap_lines(s: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for para in s.split('\n') {
        if para.is_empty() {
            out.push(String::new());
            continue;
        }
        let chars: Vec<char> = para.chars().collect();
        for chunk in chars.chunks(width) {
            out.push(chunk.iter().collect());
            if out.len() >= 8 {
                return out;
            }
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn render_host_header(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>, _code_active: bool) {
    let brand = Span::styled(
        " DB Studio ",
        Style::default()
            .fg(theme::bg())
            .bg(theme::text())
            .add_modifier(Modifier::BOLD),
    );

    // Home: brand only — actions live in the body (A / W).
    if ctx.ui.db_surface == DbSurface::Home {
        frame.render_widget(
            Paragraph::new(Line::from(vec![brand])).style(Style::default().bg(theme::bg())),
            area,
        );
        return;
    }

    let active_id = ctx.db.and_then(|d| d.active_source.as_deref());
    let conn = ctx
        .db
        .and_then(|d| {
            active_id
                .and_then(|id| d.sources.iter().find(|s| s.id == id))
                .map(|s| s.name.as_str())
        })
        .unwrap_or("—");
    let busy = ctx
        .ui
        .busy_label
        .as_deref()
        .or_else(|| {
            if ctx.db.map(|d| d.query_running).unwrap_or(false) {
                Some("RUNNING…")
            } else {
                None
            }
        });
    // Quiet chrome like Home — no health "ok" badge in the header.
    let mut spans = vec![
        brand,
        Span::styled("  studio  ", theme::style_muted()),
        Span::styled("·  ", theme::style_faint()),
        Span::styled(conn.to_string(), theme::style_faint()),
    ];
    if let Some(b) = busy {
        spans.push(Span::styled("  ·  ", theme::style_faint()));
        spans.push(Span::styled(
            b.to_string(),
            theme::style_accent().add_modifier(Modifier::BOLD),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::bg())),
        area,
    );
}

fn render_db_help_overlay(frame: &mut Frame, screen: Rect, surface: DbSurface) {
    let width = screen.width.min(64).max(40);
    let height = screen.height.min(18).max(12);
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 4;
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let title = match surface {
        DbSurface::Home => " help · Home ",
        DbSurface::Studio => " help · Studio ",
    };
    let lines: Vec<Line> = match surface {
        DbSurface::Home => vec![
            Line::from(Span::styled(
                "A add connection",
                theme::style_muted(),
            )),
            Line::from(Span::styled(
                "W workspaces · pick connection → Studio",
                theme::style_muted(),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "? / Esc close",
                theme::style_faint(),
            )),
        ],
        DbSurface::Studio => vec![
            Line::from(Span::styled(
                "1–4 panes · Tab cycle · f fullscreen",
                theme::style_muted(),
            )),
            Line::from(Span::styled(
                "Explorer: Enter open · d Go to DDL · n Create menu",
                theme::style_muted(),
            )),
            Line::from(Span::styled(
                "Table data: w WHERE · o ORDER BY · Enter apply",
                theme::style_muted(),
            )),
            Line::from(Span::styled(
                "Super-⇧A Search Everywhere · Super-⇧G Git · Super-` Terminal",
                theme::style_muted(),
            )),
            Line::from(Span::styled(
                "Super-⇧O Hierarchy · F7 Debug · Super-⇧F Search",
                theme::style_muted(),
            )),
            Line::from(Span::styled(
                "Ctrl+arrows resize focused pane · Esc Home",
                theme::style_muted(),
            )),
            Line::from(Span::styled(
                "F5 run · b database · s schema · D density · Super-⇧E CSV",
                theme::style_muted(),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "? / Esc close",
                theme::style_faint(),
            )),
        ],
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(theme::border_idle()))
                    .style(Style::default().bg(theme::surface())),
            )
            .alignment(ratatui::layout::Alignment::Left),
        area,
    );
}

fn render_home_dashboard(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    // Left-rail: brand → Code / Database / Sessions. Path at bottom.
    let full = ctx.workspace.root.display().to_string();
    let sel = ctx.ui.home_selected.min(2);

    frame.render_widget(
        Block::default()
            .style(Style::default().bg(theme::bg()))
            .borders(Borders::NONE),
        area,
    );

    let rail = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(48),
            Constraint::Min(0),
        ])
        .split(area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(7),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(rail[1]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "IDE",
            Style::default()
                .fg(theme::text())
                .add_modifier(Modifier::BOLD),
        ))),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "────────────────────────",
            theme::style_faint(),
        ))),
        rows[2],
    );

    let items = ["Code", "Database", "Sessions"];
    let mut actions = Vec::with_capacity(5);
    for (i, name) in items.iter().enumerate() {
        let on = i == sel;
        let num = format!("{}", i + 1);
        let num_style = if on {
            theme::style_selected()
        } else {
            theme::style_muted()
        };
        let name_style = if on {
            Style::default()
                .fg(theme::text())
                .add_modifier(Modifier::BOLD)
        } else {
            theme::style_muted()
        };
        actions.push(Line::from(vec![
            Span::styled(num, num_style),
            Span::styled(format!("  {name}"), name_style),
        ]));
        if i + 1 < items.len() {
            actions.push(Line::from(""));
        }
    }
    frame.render_widget(Paragraph::new(actions), rows[4]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(full, theme::style_faint()))),
        rows[6],
    );
}

/// Build the same 1..=9 slot list shown on Home (projects then files).
pub fn home_recent_slots(
    recent_projects: &[String],
    recent_files: &[String],
    current_workspace: &str,
) -> Vec<HomeRecentSlot> {
    let mut out = Vec::new();
    for path in recent_projects.iter() {
        if out.len() >= 9 {
            break;
        }
        if path == current_workspace {
            continue;
        }
        if out.iter().filter(|s| matches!(s, HomeRecentSlot::Project(_))).count() >= 5 {
            break;
        }
        out.push(HomeRecentSlot::Project(path.clone()));
    }
    for path in recent_files.iter() {
        if out.len() >= 9 {
            break;
        }
        let files_n = out
            .iter()
            .filter(|s| matches!(s, HomeRecentSlot::File(_)))
            .count();
        if files_n >= 4 {
            break;
        }
        out.push(HomeRecentSlot::File(path.clone()));
    }
    out
}

#[derive(Debug, Clone)]
pub enum HomeRecentSlot {
    Project(String),
    File(String),
}

fn render_db_home(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    frame.render_widget(
        Block::default()
            .borders(Borders::NONE)
            .style(Style::default().bg(theme::bg())),
        area,
    );

    // Brand lives in the header — body shows connections (DataGrip-like) + A/W tips.
    let sources = ctx.db.map(|d| d.sources.as_slice()).unwrap_or(&[]);
    let active = ctx.db.and_then(|d| d.active_source.as_deref());
    let n = sources.len();
    let sel = if n == 0 {
        0
    } else {
        ctx.ui.home_selected.min(n.saturating_sub(1))
    };

    // Tips live only in the footer — avoid duplicating Enter/A/W in the body.
    let col = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(4)])
        .margin(2)
        .split(area);

    let mut lines: Vec<Line<'static>> = vec![
        Line::from(Span::styled(
            "Connections",
            theme::style_muted().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    if sources.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No connections yet — A to add",
            theme::style_faint(),
        )));
    } else {
        for (i, src) in sources.iter().enumerate() {
            let is_sel = i == sel;
            let is_active = active == Some(src.id.as_str());
            let mark = if is_active { "●" } else { "○" };
            let where_ = match src.driver {
                ide_db::DbDriver::Sqlite => {
                    if src.file.is_empty() {
                        "sqlite".into()
                    } else {
                        truncate_chars(&src.file, 36)
                    }
                }
                _ => {
                    if src.database.is_empty() {
                        format!("{}:{}", src.host, src.port)
                    } else {
                        format!("{}:{}/{}", src.host, src.port, src.database)
                    }
                }
            };
            let label = format!(
                " {mark} {} · {} · {where_}",
                src.name,
                src.driver.label()
            );
            let style = if is_sel {
                Style::default()
                    .fg(theme::text())
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else if is_active {
                theme::style_text().add_modifier(Modifier::BOLD)
            } else {
                theme::style_text()
            };
            lines.push(Line::from(Span::styled(label, style)));
        }
    }

    frame.render_widget(Paragraph::new(lines), col[1]);
}

fn render_db_home_status(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    // One footer only — toast then keybinds. Green OK only for Success tone.
    let line = if let Some(msg) = &ctx.ui.status {
        let mut spans = Vec::new();
        match msg.tone {
            StatusTone::Success => {
                spans.push(Span::styled(" OK ", theme::style_ok_badge()));
            }
            StatusTone::Error => {
                spans.push(Span::styled(" ERR ", theme::style_err_badge()));
            }
            StatusTone::Info => {
                spans.push(Span::styled(" · ", theme::style_faint()));
            }
        }
        spans.push(Span::styled(
            format!(" {} ", msg.text),
            theme::style_text(),
        ));
        Line::from(spans)
    } else {
        Line::from(vec![
            Span::styled(
                " DB Home ",
                Style::default()
                    .fg(theme::bg())
                    .bg(theme::text())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Enter open  ·  A add  ·  W workspaces ",
                theme::style_faint(),
            ),
        ])
    };
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::bg())),
        area,
    );
}

/// Approximate Console pane rect inside Studio (for completion popup anchoring).
fn studio_console_area(size: Rect, ui: &UiState) -> Rect {
    let body = Rect {
        x: size.x,
        y: size.y.saturating_add(1),
        width: size.width,
        height: size.height.saturating_sub(2),
    };
    if ui.db_fullscreen == Some(Focus::Editor) {
        return body;
    }
    let results_pct = ui.results_pct.clamp(25, 70);
    let top_pct = 100u16.saturating_sub(results_pct).max(30);
    let top_h = (body.height as u32 * top_pct as u32 / 100).max(3) as u16;
    let left_w = studio_left_stack_width(body.width, ui.studio_left_width);
    Rect {
        x: body.x.saturating_add(left_w),
        y: body.y,
        width: body.width.saturating_sub(left_w),
        height: top_h.min(body.height),
    }
}

fn render_database_studio(frame: &mut Frame, size: Rect, ctx: &RenderCtx<'_>) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(size);

    render_host_header(frame, root[0], ctx, false);

    // DataGrip-like tool windows: Git / Search / Hierarchy / Debug overlay Studio.
    let tool_focus = matches!(
        ctx.ui.focus,
        Focus::Git | Focus::Search | Focus::Outline | Focus::Debug
    );
    if tool_focus {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Min(20)])
            .split(root[1]);
        match ctx.ui.focus {
            Focus::Git => render_git(frame, body[0], ctx.git, true),
            Focus::Search => render_search(frame, body[0], ctx.search, true),
            Focus::Outline => render_outline(frame, body[0], ctx.ui),
            Focus::Debug => render_debug_side(frame, body[0], ctx),
            _ => {}
        }
        render_results_grid(frame, body[1], ctx);
        render_studio_footer(frame, root[2], ctx);
        return;
    }
    if ctx.ui.focus == Focus::Terminal {
        let body = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(root[1]);
        // Keep explorer|structure|console on top strip when terminal open.
        let left_w = studio_left_stack_width(body[0].width, ctx.ui.studio_left_width);
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(left_w), Constraint::Min(24)])
            .split(body[0]);
        let explor_w = studio_explorer_width(left_w, ctx.ui.explorer_width);
        let left = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(explor_w), Constraint::Min(10)])
            .split(top[0]);
        render_db_tree(frame, left[0], ctx.db, ctx.ui.focus == Focus::Database);
        render_db_structure(frame, left[1], ctx.db, ctx.ui.focus == Focus::Structure);
        render_sql_editor(frame, top[1], ctx);
        render_bottom(frame, body[1], ctx);
        render_studio_footer(frame, root[2], ctx);
        return;
    }

    if let Some(pane) = ctx.ui.db_fullscreen {
        match pane {
            Focus::Database => {
                render_db_tree(frame, root[1], ctx.db, true);
            }
            Focus::Structure => {
                render_db_structure(frame, root[1], ctx.db, true);
            }
            Focus::Editor => {
                render_sql_editor(frame, root[1], ctx);
            }
            Focus::Results => {
                render_results_grid(frame, root[1], ctx);
            }
            _ => {
                render_db_tree(frame, root[1], ctx.db, true);
            }
        }
    } else {
        let results_pct = ctx.ui.results_pct.clamp(25, 70);
        let top_pct = 100u16.saturating_sub(results_pct).max(30);
        let body = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(top_pct),
                Constraint::Percentage(results_pct),
            ])
            .split(root[1]);

        let left_w = studio_left_stack_width(body[0].width, ctx.ui.studio_left_width);
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(left_w), Constraint::Min(24)])
            .split(body[0]);

        // Explorer | Structure — keep Structure ≥24 cols so empty-state copy stays readable.
        let explor_w = studio_explorer_width(left_w, ctx.ui.explorer_width);
        let left = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(explor_w),
                Constraint::Min(10),
            ])
            .split(top[0]);
        render_db_tree(frame, left[0], ctx.db, ctx.ui.focus == Focus::Database);
        render_db_structure(frame, left[1], ctx.db, ctx.ui.focus == Focus::Structure);
        render_sql_editor(frame, top[1], ctx);
        render_results_grid(frame, body[1], ctx);
    }

    render_studio_footer(frame, root[2], ctx);
}

fn render_studio_footer(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    // Footer: status wins over keymap. Green OK only for Success (not pane tips).
    if let Some(msg) = &ctx.ui.status {
        let text = if msg.text.chars().count() > 90 {
            format!("{}…", msg.text.chars().take(89).collect::<String>())
        } else {
            msg.text.clone()
        };
        let mut spans = Vec::new();
        match msg.tone {
            StatusTone::Success => {
                spans.push(Span::styled(" OK ", theme::style_ok_badge()));
            }
            StatusTone::Error => {
                spans.push(Span::styled(" ERR ", theme::style_err_badge()));
            }
            StatusTone::Info => {
                spans.push(Span::styled(" · ", theme::style_faint()));
            }
        }
        spans.push(Span::styled(format!(" {text} "), theme::style_text()));
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::bg())),
            area,
        );
        return;
    }

    let pane = match ctx.ui.focus {
        Focus::Database => "Explorer",
        Focus::Structure => "Structure",
        Focus::Editor => match ctx.ui.mode {
            Mode::Insert => "Console · INSERT",
            Mode::Normal => "Console · NORMAL",
            _ => "Console",
        },
        Focus::Results => "Results",
        Focus::Git => "Git",
        Focus::Search => "Search",
        Focus::Outline => "Hierarchy",
        Focus::Debug => "Debug",
        Focus::Terminal => "Terminal",
        _ => "Studio",
    };
    let hint = match ctx.ui.focus {
        Focus::Database => "Enter open · p props · e edit · n New · d DDL",
        Focus::Structure => "Ctrl+←→ split  ·  Esc explorer",
        Focus::Editor if matches!(ctx.ui.mode, Mode::Insert) => {
            "typing · Alt-b DB · Alt-s schema · Alt-L limit · Esc"
        }
        Focus::Editor => {
            "i insert · b/s DB·schema · Alt-L limit · Ctrl-A all"
        }
        Focus::Results if ctx.ui.mode == Mode::EditCell => "Enter save  ·  Esc cancel",
        Focus::Results if ctx.ui.mode == Mode::EditTableFilter => "Enter apply · Tab field · Esc",
        Focus::Results if ctx.db.is_some_and(|d| d.active_is_table_tab()) => {
            "w WHERE · o ORDER BY · x close tab · r refresh"
        }
        Focus::Results => "D density · cell hover · ]/[ page · Alt-L limit",
        Focus::Git => "Esc/q back · s stage · c commit",
        Focus::Terminal => "q exits pager · Ctrl-` / Esc back",
        Focus::Search | Focus::Outline | Focus::Debug => "Esc/q back to Studio",
        _ => "Tab panes  ·  Esc home",
    };
    let zoom = if ctx.ui.db_fullscreen.is_some() {
        " · full"
    } else {
        ""
    };

    let spans = vec![
        Span::styled(
            format!(" {pane} "),
            Style::default()
                .fg(theme::bg())
                .bg(theme::text())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {hint}{zoom} "), theme::style_faint()),
    ];

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::bg())),
        area,
    );
}

fn render_nvim_grid(frame: &mut Frame, area: Rect, grid: &NvimGrid) {
    let rows = area.height as usize;
    let cols = area.width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut spans: Vec<Span> = Vec::with_capacity(cols);
        let mut c = 0usize;
        while c < cols {
            let (text, attr_id) = if r < grid.height && c < grid.width {
                grid.cell(r, c)
                    .map(|cell| (cell.text.clone(), cell.attr_id))
                    .unwrap_or_else(|| (" ".into(), 0))
            } else {
                (" ".into(), 0)
            };
            let attr = grid.attr(attr_id);
            let mut style = Style::default();
            if let Some((rr, gg, bb)) = attr.fg {
                style = style.fg(Color::Rgb(rr, gg, bb));
            }
            if let Some((rr, gg, bb)) = attr.bg {
                style = style.bg(Color::Rgb(rr, gg, bb));
            }
            if attr.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            if attr.italic {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if attr.underline {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if attr.reverse {
                style = style.add_modifier(Modifier::REVERSED);
            }
            let is_cursor = r == grid.cursor_row && c == grid.cursor_col;
            if is_cursor {
                style = style.add_modifier(Modifier::REVERSED);
            }
            let display = if text.is_empty() {
                " ".to_string()
            } else {
                text
            };
            // Wide cells: advance by unicode width best-effort (nvim already laid out)
            let w = unicode_width::UnicodeWidthStr::width(display.as_str()).max(1);
            spans.push(Span::styled(display, style));
            c += w;
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_nvim_buf_tabs(
    frame: &mut Frame,
    area: Rect,
    bufs: &[ide_nvim::NvimBufInfo],
    breadcrumbs: &str,
) {
    let mut spans: Vec<Span> = Vec::new();
    for (i, b) in bufs.iter().take(8).enumerate() {
        let mark = if b.current { " " } else { " " };
        let label = if i < 9 {
            format!("{mark}{} {} ", i + 1, b.name)
        } else {
            format!("{mark}{} ", b.name)
        };
        if b.current {
            spans.push(Span::styled(label, theme::style_selected()));
        } else {
            spans.push(Span::styled(label, theme::style_muted()));
        }
    }
    if bufs.len() > 8 {
        spans.push(Span::styled(
            format!(" +{} ", bufs.len() - 8),
            theme::style_faint(),
        ));
    }
    if !breadcrumbs.is_empty() {
        spans.push(Span::styled("  │  ", theme::style_faint()));
        spans.push(Span::styled(
            truncate_chars(breadcrumbs, 48),
            theme::style_faint(),
        ));
    } else {
        spans.push(Span::styled("  Alt-1…9 ", theme::style_faint()));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::surface())),
        area,
    );
}

fn render_nvim_host_status(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    diag: (u64, u64),
    git: Option<&GitState>,
) {
    let mode = label
        .rsplit_once('·')
        .map(|(_, m)| m.trim())
        .unwrap_or("");
    let backend = label
        .rsplit_once('·')
        .map(|(b, _)| b.trim())
        .unwrap_or(label);
    let branch = git
        .and_then(|g| {
            if g.branch.is_empty() {
                None
            } else {
                Some(g.branch.as_str())
            }
        })
        .unwrap_or("—");
    let diag_txt = if diag.0 + diag.1 > 0 {
        format!("E{} W{}", diag.0, diag.1)
    } else {
        "ok".into()
    };
    let line = Line::from(vec![
        Span::styled(" IDE ", theme::style_selected()),
        Span::styled(format!(" {branch} "), theme::style_muted()),
        if mode.is_empty() {
            Span::raw("")
        } else {
            Span::styled(format!(" {mode} "), theme::style_text())
        },
        Span::styled(format!(" │ {diag_txt} "), theme::style_faint()),
        Span::styled(
            " │ Files·Git·DB · Ctrl-⇧A agents · Ctrl-1…3 · Ctrl-Q ",
            theme::style_faint(),
        ),
    ]);
    let _ = backend;
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::surface())),
        area,
    );
}

/// Truncate on Unicode char boundaries (byte slices panic on multi-byte glyphs like —).
/// Studio left stack (Explorer+Structure): prefer enough room for Structure copy.
fn studio_left_stack_width(body_width: u16, configured: u16) -> u16 {
    let max = body_width.saturating_sub(24).max(40);
    configured.clamp(40, 120).min(max)
}

/// Cap Explorer so Structure keeps ≥24 cols (readable empty state / titles).
fn studio_explorer_width(left_w: u16, configured: u16) -> u16 {
    let max_explor = left_w.saturating_sub(24).max(14);
    configured.clamp(14, max_explor)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

/// Build Console toolbar spans; drop schema then database as whole chips when narrow.
fn console_toolbar_spans(
    running: bool,
    db_label: &str,
    schema_label: &str,
    lim: &str,
    width: u16,
) -> Vec<Span<'static>> {
    let run = if running {
        Span::styled(" RUNNING… Esc ", theme::style_selected())
    } else {
        Span::styled(" F5 run ", theme::style_faint())
    };
    let lim_span = Span::styled(format!("Alt-L {lim}"), theme::style_muted());
    let db_chip = format!("Alt-b {db_label} ▾");
    let schema_chip = format!("Alt-s {schema_label} ▾");

    // Candidates from richest → sparsest; pick first that fits.
    let candidates: Vec<Vec<Span<'static>>> = vec![
        vec![
            Span::styled(" ", theme::style_faint()),
            run.clone(),
            Span::styled(" │ ", theme::style_faint()),
            Span::styled(db_chip.clone(), theme::style_muted()),
            Span::styled(" │ ", theme::style_faint()),
            Span::styled(schema_chip.clone(), theme::style_muted()),
            Span::styled(" │ ", theme::style_faint()),
            lim_span.clone(),
        ],
        vec![
            Span::styled(" ", theme::style_faint()),
            run.clone(),
            Span::styled(" │ ", theme::style_faint()),
            Span::styled(db_chip.clone(), theme::style_muted()),
            Span::styled(" │ ", theme::style_faint()),
            lim_span.clone(),
        ],
        vec![
            Span::styled(" ", theme::style_faint()),
            run.clone(),
            Span::styled(" │ ", theme::style_faint()),
            lim_span.clone(),
        ],
        vec![Span::styled(" ", theme::style_faint()), run, Span::raw(" "), lim_span],
    ];

    let max = width as usize;
    for c in candidates {
        let len: usize = c.iter().map(|s| s.content.chars().count()).sum();
        if len <= max {
            return c;
        }
    }
    vec![
        Span::styled(" F5 ", theme::style_faint()),
        Span::styled(lim.to_string(), theme::style_muted()),
    ]
}

/// Drop whole ` · `-separated tokens from the right before mid-glyph ellipsis.
fn fit_dot_title(title: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if title.chars().count() <= max {
        return title.to_string();
    }
    let parts: Vec<&str> = title.split(" · ").collect();
    if parts.len() <= 1 {
        return truncate_chars(title, max);
    }
    for keep in (1..parts.len()).rev() {
        let candidate = parts[..keep].join(" · ");
        if candidate.chars().count() <= max {
            return candidate;
        }
    }
    truncate_chars(parts[0], max)
}

/// Source row label: prefer `Name (Driver) [role]`, then drop driver, then role,
/// then ellipsize only the name — never mid-token clips like `[ad…]`.
fn fit_explorer_source_label(base_label: &str, role: Option<&str>, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let role_tag = role
        .filter(|r| !r.is_empty())
        .map(|r| format!(" [{r}]"))
        .unwrap_or_default();

    // base is typically "★ Name (PostgreSQL)" from source_tree_node.
    let (prefix_name, driver) = match base_label.rfind(" (") {
        Some(i) if base_label.ends_with(')') => {
            let name = &base_label[..i];
            let drv = &base_label[i + 2..base_label.len() - 1];
            (name, Some(drv))
        }
        _ => (base_label, None),
    };

    let with_driver = match driver {
        Some(d) => format!("{prefix_name} ({d}){role_tag}"),
        None => format!("{prefix_name}{role_tag}"),
    };
    if with_driver.chars().count() <= max {
        return with_driver;
    }

    let name_role = format!("{prefix_name}{role_tag}");
    if name_role.chars().count() <= max {
        return name_role;
    }

    if prefix_name.chars().count() <= max {
        return prefix_name.to_string();
    }

    truncate_chars(prefix_name, max)
}

#[cfg(test)]
mod explorer_label_tests {
    use super::{fit_dot_title, fit_explorer_source_label};

    #[test]
    fn keeps_full_when_room() {
        let s = fit_explorer_source_label("DataLake (PostgreSQL)", Some("admin"), 40);
        assert_eq!(s, "DataLake (PostgreSQL) [admin]");
    }

    #[test]
    fn structure_title_drops_right_tokens() {
        let s = fit_dot_title("query · rag · public.session_notes", 18);
        assert_eq!(s, "query · rag");
        assert!(!s.ends_with('·'));
        assert!(!s.contains("session"));
    }

    #[test]
    fn structure_title_keeps_short() {
        assert_eq!(fit_dot_title("public.users", 40), "public.users");
    }

    #[test]
    fn drops_driver_before_role() {
        let s = fit_explorer_source_label("DataLake (PostgreSQL)", Some("admin"), 22);
        assert_eq!(s, "DataLake [admin]");
        assert!(!s.contains('…'));
    }

    #[test]
    fn drops_role_whole_never_mid_bracket() {
        let s = fit_explorer_source_label("DataLake (PostgreSQL)", Some("admin"), 10);
        assert_eq!(s, "DataLake");
        assert!(!s.contains('['));
        assert!(!s.contains("ad…"));
    }

    #[test]
    fn ellipsizes_name_only_when_forced() {
        let s = fit_explorer_source_label("VeryLongConnectionName (PostgreSQL)", Some("admin"), 8);
        assert!(s.ends_with('…'));
        assert!(!s.contains('['));
        assert!(!s.contains('('));
    }
}
