//! TUI layouts, keymap, and command palette.

mod db_connect_form;
mod keymap;
mod palette;
pub mod theme;
mod widgets;

pub use db_connect_form::DbConnectForm;
pub use keymap::{Action, KeyContext, Keymap};
pub use palette::{Palette, PaletteMode};
// HomeRecentSlot exported via lib for app quick-jump.

use ide_buffer::BufferSet;
use ide_core::{
    BottomPanel, CodeSurface, Cursor, DbSurface, Focus, LanguageId, Mode, ProductMode, Sidebar,
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

#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub text: String,
    pub error: bool,
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
    /// DB Home: Sources list vs Recent queries.
    pub home_pane: HomePane,
    /// Selected index within recent history (newest-first, 0..5).
    pub home_recent_selected: usize,
}

/// Which column is active on DB Home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HomePane {
    #[default]
    Sources,
    Recents,
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
                render_db_home_status(frame, root[2]);
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

fn parse_accent_color(raw: &str) -> Color {
    let s = raw.trim();
    if s.is_empty() {
        return theme::accent();
    }
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex[0..2], 16),
                u8::from_str_radix(&hex[2..4], 16),
                u8::from_str_radix(&hex[4..6], 16),
            ) {
                return Color::Rgb(r, g, b);
            }
        }
    }
    match s.to_ascii_lowercase().as_str() {
        "cyan" => Color::Cyan,
        "green" => Color::Green,
        "magenta" | "pink" => Color::Magenta,
        "yellow" => Color::Yellow,
        "red" => Color::Red,
        "blue" => Color::Blue,
        _ => theme::accent(),
    }
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
                // ASCII / width-1 only. ☰ is EastAsian Wide and left ghost "t" cells.
                let source_accent = |source_id: &str| -> Color {
                    ws.sources
                        .iter()
                        .find(|s| s.id == source_id)
                        .map(|s| parse_accent_color(&s.accent))
                        .unwrap_or(theme::accent())
                };
                let (glyph, color) = match e.kind {
                    DbNodeKind::Group => ("G", theme::muted()),
                    DbNodeKind::Source => {
                        let health = ws.connection_health.get(&e.source_id);
                        match health {
                            Some(ide_db::ConnHealth::Dead(_)) => ("!", theme::warn()),
                            Some(ide_db::ConnHealth::Ok { .. }) => ("*", Color::Green),
                            Some(ide_db::ConnHealth::Stale { .. }) => ("~", theme::warn()),
                            None => ("*", source_accent(&e.source_id)),
                        }
                    }
                    DbNodeKind::Database => ("#", theme::accent()),
                    DbNodeKind::Schema => ("~", theme::muted()),
                    DbNodeKind::Table => ("+", theme::text()),
                    DbNodeKind::View => ("=", theme::muted()),
                    DbNodeKind::Column => (".", theme::faint()),
                    DbNodeKind::Admin => ("@", theme::accent()),
                    DbNodeKind::AdminQuery => ("!", theme::muted()),
                    DbNodeKind::AdminAction => (">", theme::accent()),
                    DbNodeKind::Routine => ("*", theme::muted()),
                };
                let twist = match e.kind {
                    DbNodeKind::Column
                    | DbNodeKind::Table
                    | DbNodeKind::View
                    | DbNodeKind::AdminQuery
                    | DbNodeKind::AdminAction
                    | DbNodeKind::Routine => " ",
                    _ if e.expanded => "-",
                    _ => ">",
                };
                let indent = " ".repeat(e.depth.saturating_mul(2));
                let selected = i == ws.selected;
                let row_style = if selected {
                    theme::style_selected()
                } else {
                    Style::default()
                };
                let glyph_style = if selected {
                    theme::style_selected()
                } else {
                    Style::default().fg(color)
                };
                let label_style = if selected {
                    theme::style_selected()
                } else {
                    Style::default().fg(theme::text())
                };
                let inner_w = area.width.saturating_sub(2) as usize;
                let twist_part = format!("{indent}{twist} ");
                let glyph_part = format!("{glyph} ");
                let used = twist_part.chars().count() + glyph_part.chars().count();
                let label_max = inner_w.saturating_sub(used).max(4);
                let role_tag = if matches!(e.kind, DbNodeKind::Source) {
                    ws.sources
                        .iter()
                        .find(|s| s.id == e.source_id)
                        .map(|s| format!(" [{}]", s.role.label()))
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let mut label = truncate_chars(&format!("{}{role_tag}", e.label), label_max);
                let pad = inner_w.saturating_sub(used + label.chars().count());
                if pad > 0 {
                    label.push_str(&" ".repeat(pad));
                }
                ListItem::new(Line::from(vec![
                    Span::styled(twist_part, if selected { row_style } else { theme::style_faint() }),
                    Span::styled(glyph_part, glyph_style),
                    Span::styled(label, label_style),
                ]))
            })
            .collect(),
        Some(ws) => vec![
            ListItem::new(Span::styled(
                format!(" {} ", ws.status),
                theme::style_muted(),
            )),
            ListItem::new(Span::styled(
                " expand a source · Enter on a table ",
                theme::style_faint(),
            )),
        ],
        None => vec![ListItem::new(Span::styled(
            " no sources — press a to connect ",
            theme::style_faint(),
        ))],
    };
    let title = match db {
        Some(ws) => {
            let n = ws.sources.len();
            let active = ws
                .active_source
                .as_ref()
                .and_then(|id| ws.sources.iter().find(|s| &s.id == id))
                .map(|s| s.driver.label())
                .unwrap_or("—");
            if focused {
                format!(" ▸ EXPLORER · {n} src · {active} ")
            } else {
                format!(" Explorer · {n} src · {active} ")
            }
        }
        None => {
            if focused {
                " ▸ EXPLORER ".into()
            } else {
                " Explorer ".into()
            }
        }
    };
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(border))
                .style(Style::default().bg(theme::surface())),
        ),
        area,
    );
}

fn render_db_structure(frame: &mut Frame, area: Rect, db: Option<&DbWorkspace>, focused: bool) {
    let border = theme::border(focused);
    let inner_h = area.height.saturating_sub(2) as usize;
    let (title, items) = match db {
        Some(ws) if !ws.structure_lines.is_empty() => {
            let title = if focused {
                format!(" ▸ STRUCTURE · {} ", truncate_chars(&ws.structure_title, 28))
            } else {
                format!(" Structure · {} ", truncate_chars(&ws.structure_title, 28))
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
        Some(_) => (
            if focused {
                " ▸ STRUCTURE ".into()
            } else {
                " Structure ".into()
            },
            vec![ListItem::new(Span::styled(
                " select a table · columns · indexes · grants · DDL ",
                theme::style_faint(),
            ))],
        ),
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
                .style(Style::default().bg(theme::surface())),
        ),
        area,
    );
}

fn render_sql_editor(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    let focused = ctx.ui.focus == Focus::Editor;
    let border = theme::border(focused);
    let (conn, driver) = ctx
        .db
        .map(|d| {
            let src = d
                .active_source
                .as_ref()
                .and_then(|id| d.sources.iter().find(|s| &s.id == id));
            (
                src.map(|s| s.name.as_str()).unwrap_or(d.status.as_str()),
                src.map(|s| s.driver.label()).unwrap_or("SQL"),
            )
        })
        .unwrap_or(("database", "SQL"));
    let title = if focused {
        format!(" ▸ CONSOLE · {driver} · {conn} ")
    } else {
        format!(" Console · {driver} · {conn} ")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(theme::surface()));
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
                Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::surface_2())),
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

    // Toolbar strip inside console
    if h >= 2 {
        let run_hint = if ctx.db.map(|d| d.query_running).unwrap_or(false) {
            Span::styled(" RUNNING… Esc ", theme::style_selected())
        } else {
            Span::styled(" F5 / Ctrl-↵ run ", theme::style_text())
        };
        let hist_n = ctx.db.map(|d| d.history.len()).unwrap_or(0);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ▸ ", theme::style_muted()),
                run_hint,
                Span::styled(" · ", theme::style_faint()),
                Span::styled("Ctrl-E explain", theme::style_faint()),
                Span::styled(" · ", theme::style_faint()),
                Span::styled(
                    format!("hist {hist_n}"),
                    theme::style_faint(),
                ),
            ]))
            .style(Style::default().bg(theme::surface_2())),
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
            spans.push(Span::styled(slice, style_for(kind)));
            col = run_end;
        }
        out_lines.push(Line::from(spans));
    }
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
                        Style::default().bg(Color::Rgb(40, 40, 50))
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
                    Style::default().bg(Color::Rgb(40, 40, 50))
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
                    Style::default()
                        .bg(Color::Rgb(40, 40, 50))
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
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
                    Style::default().bg(Color::Rgb(40, 40, 50))
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
                Style::default().bg(Color::Rgb(40, 40, 50)).fg(Color::White)
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

fn render_results_grid(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>) {
    let focused = ctx.ui.focus == Focus::Results;
    let border = theme::border(focused);
    let Some(db) = ctx.db else {
        frame.render_widget(
            Paragraph::new(Span::styled(" no results ", theme::style_faint())).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(if focused {
                        " ▸ RESULTS "
                    } else {
                        " Results "
                    })
                    .border_style(Style::default().fg(border))
                    .style(Style::default().bg(theme::surface())),
            ),
            area,
        );
        return;
    };
    let Some(result) = &db.last_result else {
        let results_title = if focused {
            " ▸ RESULTS "
        } else {
            " Results "
        };
        let conn = db
            .active_source
            .as_ref()
            .and_then(|id| db.sources.iter().find(|s| &s.id == id))
            .map(|s| s.name.as_str())
            .unwrap_or("—");
        let hint = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  connected · {conn}"),
                theme::style_muted(),
            )),
            Line::from(Span::styled(
                "  F5 / Ctrl+↵  run Console SQL",
                theme::style_faint(),
            )),
            Line::from(Span::styled(
                "  1 Explorer · Enter on a table",
                theme::style_faint(),
            )),
            Line::from(Span::styled(
                "  y copy  ·  i edit  ·  / filter",
                theme::style_faint(),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(results_title)
                .border_style(Style::default().fg(border))
                .style(Style::default().bg(theme::surface())),
        );
        frame.render_widget(hint, area);
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
    let filter_tag = if db.results_filter.trim().is_empty() {
        String::new()
    } else {
        format!(" · /{}", db.results_filter.trim())
    };
    let title = format!(
        " {}{nrows}×{ncols} · {} ms{}{}{}{}{}{} · {density_tag} ",
        if focused { "▸ RESULTS · " } else { "Results · " },
        result.elapsed_ms,
        if table_label.is_empty() {
            String::new()
        } else {
            format!(" · {table_label}")
        },
        pk_label,
        if result.truncated { " · truncated" } else { "" },
        if result.has_more {
            format!(" · page {}+", result.page + 1)
        } else if result.page > 0 {
            format!(" · page {}", result.page + 1)
        } else {
            String::new()
        },
        dirty_label,
        filter_tag,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(theme::surface()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let max_col_width = if compact { 14usize } else { 22usize };
    let row_stride = if compact { 1usize } else { 2usize };
    let cell_stride = max_col_width + 2; // leading/trailing space
    let vis_cols = (inner.width as usize / cell_stride).max(1);
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
    let body_h = inner.height.saturating_sub(2) as usize;
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
                        truncate_chars("∅", max_col_width)
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
                        theme::style_faint()
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
                " ".repeat(inner.width as usize),
                theme::style_faint(),
            )));
        }
    }
    let h_hint = if ncols > vis_cols {
        format!(" cols {}-{} / {ncols}", col_start + 1, col_end)
    } else {
        String::new()
    };
    let rule_w = (inner.width as usize).saturating_sub(h_hint.chars().count());
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
            "─".repeat(inner.width as usize),
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
            truncate_chars(ex, inner.width as usize),
            theme::style_faint(),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
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
                    Style::default().bg(Color::Rgb(40, 40, 50))
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
        let style = if msg.error {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        };
        spans.push(Span::styled(format!(" | {} ", msg.text), style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Rgb(30, 30, 30))),
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
    let height = (ui.completions.len().min(10) as u16).saturating_add(2);
    let width = 56u16.min(editor_area.width.saturating_sub(4));
    let row = if ui.product == ProductMode::Database && ui.focus == Focus::Editor {
        ui.db_cursor
            .row
            .saturating_sub(ui.db_scroll_row) as u16
    } else if let Some(buf) = buffers.active() {
        buf.cursor.row.saturating_sub(buf.scroll_row) as u16
    } else {
        return;
    };
    let _ = db;
    let y = (editor_area.y + 1 + row + 1)
        .min(editor_area.y + editor_area.height.saturating_sub(height));
    let area = Rect {
        x: editor_area.x + 6,
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
            let style = if i == ui.completion_idx {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                Style::default()
            };
            ListItem::new(Span::styled(format!(" {}  {}", c.label, detail), style))
        })
        .collect();
    frame.render_widget(Clear, area);
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(" Completions ")),
        area,
    );
}

fn render_hover(frame: &mut Frame, editor_area: Rect, hover: &str) {
    let width = editor_area.width.saturating_sub(4).min(70);
    let lines: Vec<&str> = hover.lines().take(10).collect();
    let height = (lines.len() as u16).saturating_add(2).min(14);
    let area = Rect {
        x: editor_area.x + 2,
        y: editor_area.y + 1,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(hover.to_string())
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" Hover ")),
        area,
    );
}

fn render_host_header(frame: &mut Frame, area: Rect, ctx: &RenderCtx<'_>, _code_active: bool) {
    let active_id = ctx.db.and_then(|d| d.active_source.as_deref());
    let conn = ctx
        .db
        .and_then(|d| {
            active_id
                .and_then(|id| d.sources.iter().find(|s| s.id == id))
                .map(|s| s.name.as_str())
        })
        .unwrap_or("no connection");
    let health = ctx
        .db
        .and_then(|d| active_id.and_then(|id| d.connection_health.get(id)))
        .map(|h| h.badge())
        .unwrap_or("—");
    let surface = match ctx.ui.db_surface {
        DbSurface::Home => "home",
        DbSurface::Studio => "studio",
    };
    let spans = vec![
        Span::styled(
            " DB Studio ",
            Style::default()
                .fg(theme::bg())
                .bg(theme::text())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {surface}  "), theme::style_muted()),
        Span::styled("·  ", theme::style_faint()),
        Span::styled(conn.to_string(), theme::style_faint()),
        Span::styled("  ", theme::style_faint()),
        Span::styled(health, theme::style_muted()),
    ];
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::bg())),
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

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .margin(2)
        .split(area);

    let sources_n = ctx.db.map(|d| d.sources.len()).unwrap_or(0);
    let active = ctx
        .db
        .and_then(|d| {
            d.active_source
                .as_ref()
                .and_then(|id| d.sources.iter().find(|s| &s.id == id))
                .map(|s| s.name.as_str())
        })
        .unwrap_or("none");
    let sel = ctx.ui.home_selected;

    let hero = if sources_n == 0 {
        vec![
            Line::from(Span::styled(
                "DB Studio",
                Style::default()
                    .fg(theme::text())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "No connections yet. Press  a  to add one.",
                theme::style_muted(),
            )),
            Line::from(Span::styled(
                "Postgres · MySQL · SQLite · DuckDB",
                theme::style_faint(),
            )),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                "DB Studio",
                Style::default()
                    .fg(theme::text())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                format!("{sources_n} source(s)  ·  last  {active}"),
                theme::style_muted(),
            )),
            Line::from(Span::styled(
                "Enter connects · then Studio",
                theme::style_faint(),
            )),
        ]
    };
    frame.render_widget(Paragraph::new(hero), inner[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(inner[1]);

    let sources_active = ctx.ui.home_pane == HomePane::Sources;
    let sources_title = if sources_active {
        "▸ SOURCES"
    } else {
        "SOURCES"
    };
    let mut left: Vec<Line> = vec![
        Line::from(Span::styled(
            sources_title,
            theme::style_muted().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "────────────────────────────",
            theme::style_faint(),
        )),
        Line::from(""),
    ];

    if let Some(db) = ctx.db {
        if db.sources.is_empty() {
            left.push(Line::from(Span::styled(
                "(none — press a)",
                theme::style_faint(),
            )));
        } else {
            for (i, src) in db.sources.iter().take(9).enumerate() {
                let n = (i + 1) as u8;
                let selected = sources_active && i == sel;
                let is_active = db.active_source.as_deref() == Some(src.id.as_str());
                let health = db
                    .connection_health
                    .get(&src.id)
                    .map(|h| h.badge())
                    .unwrap_or("—");
                let health_style = match health {
                    "ok" => theme::style_accent().add_modifier(Modifier::BOLD),
                    "stale" => theme::style_muted(),
                    "dead" => theme::style_text().add_modifier(Modifier::BOLD),
                    _ => theme::style_faint(),
                };
                let pin = if src.pinned { "★ " } else { "  " };
                let cursor = if selected { "› " } else { "  " };
                let name_style = if selected {
                    Style::default()
                        .fg(theme::text())
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                        .fg(theme::text())
                        .add_modifier(Modifier::BOLD)
                };
                left.push(Line::from(vec![
                    Span::styled(cursor, theme::style_selected()),
                    Span::styled(format!("[{n}] "), theme::style_faint()),
                    Span::styled(pin, theme::style_muted()),
                    Span::styled(src.name.clone(), name_style),
                    Span::styled("  ", theme::style_faint()),
                    Span::styled(health, health_style),
                    if is_active {
                        Span::styled(" · last", theme::style_muted())
                    } else {
                        Span::raw("")
                    },
                ]));
                let detail = match src.driver {
                    ide_db::DbDriver::Sqlite => {
                        if src.file.is_empty() {
                            "sqlite".into()
                        } else {
                            src.file.clone()
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
                left.push(Line::from(Span::styled(
                    format!("      {}  {}", src.driver.label(), detail),
                    theme::style_faint(),
                )));
            }
        }
    } else {
        left.push(Line::from(Span::styled(
            "(workspace unavailable)",
            theme::style_faint(),
        )));
    }
    frame.render_widget(Paragraph::new(left), body[0]);

    let recents_active = ctx.ui.home_pane == HomePane::Recents;
    let recent_title = if recents_active {
        "▸ RECENT"
    } else {
        "RECENT"
    };
    let mut right: Vec<Line> = vec![
        Line::from(Span::styled(
            recent_title,
            theme::style_muted().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "────────────────────────────",
            theme::style_faint(),
        )),
        Line::from(""),
    ];
    if let Some(db) = ctx.db {
        if db.history.is_empty() {
            right.push(Line::from(Span::styled(
                "(no queries yet)",
                theme::style_faint(),
            )));
        } else {
            let rsel = ctx.ui.home_recent_selected;
            for (i, entry) in db.history.iter().rev().take(6).enumerate() {
                let selected = recents_active && i == rsel;
                let cursor = if selected { "› " } else { "  " };
                let style = if selected {
                    Style::default()
                        .fg(theme::text())
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    theme::style_muted()
                };
                right.push(Line::from(vec![
                    Span::styled(cursor, theme::style_selected()),
                    Span::styled(entry.label(40), style),
                ]));
            }
            if recents_active {
                right.push(Line::from(""));
                right.push(Line::from(Span::styled(
                    "Enter → Studio with SQL",
                    theme::style_faint(),
                )));
            }
        }

        right.push(Line::from(""));
        right.push(Line::from(Span::styled(
            "FAVORITES",
            theme::style_muted().add_modifier(Modifier::BOLD),
        )));
        right.push(Line::from(Span::styled(
            "────────────────────────────",
            theme::style_faint(),
        )));
        right.push(Line::from(""));

        let pinned: Vec<&str> = db
            .sources
            .iter()
            .filter(|s| s.pinned)
            .map(|s| s.name.as_str())
            .take(4)
            .collect();
        if pinned.is_empty() && db.bookmarks.is_empty() {
            right.push(Line::from(Span::styled(
                "(p pin source · bookmarks later)",
                theme::style_faint(),
            )));
        } else {
            for name in pinned {
                right.push(Line::from(vec![
                    Span::styled("★ ", theme::style_muted()),
                    Span::styled(name.to_string(), theme::style_text()),
                ]));
            }
            for bm in db.bookmarks.iter().take(4) {
                let label = if bm.chars().count() > 40 {
                    format!("{}…", bm.chars().take(39).collect::<String>())
                } else {
                    bm.clone()
                };
                right.push(Line::from(Span::styled(label, theme::style_faint())));
            }
        }
    }
    frame.render_widget(Paragraph::new(right), body[1]);

    let foot = vec![Line::from(vec![
        Span::styled("Tab", theme::style_text()),
        Span::styled(" pane  ·  ", theme::style_faint()),
        Span::styled("j/k", theme::style_text()),
        Span::styled("  ·  ", theme::style_faint()),
        Span::styled("Enter", theme::style_text()),
        Span::styled(" open  ·  ", theme::style_faint()),
        Span::styled("1-9", theme::style_text()),
        Span::styled("  ·  ", theme::style_faint()),
        Span::styled("a", theme::style_text()),
        Span::styled(" add  ·  ", theme::style_faint()),
        Span::styled("p", theme::style_text()),
        Span::styled(" pin  ·  ", theme::style_faint()),
        Span::styled("r", theme::style_text()),
        Span::styled(" health  ·  ", theme::style_faint()),
        Span::styled("q", theme::style_text()),
        Span::styled(" quit", theme::style_faint()),
    ])];
    frame.render_widget(Paragraph::new(foot), inner[2]);
}

fn render_db_home_status(frame: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled(
            " DB Home ",
            Style::default()
                .fg(theme::bg())
                .bg(theme::text())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  Tab pane  ·  j/k  ·  Enter open  ·  a add  ·  p pin  ·  r health  ·  q quit ",
            theme::style_faint(),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::bg())),
        area,
    );
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
        let body = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(root[1]);

        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(42), Constraint::Min(24)])
            .split(body[0]);

        // Explorer | Structure side-by-side.
        let left = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(top[0]);
        render_db_tree(frame, left[0], ctx.db, ctx.ui.focus == Focus::Database);
        render_db_structure(frame, left[1], ctx.db, ctx.ui.focus == Focus::Structure);
        render_sql_editor(frame, top[1], ctx);
        render_results_grid(frame, body[1], ctx);
    }

    // Zellij-style footer: spaced groups, one modifier prefix, · between keys.
    let conn = if let Some(db) = ctx.db {
        let src = db
            .active_source
            .as_ref()
            .and_then(|id| db.sources.iter().find(|s| &s.id == id));
        src.map(|s| s.name.as_str()).unwrap_or("—")
    } else {
        "—"
    };
    let meta = if let Some(db) = ctx.db {
        if db.query_running {
            "RUNNING… Esc cancel".into()
        } else if let Some(r) = &db.last_result {
            format!("{}×{}  {}ms", r.rows.len(), r.columns.len(), r.elapsed_ms)
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let pane = match ctx.ui.focus {
        Focus::Database => "1 explorer",
        Focus::Structure => "2 structure",
        Focus::Editor => "3 console",
        Focus::Results => "4 results",
        _ => "studio",
    };
    let zoom = if ctx.ui.db_fullscreen.is_some() {
        " FULL "
    } else {
        ""
    };

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(
        format!(" {pane} "),
        Style::default()
            .fg(theme::bg())
            .bg(theme::accent())
            .add_modifier(Modifier::BOLD),
    ));
    if !zoom.is_empty() {
        spans.push(Span::styled(
            zoom,
            Style::default()
                .fg(theme::bg())
                .bg(theme::text())
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::raw("  "));
    // Four studio panes
    push_zellij_group(&mut spans, "", &["Tab", "1", "2", "3", "4"]);
    spans.push(Span::styled("  │  ", theme::style_faint()));
    push_zellij_group(&mut spans, "", &["f"]);
    spans.push(Span::styled(" fullscreen  │  ", theme::style_faint()));

    match ctx.ui.focus {
        Focus::Database => {
            push_zellij_group(&mut spans, "", &["j", "k", "Enter"]);
            spans.push(Span::styled("  │  ", theme::style_faint()));
            push_zellij_group(&mut spans, "Ctrl+", &["Tab", "[", "]", "t", "x"]);
        }
        Focus::Structure => {
            push_zellij_group(&mut spans, "", &["j", "k"]);
            spans.push(Span::styled("  scroll  │  ", theme::style_faint()));
            push_zellij_group(&mut spans, "", &["f", "Esc"]);
        }
        Focus::Editor => {
            push_zellij_group(&mut spans, "", &["F5", "i"]);
            spans.push(Span::styled("  │  ", theme::style_faint()));
            push_zellij_group(&mut spans, "Ctrl+", &["Tab", "E", "S", "↵"]);
            spans.push(Span::styled("  │  ", theme::style_faint()));
            push_zellij_group(&mut spans, "", &["[", "]", "t", "x"]);
        }
        Focus::Results if ctx.ui.mode == Mode::EditCell => {
            push_zellij_group(&mut spans, "", &["Enter", "Esc"]);
            spans.push(Span::styled("  │  ", theme::style_faint()));
            push_zellij_group(&mut spans, "Ctrl+", &["S"]);
        }
        Focus::Results => {
            push_zellij_group(&mut spans, "", &["h", "j", "k", "l", "i", "y", "[", "]"]);
            spans.push(Span::styled("  edit/copy/pages  │  ", theme::style_faint()));
            push_zellij_group(&mut spans, "Ctrl+", &["S", "Tab"]);
        }
        _ => {}
    }

    spans.push(Span::styled("  │  ", theme::style_faint()));
    spans.push(Span::styled(conn.to_string(), theme::style_muted()));
    if !meta.is_empty() {
        spans.push(Span::styled("  ", theme::style_faint()));
        spans.push(Span::styled(meta, theme::style_faint()));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::bg())),
        root[2],
    );
}

/// Zellij-like key cluster: optional prefix + keys joined by middle dots.
fn push_zellij_group(spans: &mut Vec<Span<'static>>, prefix: &'static str, keys: &[&'static str]) {
    if !prefix.is_empty() {
        spans.push(Span::styled(prefix, theme::style_faint()));
    }
    for (i, key) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", theme::style_faint()));
        }
        spans.push(Span::styled(
            *key,
            theme::style_accent().add_modifier(Modifier::BOLD),
        ));
    }
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
