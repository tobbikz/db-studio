//! Command / file / projects palette with fuzzy matching.

use crate::theme;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaletteMode {
    #[default]
    Commands,
    Files,
    CodeActions,
    Projects,
    /// Home W — DB connection workspaces.
    Workspaces,
    /// Fuzzy jump to table/view rows visible in the DB explorer.
    DbObjects,
    /// Create menu (New query / schema / table / …).
    Create,
    /// Search Everywhere — tools + commands + DB objects.
    SearchEverywhere,
    /// Console toolbar: pick database (catalog).
    ConsoleDatabase,
    /// Console toolbar: pick schema (search path).
    ConsoleSchema,
    /// Results-focused DataGrip-like actions (export / copy / page / …).
    ResultsActions,
}

#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub label: String,
    pub id: String,
    pub path: Option<PathBuf>,
    /// Secondary line (path, hint) — shown muted under/beside label.
    pub subtitle: Option<String>,
}

#[derive(Debug, Default)]
pub struct Palette {
    pub mode: PaletteMode,
    pub query: String,
    pub items: Vec<PaletteItem>,
    pub filtered: Vec<usize>,
    pub selected: usize,
}

impl Palette {
    pub fn open_commands() -> Self {
        let items = vec![
            item("Open File", "open_file"),
            item("Open Folder", "open_folder"),
            item("Projects", "projects"),
            item("Save", "save"),
            item("Format Document", "format"),
            item("Rename Symbol", "rename"),
            item("Code Actions", "code_action"),
            item("Toggle Inlay Hints", "toggle_hints"),
            item("Project Search", "search"),
            item("Find in Buffer", "find"),
            item("Replace in Buffer", "replace"),
            item("Git Commit", "git_commit"),
            item("Git Push", "git_push"),
            item("Git Pull", "git_pull"),
            item("Database Mode", "database"),
            item("Run SQL Query", "run_query"),
            item("Explain SQL", "explain_query"),
            item("Apply Row Updates", "apply_updates"),
            item("Insert result row", "insert_row"),
            item("Delete result row…", "delete_row"),
            item("Toggle cell NULL", "toggle_cell_null"),
            item("View wide / blob cell", "view_wide_cell"),
            item("Paste TSV into results…", "paste_tsv"),
            item("Undo last cell edit", "undo_cell_edit"),
            item("Dry-run migration file", "migrate_dry_run"),
            item("Apply migration file…", "migrate_apply"),
            item("Migration status", "migrate_status"),
            item("Rollback last migration", "migrate_rollback"),
            item("Export Results CSV", "export_csv"),
            item("Export Results SQL INSERTs", "export_sql"),
            item("Export Results JSON", "export_json"),
            item("Export Results Markdown", "export_markdown"),
            item("Format SQL", "format_sql"),
            item("Schema diff → migration file", "schema_diff_migration"),
            item("DROP selected table…", "drop_table"),
            item("TRUNCATE selected table…", "truncate_table"),
            item("ALTER ADD column…", "alter_add_column"),
            item("ALTER DROP column…", "alter_drop_column"),
            item("ALTER RENAME column…", "alter_rename_column"),
            item("Fan-out SQL (pinned ★)", "fan_out_sql"),
            item("Export connection pack", "export_conn_pack"),
            item("Import connection pack", "import_conn_pack"),
            item("Find connection", "find_connection"),
            item("Pin / unpin SQL tab", "pin_tab"),
            item("Rename SQL tab", "rename_tab"),
            item("Close SQL tab", "close_tab"),
            item("New SQL tab", "new_tab"),
            item("Set console database…", "console_database"),
            item("Set console schema…", "console_schema"),
            item("Studio help", "studio_help"),
            item("Theme: AMOLED mono", "theme_black"),
            item("Theme: AMOLED light mono", "theme_white"),
            item("Theme: Dark", "theme_dark"),
            item("Theme: Nord", "theme_nord"),
            item("Theme: Catppuccin Mocha", "theme_catppuccin-mocha"),
            item("Theme: Tokyo Night", "theme_tokyo-night"),
            item("Theme: Dracula", "theme_dracula"),
            item("Theme: Light", "theme_light"),
            item("Theme: High contrast", "theme_high-contrast"),
            item("Theme: new custom…", "theme_new_custom"),
            item("Theme: reload customs", "theme_reload"),
            item("Onboarding tips", "onboarding_tips"),
            item("Recipe gallery", "recipe_gallery"),
            item("Recipe: top customers", "recipe_top_customers"),
            item("Recipe: revenue by day", "recipe_revenue_by_day"),
            item("Recipe: stratified sample", "recipe_stratified_sample"),
            item("Recipe: embedding similarity", "recipe_embedding_similarity"),
            item("Recipe: feature table", "recipe_feature_table"),
            item("Recipe: batch score", "recipe_batch_score"),
            item("Chart results (bar)", "chart_bar"),
            item("Chart results (line)", "chart_line"),
            item("Chart results (pie)", "chart_pie"),
            item("Export result pack (SQL+CSV)", "export_result_pack"),
            item("Quality strip (null%/stats)", "quality_strip"),
            item("Toggle mask PII", "toggle_mask_pii"),
            item("Privilege audit (role…)", "privilege_audit"),
            item("Export audit log CSV", "export_audit_csv"),
            item("Diagnose pack (support)", "diagnose_pack"),
            item("Column lineage sketch", "lineage_sketch"),
            item("CDC / replication lag", "cdc_lag"),
            item("dbt detect project", "dbt_detect"),
            item("dbt run (Agents recipe)", "dbt_run"),
            item("dbt test (Agents recipe)", "dbt_test"),
            item("dbt compile (Agents recipe)", "dbt_compile"),
            item("dbt schema.yml stubs", "dbt_schema_stubs"),
            item("Airflow DAGs deep-link", "airflow_link"),
            item("BI deep-link (Superset/Metabase)", "bi_deeplink"),
            item("BI export CSV path", "bi_export_csv"),
            item("Model registry deep-link", "model_registry_link"),
            item("Codegen Rust struct", "codegen_rust"),
            item("Codegen TypeScript interface", "codegen_ts"),
            item("Seed fixtures from testdata/", "seed_fixtures"),
            item("Golden CSV diff vs results", "golden_diff"),
            item("Fan-out rowcount assert", "fanout_assert"),
            item("EXPLAIN cost / bytes", "explain_cost"),
            item("Schema ADR stub note", "schema_adr_stub"),
            item("Naming hint (logical→physical)", "naming_hint"),
            item("Health webhook test", "health_webhook"),
            item("Signed connection pack export", "signed_conn_pack"),
            item("Agents: ask→SQL preset seed", "agents_ask_sql"),
            item("Code help", "code_help"),
            item("Yank Neovim → host clipboard", "yank_clipboard"),
            item("Paste host clipboard → editor", "paste_clipboard"),
            item("Script object / Go to DDL", "script_object"),
            item("Create… (New)", "create_menu"),
            item("Search Everywhere", "search_everywhere"),
            item("Go to DDL", "script_object"),
            item("Query history", "query_history"),
            item("Pin / unpin connection", "pin_connection"),
            item("Health overview (active driver)", "health_overview"),
            item("Health activity (active driver)", "health_activity"),
            item("Toggle Health Activity auto-refresh", "health_activity_auto_refresh"),
            item("MSSQL Health overview", "mssql_health_overview"),
            item("MSSQL Health activity", "mssql_health_activity"),
            item("MSSQL Health blocking", "mssql_health_blocking"),
            item("MSSQL Health waits", "mssql_health_waits"),
            item("MSSQL Health expensive", "mssql_health_expensive"),
            item("MSSQL Health I/O", "mssql_health_io"),
            item("MSSQL Agent Jobs", "mssql_health_jobs"),
            item("MSSQL Index fragmentation", "mssql_health_indexes"),
            item("Chart results", "chart_results"),
            item("Results next page", "result_page_next"),
            item("Results prev page", "result_page_prev"),
            item("Filter results (/)", "filter_results"),
            item("Toggle results density", "toggle_results_density"),
            item("Pin / unpin result columns", "toggle_pin_columns"),
            item("ER sketch (FKs)", "er_sketch"),
            item("ER canvas (boxes)", "er_canvas"),
            item("Refresh connection health", "refresh_health"),
            item("Recent tables", "recent_tables"),
            item("Insert SQL snippet", "insert_snippet"),
            item("OAuth: open browser", "oauth_open"),
            item("OAuth: paste token", "oauth_paste"),
            item("Save result as temp table", "save_result_temp"),
            item("Save result as view", "save_result_view"),
            item("Brain ask (RAG)", "brain_ask"),
            item("Cancel running query", "cancel_query"),
            item("Import CSV into table", "import_csv"),
            item("Import JSON into table", "import_json"),
            item("Dump database", "dump_database"),
            item("Restore SQL dump", "restore_dump"),
            item("Schema diff (left..right)", "schema_diff"),
            item("Schema diff → DDL patch", "schema_diff_ddl"),
            item("Data compare (left..right table)", "data_compare"),
            item("Begin transaction", "txn_begin"),
            item("Commit transaction", "txn_commit"),
            item("Rollback transaction", "txn_rollback"),
            item("Kill session (results row)", "kill_session"),
            item("Bookmark current SQL", "bookmark_sql"),
            item("Save SQL to queries/", "save_query"),
            item("Sync queries push (git)", "sync_queries_push"),
            item("Sync queries pull (git)", "sync_queries_pull"),
            item("Open Settings", "settings"),
            item("Add Data Source", "add_datasource"),
            item("Edit connection", "edit_connection"),
            item("Duplicate connection", "duplicate_connection"),
            item("Delete connection", "delete_connection"),
            item("Maintain: vacuum/optimize/rebuild", "vacuum_analyze"),
            item("Maintain: analyze/statistics", "analyze_table"),
            item("Maintain: reindex/rebuild", "reindex_table"),
            item("Inspect table privileges", "inspect_privileges"),
            item("List RLS policies (Postgres)", "inspect_policies"),
            item("Timing histogram", "timing_histogram"),
            item("Find table/view (explorer)", "db_object_search"),
            item("Add Brain (Postgres)", "add_brain"),
            item("Git", "git"),
            item("Outline", "outline"),
            item("Terminal", "terminal"),
            item("Sessions (SSH/RDP)", "sessions"),
            item("Agents (Cursor/Claude/…)", "agents"),
            item("Debug", "debug"),
            item("Split Editor", "split"),
            item("Close Tab", "close_tab"),
            item("Toggle File Tree", "toggle_tree"),
            item("Goto Definition", "goto_def"),
            item("Hover", "hover"),
            item("Completions", "completion"),
            item("Next Problem", "next_problem"),
            item("Quit", "quit"),
        ];
        let mut p = Self {
            mode: PaletteMode::Commands,
            query: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
        };
        p.refilter();
        p
    }

    pub fn open_files(files: &[PathBuf], root: &std::path::Path) -> Self {
        let items: Vec<PaletteItem> = files
            .iter()
            .map(|p| {
                let rel = p.strip_prefix(root).unwrap_or(p);
                let name = rel
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("file")
                    .to_string();
                let parent = rel
                    .parent()
                    .map(|d| d.display().to_string())
                    .filter(|s| !s.is_empty() && s != ".");
                PaletteItem {
                    id: "file".into(),
                    label: name,
                    path: Some(p.clone()),
                    subtitle: parent,
                }
            })
            .collect();
        let mut p = Self {
            mode: PaletteMode::Files,
            query: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
        };
        p.refilter();
        p
    }

    pub fn open_projects(paths: &[String]) -> Self {
        let items: Vec<PaletteItem> = if paths.is_empty() {
            vec![PaletteItem {
                id: "open_folder".into(),
                label: "Open a folder".into(),
                path: None,
                subtitle: Some("no recent projects yet".into()),
            }]
        } else {
            paths
                .iter()
                .map(|p| {
                    let name = std::path::Path::new(p)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(p.as_str())
                        .to_string();
                    PaletteItem {
                        id: "project".into(),
                        label: name,
                        path: Some(PathBuf::from(p)),
                        subtitle: Some(p.clone()),
                    }
                })
                .collect()
        };
        let mut p = Self {
            mode: PaletteMode::Projects,
            query: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
        };
        p.refilter();
        p
    }

    pub fn open_db_objects(items: Vec<(String, String, String)>) -> Self {
        let items: Vec<PaletteItem> = items
            .into_iter()
            .map(|(label, subtitle, flat_idx)| PaletteItem {
                label,
                id: format!("dbobj:{flat_idx}"),
                path: None,
                subtitle: Some(subtitle),
            })
            .collect();
        let mut p = Self {
            mode: PaletteMode::DbObjects,
            query: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
        };
        p.refilter();
        p
    }

    pub fn open_actions(actions: Vec<(String, String)>) -> Self {
        let items = actions
            .into_iter()
            .map(|(label, id)| PaletteItem {
                label,
                id,
                path: None,
                subtitle: None,
            })
            .collect();
        let mut p = Self {
            mode: PaletteMode::CodeActions,
            query: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
        };
        p.refilter();
        p
    }

    /// Curated Results data actions (export, copy, edit, page, materialize).
    pub fn open_results_actions() -> Self {
        let items = vec![
            item("Export CSV", "export_csv"),
            item("Export JSON", "export_json"),
            item("Export SQL INSERTs", "export_sql"),
            item("Export Markdown", "export_markdown"),
            item("Export result pack (SQL+CSV)", "export_result_pack"),
            item("Copy cell (y)", "yank_result_cell"),
            item("Copy row (TSV)", "yank_result_row"),
            item("Copy all as TSV", "yank_result_all"),
            item("Filter results (/)", "filter_results"),
            item("Edit WHERE (w)", "edit_table_where"),
            item("Edit ORDER BY (o)", "edit_table_order"),
            item("Next page (])", "result_page_next"),
            item("Prev page ([)", "result_page_prev"),
            item("Toggle density (D)", "toggle_results_density"),
            item("Pin / unpin columns", "toggle_pin_columns"),
            item("Apply row updates (a)", "apply_updates"),
            item("Insert result row", "insert_row"),
            item("Delete result row…", "delete_row"),
            item("Toggle cell NULL", "toggle_cell_null"),
            item("Paste TSV into results…", "paste_tsv"),
            item("Undo last cell edit (u)", "undo_cell_edit"),
            item("Save as temp table", "save_result_temp"),
            item("Save as view", "save_result_view"),
            item("Chart results", "chart_results"),
            item("Import CSV into table", "import_csv"),
            item("Import JSON into table", "import_json"),
            item("Quality strip (null%/stats)", "quality_strip"),
            item("Toggle mask PII", "toggle_mask_pii"),
        ];
        let mut p = Self {
            mode: PaletteMode::ResultsActions,
            query: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
        };
        p.refilter();
        p
    }

    pub fn open_workspaces(actions: Vec<(String, String)>) -> Self {
        let items = actions
            .into_iter()
            .map(|(label, id)| PaletteItem {
                label,
                id,
                path: None,
                subtitle: None,
            })
            .collect();
        let mut p = Self {
            mode: PaletteMode::Workspaces,
            query: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
        };
        p.refilter();
        p
    }

    pub fn open_create() -> Self {
        Self::open_create_for(ide_db::DbDriver::Postgres)
    }

    /// Create menu filtered by what the active driver supports.
    pub fn open_create_for(driver: ide_db::DbDriver) -> Self {
        let mut items = vec![item("Query Console", "create_query")];
        for (label, id) in crate::create_dialect::create_menu_ids(driver) {
            items.push(item(label, id));
        }
        items.push(item("Data Source…", "add_datasource"));
        let mut p = Self {
            mode: PaletteMode::Create,
            query: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
        };
        p.refilter();
        p
    }

    /// Tools first, then optional DB objects, then remaining commands.
    pub fn open_search_everywhere(
        db_objects: Vec<(String, String, String)>,
    ) -> Self {
        let mut items = vec![
            item("Git", "git"),
            item("Terminal", "terminal"),
            item("Debug", "debug"),
            item("Hierarchy (Outline)", "outline"),
            item("Project Search", "search"),
            item("Go to DDL", "script_object"),
            item("Create…", "create_menu"),
            item("Find table/view", "db_object_search"),
            item("New Query", "new_tab"),
            item("Set console database…", "console_database"),
            item("Set console schema…", "console_schema"),
        ];
        for (label, subtitle, flat_idx) in db_objects {
            items.push(PaletteItem {
                label,
                id: format!("dbobj:{flat_idx}"),
                path: None,
                subtitle: Some(subtitle),
            });
        }
        // Append a trimmed command set (skip duplicates already listed).
        for it in Self::open_commands().items {
            if items.iter().any(|e| e.id == it.id) {
                continue;
            }
            items.push(it);
        }
        let mut p = Self {
            mode: PaletteMode::SearchEverywhere,
            query: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
        };
        p.refilter();
        p
    }

    pub fn open_console_databases(names: Vec<String>) -> Self {
        let items = names
            .into_iter()
            .map(|name| PaletteItem {
                id: format!("condb:{name}"),
                label: name,
                path: None,
                subtitle: Some("database".into()),
            })
            .collect();
        let mut p = Self {
            mode: PaletteMode::ConsoleDatabase,
            query: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
        };
        p.refilter();
        p
    }

    pub fn open_console_schemas(names: Vec<String>) -> Self {
        let items = names
            .into_iter()
            .map(|name| PaletteItem {
                id: format!("conschema:{name}"),
                label: name,
                path: None,
                subtitle: Some("schema".into()),
            })
            .collect();
        let mut p = Self {
            mode: PaletteMode::ConsoleSchema,
            query: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
        };
        p.refilter();
        p
    }

    fn haystack(it: &PaletteItem) -> String {
        match &it.subtitle {
            Some(s) => format!("{} {}", it.label, s),
            None => it.label.clone(),
        }
    }

    pub fn refilter(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.items.len()).collect();
        } else {
            let mut matcher = Matcher::new(Config::DEFAULT);
            let pattern = Pattern::new(
                &self.query,
                CaseMatching::Ignore,
                Normalization::Smart,
                AtomKind::Fuzzy,
            );
            let mut scored: Vec<(usize, u32)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| {
                    let text = Self::haystack(it);
                    let mut buf = Vec::new();
                    let hay = Utf32Str::new(&text, &mut buf);
                    pattern.score(hay, &mut matcher).map(|score| (i, score))
                })
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered = scored.into_iter().map(|(i, _)| i).collect();
        }
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    pub fn selected_item(&self) -> Option<&PaletteItem> {
        self.filtered
            .get(self.selected)
            .and_then(|i| self.items.get(*i))
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        }
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.refilter();
    }

    pub fn append_items(&mut self, extra: Vec<(String, String)>) {
        for (label, id) in extra {
            self.items.push(PaletteItem {
                label,
                id,
                path: None,
                subtitle: None,
            });
        }
        self.refilter();
    }
}

fn item(label: &str, id: &str) -> PaletteItem {
    PaletteItem {
        label: label.into(),
        id: id.into(),
        path: None,
        subtitle: None,
    }
}

pub fn render_palette(frame: &mut Frame, screen: Rect, palette: &Palette) {
    // Full-screen black scrim — same plate language as Home / connect modal.
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::bg())),
        screen,
    );

    let width = screen.width.min(64).max(40);
    let height = screen.height.min(18).max(10);
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 4;
    let area = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, area);

    let title = match palette.mode {
        PaletteMode::Commands => " Commands ",
        PaletteMode::Files => " Open file ",
        PaletteMode::CodeActions => " Actions ",
        PaletteMode::Projects => " Projects ",
        PaletteMode::Workspaces => " Workspaces ",
        PaletteMode::DbObjects => " Find object ",
        PaletteMode::Create => " New ",
        PaletteMode::SearchEverywhere => " Search everywhere ",
        PaletteMode::ConsoleDatabase => " Database ",
        PaletteMode::ConsoleSchema => " Schema ",
        PaletteMode::ResultsActions => " Results · actions ",
    };

    frame.render_widget(
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
        area,
    );

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // filter
            Constraint::Length(1), // blank
            Constraint::Min(4),    // list
            Constraint::Length(1), // blank
            Constraint::Length(1), // footer
        ])
        .margin(2)
        .split(area);

    let filter_text = if palette.query.is_empty() {
        Line::from(Span::styled("type to filter…", theme::style_faint()))
    } else {
        Line::from(vec![
            Span::styled(
                palette.query.as_str(),
                Style::default()
                    .fg(theme::text())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("▌", theme::style_muted()),
        ])
    };
    frame.render_widget(
        Paragraph::new(filter_text).style(Style::default().bg(theme::bg())),
        inner[0],
    );

    let list_h = inner[2].height as usize;
    let start = palette
        .selected
        .saturating_sub(list_h.saturating_sub(1) / 2)
        .min(palette.filtered.len().saturating_sub(list_h));
    let mut lines: Vec<Line> = Vec::new();
    let label_w = (width as usize).saturating_sub(6).max(12);
    for (vis_off, &idx) in palette.filtered.iter().enumerate().skip(start).take(list_h) {
        let it = &palette.items[idx];
        let selected = vis_off == palette.selected;
        let name_style = if selected {
            theme::style_selected()
        } else {
            theme::style_text()
        };
        let label = if let Some(sub) = &it.subtitle {
            truncate(&format!("{}  ·  {sub}", it.label), label_w)
        } else {
            truncate(&it.label, label_w)
        };
        lines.push(Line::from(Span::styled(format!(" {label} "), name_style)));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            " No matches — clear filter or Esc ",
            theme::style_faint().add_modifier(Modifier::ITALIC),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner[2]);

    let theme_row = palette
        .selected_item()
        .is_some_and(|it| {
            it.id.starts_with("theme_") && it.id != "theme_new_custom" && it.id != "theme_reload"
        });
    let hint = if theme_row {
        Line::from(vec![
            Span::styled(
                " ↑↓ ",
                Style::default()
                    .fg(theme::bg())
                    .bg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" preview  ·  ", theme::style_faint()),
            Span::styled(
                " Enter ",
                Style::default()
                    .fg(theme::bg())
                    .bg(theme::text())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" keep  ·  Esc restore ", theme::style_faint()),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " Enter ",
                Style::default()
                    .fg(theme::bg())
                    .bg(theme::text())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" open  ·  Esc close ", theme::style_faint()),
        ])
    };
    frame.render_widget(Paragraph::new(hint), inner[4]);
}

/// Centered dump restore path prompt.
pub fn render_restore_dump(frame: &mut Frame, screen: Rect, input: &str) {
    render_path_prompt(
        frame,
        screen,
        " restore dump ",
        "  path to .sql dump (pg_dump / mysqldump / sqlite3)",
        "  restores into the active database",
        input,
    );
}

/// Schema compare prompt: `left..right` (`schema` or `database.schema`).
pub fn render_schema_diff(frame: &mut Frame, screen: Rect, input: &str) {
    render_path_prompt(
        frame,
        screen,
        " schema diff ",
        "  left..right  (schema or database.schema)",
        "  + only right · - only left · ~ type/null drift",
        input,
    );
}

pub fn render_data_compare(frame: &mut Frame, screen: Rect, input: &str) {
    render_path_prompt(
        frame,
        screen,
        " data compare ",
        "  left..right  (schema.table or db.schema.table)",
        "  compares up to 500 rows · + / - / ~",
        input,
    );
}

pub fn render_query_params(frame: &mut Frame, screen: Rect, input: &str) {
    render_path_prompt(
        frame,
        screen,
        " query params ",
        "  name=foo,1=42  (:name / @name / $n)",
        "  values SQL-escaped for this run only · NULL = null",
        input,
    );
}

pub fn render_alter_column(frame: &mut Frame, screen: Rect, input: &str, kind: &str) {
    let (title, hint1, hint2) = match kind {
        "drop" => (
            " ALTER DROP column ",
            "  column_name",
            "  confirm before run · select table first",
        ),
        "rename" => (
            " ALTER RENAME column ",
            "  old_name=new_name",
            "  confirm before run · select table first",
        ),
        _ => (
            " ALTER ADD column ",
            "  column_name TYPE   (e.g. email TEXT)",
            "  confirm before run · select table first",
        ),
    };
    render_path_prompt(frame, screen, title, hint1, hint2, input);
}

pub fn render_save_materialize(frame: &mut Frame, screen: Rect, input: &str, as_temp: bool) {
    let kind = if as_temp { "temp table" } else { "view" };
    render_path_prompt(
        frame,
        screen,
        &format!(" save as {kind} "),
        "  identifier (letters, digits, underscore)",
        "  uses last SELECT SQL · PG/SQLite/DuckDB",
        input,
    );
}

pub fn render_session_form(
    frame: &mut Frame,
    screen: Rect,
    mgr: &ide_sessions::SessionManager,
    field: usize,
) {
    let kind = if mgr.form_kind_ssh { "SSH" } else { "RDP" };
    let edit = if mgr.form_edit_id.is_some() {
        "edit"
    } else {
        "add"
    };
    let fields = [
        ("name", mgr.form_name.as_str()),
        ("host", mgr.form_host.as_str()),
        ("user", mgr.form_user.as_str()),
        ("port", mgr.form_port.as_str()),
        (
            if mgr.form_kind_ssh {
                "identity_file"
            } else {
                "gateway"
            },
            if mgr.form_kind_ssh {
                mgr.form_identity.as_str()
            } else {
                mgr.form_gateway.as_str()
            },
        ),
        (
            if mgr.form_kind_ssh {
                "jump_host"
            } else {
                "(unused)"
            },
            mgr.form_jump.as_str(),
        ),
    ];
    let mut body = format!("  {edit} {kind} profile · Tab fields · Enter save · Esc cancel\n");
    for (i, (label, val)) in fields.iter().enumerate() {
        let mark = if i == field { ">" } else { " " };
        body.push_str(&format!("{mark} {label}: {val}\n"));
    }
    body.push_str("\n  no passwords stored — ssh-agent / -i key only");
    render_path_prompt(
        frame,
        screen,
        " session profile ",
        body.lines().nth(0).unwrap_or(""),
        &body,
        "",
    );
}

pub fn render_agent_form(
    frame: &mut Frame,
    screen: Rect,
    mgr: &ide_agents::AgentManager,
    field: usize,
) {
    let edit = if mgr.form_edit_id.is_some() {
        "edit"
    } else {
        "add"
    };
    let fields = [
        ("name", mgr.form_name.as_str()),
        ("kind (cursor|claude|opencode|custom)", mgr.form_kind.as_str()),
        ("binary override", mgr.form_binary.as_str()),
        ("extra args", mgr.form_args.as_str()),
        ("seed prompt", mgr.form_seed.as_str()),
    ];
    let mut body = format!("  {edit} agent · Tab fields · Enter save · Esc cancel\n");
    for (i, (label, val)) in fields.iter().enumerate() {
        let mark = if i == field { ">" } else { " " };
        body.push_str(&format!("{mark} {label}: {val}\n"));
    }
    body.push_str("\n  no API keys in TOML — use env (CURSOR_API_KEY, etc.)");
    render_path_prompt(
        frame,
        screen,
        " agent profile ",
        body.lines().nth(0).unwrap_or(""),
        &body,
        "",
    );
}

pub fn render_brain_ask(frame: &mut Frame, screen: Rect, input: &str) {
    render_path_prompt(
        frame,
        screen,
        " Brain ask ",
        "  natural-language question",
        "  uses brain/rag CLI or BRAIN_DSN · brain.consult",
        input,
    );
}

/// Centered CSV/JSON import path prompt.
pub fn render_import_csv(frame: &mut Frame, screen: Rect, input: &str) {
    let json = input.trim().ends_with(".json")
        || input.to_ascii_lowercase().contains("json");
    if json {
        render_path_prompt(
            frame,
            screen,
            " Import JSON ",
            "  path to JSON array / NDJSON of objects",
            "  target = open table result or explorer selection",
            input,
        );
    } else {
        render_path_prompt(
            frame,
            screen,
            " Import CSV ",
            "  path to CSV (header row required)",
            "  target = open table result or explorer selection",
            input,
        );
    }
}

fn render_path_prompt(
    frame: &mut Frame,
    screen: Rect,
    title: &str,
    hint1: &str,
    hint2: &str,
    input: &str,
) {
    let width = screen.width.min(72).max(44);
    let height = 9;
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 3;
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::border_idle()))
            .style(Style::default().bg(theme::surface()))
            .title(title.to_string()),
        area,
    );
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .margin(1)
        .split(area);
    frame.render_widget(
        Paragraph::new(Span::styled(hint1, theme::style_faint())),
        inner[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(hint2, theme::style_faint())),
        inner[1],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!("  {input}▌"),
            theme::style_text().add_modifier(Modifier::BOLD),
        )),
        inner[3],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "  Enter confirm · Esc cancel",
            theme::style_muted(),
        )),
        inner[5],
    );
}

/// Centered folder path prompt (Open folder).
pub fn render_open_folder(frame: &mut Frame, screen: Rect, input: &str) {
    let width = screen.width.min(64).max(40);
    let height = 9;
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 3;
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::border_idle()))
            .style(Style::default().bg(theme::surface()))
            .title(" Open folder "),
        area,
    );
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .margin(1)
        .split(area);
    frame.render_widget(
        Paragraph::new(Span::styled(
            "  absolute or ~/path to a directory",
            theme::style_muted(),
        )),
        inner[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "  ─────────────────────────────────────────────",
            theme::style_faint(),
        )),
        inner[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  path  ", theme::style_faint()),
            Span::styled(
                input,
                Style::default()
                    .fg(theme::text())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("▌", theme::style_muted()),
        ]))
        .style(Style::default().bg(theme::surface_2())),
        inner[2],
    );
    frame.render_widget(Paragraph::new(""), inner[3]);
    frame.render_widget(
        Paragraph::new(Span::styled(
            "  ─────────────────────────────────────────────",
            theme::style_faint(),
        )),
        inner[4],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  ", theme::style_faint()),
            Span::styled("Enter", theme::style_pill_off()),
            Span::styled(" open   ", theme::style_faint()),
            Span::styled("Esc", theme::style_pill_off()),
            Span::styled(" cancel", theme::style_faint()),
        ])),
        inner[5],
    );
}

/// Centered settings editor (mono, same family as other modals).
pub fn render_settings_modal(frame: &mut Frame, screen: Rect, draft: &str) {
    let width = screen.width.min(78).max(48);
    let height = screen.height.min(20).max(12);
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 4;
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::border_idle()))
            .style(Style::default().bg(theme::surface()))
            .title(" Settings "),
        area,
    );
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .margin(1)
        .split(area);
    frame.render_widget(
        Paragraph::new(Span::styled(
            "  ~/.config/tui-ide/config.toml",
            theme::style_muted(),
        )),
        inner[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "  ─────────────────────────────────────────────────────────",
            theme::style_faint(),
        )),
        inner[1],
    );
    let lines: Vec<Line> = draft
        .lines()
        .take(inner[2].height as usize)
        .map(|l| Line::from(Span::styled(format!("  {l}"), theme::style_text())))
        .collect();
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme::surface_2())),
        inner[2],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "  ─────────────────────────────────────────────────────────",
            theme::style_faint(),
        )),
        inner[3],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  type to edit · ", theme::style_faint()),
            Span::styled("Enter", theme::style_pill_off()),
            Span::styled(" save   ", theme::style_faint()),
            Span::styled("Esc", theme::style_pill_off()),
            Span::styled(" cancel", theme::style_faint()),
        ])),
        inner[4],
    );
}

/// Yes/no confirm for destructive actions (delete connection / apply UPDATEs).
pub fn render_confirm_delete(frame: &mut Frame, screen: Rect, title: &str, message: &str) {
    render_confirm_dialog(frame, screen, title, message, "delete");
}

/// Blocking error ack (query failure). Enter / Esc dismiss.
pub fn render_error_dialog(frame: &mut Frame, screen: Rect, title: &str, message: &str) {
    frame.render_widget(Clear, screen);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::bg())),
        screen,
    );

    let width = screen.width.min(72).max(44);
    let wrapped_lines = textwrap_lines(message, (width as usize).saturating_sub(6).max(20));
    let body_h = (wrapped_lines.len() as u16).clamp(2, 10);
    let height = body_h.saturating_add(8).min(screen.height.saturating_sub(2));
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 3;
    let area = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, area);
    let title_style = Style::default()
        .fg(theme::bg())
        .bg(theme::danger())
        .add_modifier(Modifier::BOLD);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::danger()))
            .style(Style::default().bg(theme::bg()))
            .title(Span::styled(format!(" {title} "), title_style)),
        area,
    );
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(2),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .margin(2)
        .split(area);
    let body: Vec<Line> = wrapped_lines
        .into_iter()
        .map(|l| Line::from(Span::styled(l, theme::style_text())))
        .collect();
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inner[1]);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " Enter ",
                Style::default()
                    .fg(theme::bg())
                    .bg(theme::text())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" / ", theme::style_faint()),
            Span::styled(" Esc ", theme::style_pill_off()),
            Span::styled(" dismiss", theme::style_faint()),
        ])),
        inner[3],
    );
}

fn textwrap_lines(s: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for para in s.split('\n') {
        if para.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in para.split_whitespace() {
            // Hard-break overlong tokens so 100-col modals never blow the frame.
            let chunks = if word.chars().count() > width {
                let chars: Vec<char> = word.chars().collect();
                chars
                    .chunks(width)
                    .map(|c| c.iter().collect::<String>())
                    .collect::<Vec<_>>()
            } else {
                vec![word.to_string()]
            };
            for chunk in chunks {
                if line.is_empty() {
                    line = chunk;
                } else if line.chars().count() + 1 + chunk.chars().count() <= width {
                    line.push(' ');
                    line.push_str(&chunk);
                } else {
                    out.push(std::mem::take(&mut line));
                    line = chunk;
                }
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Changes panel: pending UPDATE/INSERT/DELETE SQL before Apply (DataRig-style).
pub fn render_changes_preview(frame: &mut Frame, screen: Rect, preview: &str) {
    frame.render_widget(Clear, screen);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::bg())),
        screen,
    );

    let width = screen.width.min(88).max(52);
    let sql_lines: Vec<&str> = preview.lines().collect();
    let body_h = (sql_lines.len() as u16).clamp(4, 16);
    let height = body_h.saturating_add(8).min(screen.height.saturating_sub(2));
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 3;
    let area = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, area);
    let title_style = Style::default()
        .fg(theme::bg())
        .bg(theme::text())
        .add_modifier(Modifier::BOLD);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::text()))
            .style(Style::default().bg(theme::bg()))
            .title(Span::styled(" Changes · SQL preview ", title_style)),
        area,
    );
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .margin(2)
        .split(area);

    frame.render_widget(
        Paragraph::new(Span::styled(
            " Pending mutations — review before Apply ",
            theme::style_muted(),
        )),
        inner[0],
    );

    let max_w = (width as usize).saturating_sub(6).max(20);
    let body: Vec<Line> = sql_lines
        .into_iter()
        .map(|l| {
            let shown = if l.chars().count() > max_w {
                format!("{}…", l.chars().take(max_w.saturating_sub(1)).collect::<String>())
            } else {
                l.to_string()
            };
            let style = if shown.starts_with("UPDATE")
                || shown.starts_with("INSERT")
                || shown.starts_with("DELETE")
            {
                theme::style_accent()
            } else if shown.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                theme::style_text()
            } else {
                theme::style_muted()
            };
            Line::from(Span::styled(format!(" {shown}"), style))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(body)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(theme::border_idle()))
                    .style(Style::default().bg(theme::surface())),
            )
            .wrap(Wrap { trim: false }),
        inner[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " Enter ",
                Style::default()
                    .fg(theme::bg())
                    .bg(theme::text())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" apply   ", theme::style_faint()),
            Span::styled(" Esc ", theme::style_pill_off()),
            Span::styled(" cancel", theme::style_faint()),
        ])),
        inner[3],
    );
}

pub fn render_confirm_dialog(
    frame: &mut Frame,
    screen: Rect,
    title: &str,
    message: &str,
    action_verb: &str,
) {
    // Same AMOLED black as Home / connect form (border-only elevation).
    frame.render_widget(Clear, screen);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::bg())),
        screen,
    );

    let width = screen.width.min(58).max(42);
    let height = 12u16;
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 3;
    let area = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, area);
    let title_style = Style::default()
        .fg(theme::bg())
        .bg(theme::text())
        .add_modifier(Modifier::BOLD);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::text()))
            .style(Style::default().bg(theme::bg()))
            .title(Span::styled(format!(" {title} "), title_style)),
        area,
    );
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(2),
        ])
        .margin(2)
        .split(area);
    let msg_w = (width as usize).saturating_sub(6).max(20);
    let msg_lines = textwrap_lines(message, msg_w);
    frame.render_widget(
        Paragraph::new(
            msg_lines
                .into_iter()
                .map(|l| Line::from(Span::styled(l, theme::style_text().add_modifier(Modifier::BOLD))))
                .collect::<Vec<_>>(),
        ),
        inner[0],
    );
    let hint = if action_verb == "apply" {
        "Runs the listed UPDATE statements against the active source."
    } else if action_verb.contains("save") {
        "Unsaved buffers will be handled as chosen."
    } else {
        "Removes the entry from datasources.toml."
    };
    frame.render_widget(
        Paragraph::new(Span::styled(hint, theme::style_muted())),
        inner[2],
    );
    // One key per action (y/n still work; not advertised).
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " Enter ",
                Style::default()
                    .fg(theme::bg())
                    .bg(theme::text())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {action_verb}   "), theme::style_faint()),
            Span::styled(" Esc ", theme::style_pill_off()),
            Span::styled(" cancel", theme::style_faint()),
        ])),
        inner[4],
    );
}

fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else if max <= 1 {
        "…".into()
    } else {
        let take = max - 1;
        let mut out: String = s.chars().take(take).collect();
        out.push('…');
        out
    }
}
