//! Interactive DataGrip/DBeaver-style connection form.

use crate::theme;
use ide_db::{DataSource, DbDriver, SourceRole, password_display_hint};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnField {
    Name,
    Driver,
    Host,
    Port,
    User,
    Password,
    Database,
    File,
    SslMode,
    SslTrustCert,
    SslCa,
    ReadOnly,
    Role,
    SshHost,
    SshUser,
    SshPort,
    SshKey,
    Group,
    Accent,
}

impl ConnField {
    /// Primary connect fields first; cosmetics / TLS / SSH after.
    pub const ALL: [ConnField; 19] = [
        Self::Name,
        Self::Driver,
        Self::Host,
        Self::Port,
        Self::User,
        Self::Password,
        Self::Database,
        Self::File,
        Self::ReadOnly,
        Self::Role,
        Self::Group,
        Self::Accent,
        Self::SslMode,
        Self::SslTrustCert,
        Self::SslCa,
        Self::SshHost,
        Self::SshUser,
        Self::SshPort,
        Self::SshKey,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Group => "Group",
            Self::Accent => "Accent",
            Self::Driver => "Driver",
            Self::Host => "Host",
            Self::Port => "Port",
            Self::User => "User",
            Self::Password => "Password",
            Self::Database => "Database",
            Self::File => "File",
            Self::SslMode => "SSL mode",
            Self::SslTrustCert => "Trust cert",
            Self::SslCa => "SSL CA",
            Self::ReadOnly => "Read-only",
            Self::Role => "Role",
            Self::SshHost => "SSH host",
            Self::SshUser => "SSH user",
            Self::SshPort => "SSH port",
            Self::SshKey => "SSH key",
        }
    }

    pub fn hint(self, driver: DbDriver) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Group => "optional",
            Self::Accent => "← → color",
            Self::Driver => "← →",
            Self::Host => "host",
            Self::Port => "port",
            Self::User => "user",
            Self::Password => "password · env:VAR",
            Self::Database => "database",
            Self::File => {
                if driver == DbDriver::Sqlite {
                    "path.db"
                } else {
                    "—"
                }
            }
            Self::SslMode => "← →",
            Self::SslTrustCert => "← →",
            Self::SslCa => "optional",
            Self::ReadOnly => "← →",
            Self::Role => "← →",
            Self::SshHost => "optional",
            Self::SshUser => "optional",
            Self::SshPort => "22",
            Self::SshKey => "optional",
        }
    }

    fn next(self) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    fn prev(self) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone)]
pub struct DbConnectForm {
    pub is_edit: bool,
    pub name: String,
    pub group: String,
    pub accent: String,
    pub driver: DbDriver,
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: String,
    pub database: String,
    pub file: String,
    pub ssl_mode: String,
    pub ssl_trust_cert: bool,
    pub ssl_ca: String,
    pub read_only: bool,
    pub role: SourceRole,
    /// Source id when editing — for vault hint without replaying secrets.
    pub source_id: String,
    pub ssh_host: String,
    pub ssh_user: String,
    pub ssh_port: String,
    pub ssh_key: String,
    pub focus: ConnField,
    /// Last Ctrl-T / F6 test result — shown inside the modal (status bar is covered by scrim).
    pub test_feedback: Option<(String, bool)>,
}

impl Default for DbConnectForm {
    fn default() -> Self {
        Self {
            is_edit: false,
            name: String::new(),
            group: String::new(),
            accent: String::new(),
            driver: DbDriver::Postgres,
            host: "127.0.0.1".into(),
            port: "5432".into(),
            user: String::new(),
            password: String::new(),
            database: String::new(),
            file: String::new(),
            ssl_mode: "prefer".into(),
            ssl_trust_cert: true,
            ssl_ca: String::new(),
            read_only: false,
            role: SourceRole::Admin,
            source_id: String::new(),
            ssh_host: String::new(),
            ssh_user: String::new(),
            ssh_port: "22".into(),
            ssh_key: String::new(),
            focus: ConnField::Name,
            test_feedback: None,
        }
    }
}

impl DbConnectForm {
    pub fn fresh() -> Self {
        // Empty User — never prefill $USER / whoami (blocks honest connect tests).
        Self::default()
    }

    pub fn is_cycle_field(&self, field: ConnField) -> bool {
        matches!(
            field,
            ConnField::Driver
                | ConnField::Accent
                | ConnField::SslMode
                | ConnField::SslTrustCert
                | ConnField::ReadOnly
                | ConnField::Role
        )
    }

    /// Named accent swatches for the connect-form color picker.
    pub const ACCENT_PRESETS: [(&'static str, &'static str); 9] = [
        ("", "none"),
        ("ok", "ok"),
        ("green", "green"),
        ("cyan", "cyan"),
        ("blue", "blue"),
        ("magenta", "magenta"),
        ("yellow", "yellow"),
        ("red", "red"),
        ("slate", "slate"),
    ];

    pub fn cycle_accent(&mut self, forward: bool) {
        let cur = self.accent.trim().to_ascii_lowercase();
        let idx = Self::ACCENT_PRESETS
            .iter()
            .position(|(v, _)| *v == cur.as_str())
            .unwrap_or(0);
        let next = if forward {
            (idx + 1) % Self::ACCENT_PRESETS.len()
        } else {
            (idx + Self::ACCENT_PRESETS.len() - 1) % Self::ACCENT_PRESETS.len()
        };
        self.accent = Self::ACCENT_PRESETS[next].0.to_string();
    }

    pub fn accent_label(&self) -> &'static str {
        let cur = self.accent.trim().to_ascii_lowercase();
        Self::ACCENT_PRESETS
            .iter()
            .find(|(v, _)| *v == cur.as_str())
            .map(|(_, l)| *l)
            .unwrap_or("custom")
    }

    pub fn accent_color(&self) -> ratatui::style::Color {
        let s = self.accent.trim();
        if s.is_empty() {
            return theme::faint();
        }
        if let Some(hex) = s.strip_prefix('#') {
            if hex.len() == 6 {
                if let (Ok(r), Ok(g), Ok(b)) = (
                    u8::from_str_radix(&hex[0..2], 16),
                    u8::from_str_radix(&hex[2..4], 16),
                    u8::from_str_radix(&hex[4..6], 16),
                ) {
                    return ratatui::style::Color::Rgb(r, g, b);
                }
            }
        }
        match s.to_ascii_lowercase().as_str() {
            "ok" | "green" => theme::success(),
            "cyan" => ratatui::style::Color::Cyan,
            "blue" => ratatui::style::Color::Blue,
            "magenta" | "pink" => ratatui::style::Color::Magenta,
            "yellow" => ratatui::style::Color::Yellow,
            "red" => ratatui::style::Color::Red,
            "slate" => ratatui::style::Color::Rgb(148, 163, 184),
            _ => theme::text(),
        }
    }

    pub fn from_source(src: &DataSource) -> Self {
        Self {
            is_edit: true,
            name: src.name.clone(),
            group: src.group.clone(),
            accent: src.accent.clone(),
            driver: src.driver,
            host: src.host.clone(),
            port: if src.port == 0 {
                String::new()
            } else {
                src.port.to_string()
            },
            user: src.user.clone(),
            // Never replay plaintext — env: kept; vault shown via hint.
            password: if src.password.trim().starts_with("env:") {
                src.password.clone()
            } else {
                String::new()
            },
            database: src.database.clone(),
            file: src.file.clone(),
            ssl_mode: if src.ssl_mode.is_empty() {
                "prefer".into()
            } else {
                src.ssl_mode.clone()
            },
            ssl_trust_cert: src.ssl_trust_cert,
            ssl_ca: src.ssl_ca.clone(),
            read_only: src.read_only,
            role: src.role,
            source_id: src.id.clone(),
            ssh_host: src.ssh_host.clone(),
            ssh_user: src.ssh_user.clone(),
            ssh_port: src.ssh_port.to_string(),
            ssh_key: src.ssh_key.clone(),
            focus: ConnField::Name,
            test_feedback: None,
        }
    }

    pub fn next_field(&mut self) {
        self.focus = self.focus.next();
        self.skip_irrelevant_forward();
    }

    pub fn prev_field(&mut self) {
        self.focus = self.focus.prev();
        self.skip_irrelevant_backward();
    }

    fn skip_irrelevant_forward(&mut self) {
        for _ in 0..ConnField::ALL.len() {
            if self.field_enabled(self.focus) {
                return;
            }
            self.focus = self.focus.next();
        }
    }

    fn skip_irrelevant_backward(&mut self) {
        for _ in 0..ConnField::ALL.len() {
            if self.field_enabled(self.focus) {
                return;
            }
            self.focus = self.focus.prev();
        }
    }

    pub fn field_enabled(&self, field: ConnField) -> bool {
        match self.driver {
            DbDriver::Sqlite | DbDriver::Duckdb => matches!(
                field,
                ConnField::Name
                    | ConnField::Group
                    | ConnField::Accent
                    | ConnField::Driver
                    | ConnField::File
                    | ConnField::ReadOnly
                    | ConnField::Role
            ),
            DbDriver::Redis => matches!(
                field,
                ConnField::Name
                    | ConnField::Group
                    | ConnField::Accent
                    | ConnField::Driver
                    | ConnField::Host
                    | ConnField::Port
                    | ConnField::Password
                    | ConnField::Database
                    | ConnField::ReadOnly
                    | ConnField::Role
            ),
            DbDriver::Mongo
            | DbDriver::Elasticsearch
            | DbDriver::Trino => matches!(
                field,
                ConnField::Name
                    | ConnField::Group
                    | ConnField::Accent
                    | ConnField::Driver
                    | ConnField::Host
                    | ConnField::Port
                    | ConnField::User
                    | ConnField::Password
                    | ConnField::Database
                    | ConnField::SslMode
                    | ConnField::ReadOnly
                    | ConnField::Role
            ),
            DbDriver::Mssql => !matches!(
                field,
                ConnField::File | ConnField::SslMode | ConnField::SslCa
            ),
            DbDriver::Postgres
            | DbDriver::Mysql
            | DbDriver::Clickhouse
            | DbDriver::Snowflake
            | DbDriver::Bigquery => !matches!(field, ConnField::File | ConnField::SslTrustCert),
        }
    }

    pub fn on_driver_field(&self) -> bool {
        self.focus == ConnField::Driver
    }

    pub fn on_accent_field(&self) -> bool {
        self.focus == ConnField::Accent
    }

    pub fn on_ssl_mode_field(&self) -> bool {
        self.focus == ConnField::SslMode
    }

    pub fn on_cycle_bool_field(&self) -> bool {
        matches!(
            self.focus,
            ConnField::SslTrustCert | ConnField::ReadOnly
        )
    }

    pub fn on_role_field(&self) -> bool {
        self.focus == ConnField::Role
    }

    pub fn cycle_driver(&mut self, forward: bool) {
        const ORDER: [DbDriver; 12] = [
            DbDriver::Postgres,
            DbDriver::Mysql,
            DbDriver::Sqlite,
            DbDriver::Duckdb,
            DbDriver::Mssql,
            DbDriver::Clickhouse,
            DbDriver::Redis,
            DbDriver::Mongo,
            DbDriver::Snowflake,
            DbDriver::Bigquery,
            DbDriver::Elasticsearch,
            DbDriver::Trino,
        ];
        let idx = ORDER
            .iter()
            .position(|d| *d == self.driver)
            .unwrap_or(0);
        let next = if forward {
            (idx + 1) % ORDER.len()
        } else {
            (idx + ORDER.len() - 1) % ORDER.len()
        };
        self.driver = ORDER[next];
        match self.driver {
            DbDriver::Postgres => self.port = "5432".into(),
            DbDriver::Mysql => self.port = "3306".into(),
            DbDriver::Mssql => self.port = "1433".into(),
            DbDriver::Clickhouse => self.port = "8123".into(),
            DbDriver::Redis => self.port = "6379".into(),
            DbDriver::Mongo => self.port = "27017".into(),
            DbDriver::Elasticsearch => self.port = "9200".into(),
            DbDriver::Trino => self.port = "8080".into(),
            DbDriver::Sqlite | DbDriver::Duckdb => {}
            DbDriver::Snowflake | DbDriver::Bigquery => {}
        }
        if !self.field_enabled(self.focus) {
            self.focus = ConnField::Name;
        }
    }

    pub fn cycle_role(&mut self, forward: bool) {
        self.role = self.role.cycle(forward);
    }

    pub fn cycle_ssl_mode(&mut self, forward: bool) {
        const MODES: [&str; 4] = ["disable", "prefer", "require", "verify-full"];
        let idx = MODES
            .iter()
            .position(|m| *m == self.ssl_mode.as_str())
            .unwrap_or(1);
        let next = if forward {
            (idx + 1) % MODES.len()
        } else {
            (idx + MODES.len() - 1) % MODES.len()
        };
        self.ssl_mode = MODES[next].into();
    }

    pub fn cycle_bool_field(&mut self, forward: bool) {
        match self.focus {
            ConnField::SslTrustCert => self.ssl_trust_cert = forward,
            ConnField::ReadOnly => self.read_only = forward,
            _ => {}
        }
    }

    pub fn active_value_mut(&mut self) -> Option<&mut String> {
        match self.focus {
            ConnField::Name => Some(&mut self.name),
            ConnField::Group => Some(&mut self.group),
            ConnField::Accent | ConnField::Driver => None,
            ConnField::Host => Some(&mut self.host),
            ConnField::Port => Some(&mut self.port),
            ConnField::User => Some(&mut self.user),
            ConnField::Password => Some(&mut self.password),
            ConnField::Database => Some(&mut self.database),
            ConnField::File => Some(&mut self.file),
            ConnField::SslMode | ConnField::SslTrustCert | ConnField::ReadOnly | ConnField::Role => {
                None
            }
            ConnField::SslCa => Some(&mut self.ssl_ca),
            ConnField::SshHost => Some(&mut self.ssh_host),
            ConnField::SshUser => Some(&mut self.ssh_user),
            ConnField::SshPort => Some(&mut self.ssh_port),
            ConnField::SshKey => Some(&mut self.ssh_key),
        }
    }

    pub fn push_char(&mut self, c: char) {
        if matches!(
            self.focus,
            ConnField::Driver
                | ConnField::Accent
                | ConnField::SslMode
                | ConnField::SslTrustCert
                | ConnField::ReadOnly
                | ConnField::Role
        ) {
            return;
        }
        if let Some(v) = self.active_value_mut() {
            v.push(c);
        }
    }

    pub fn backspace(&mut self) {
        if let Some(v) = self.active_value_mut() {
            v.pop();
        }
    }

    fn display_value(&self, field: ConnField) -> String {
        match field {
            ConnField::Name => self.name.clone(),
            ConnField::Group => self.group.clone(),
            ConnField::Accent => {
                if self.accent.trim().is_empty() {
                    String::new()
                } else {
                    self.accent_label().to_string()
                }
            }
            ConnField::Driver => self.driver.label().to_string(),
            ConnField::Host => self.host.clone(),
            ConnField::Port => self.port.clone(),
            ConnField::User => self.user.clone(),
            ConnField::Password => {
                let typed = self.password.trim();
                if typed.starts_with("env:") {
                    typed.to_string()
                } else if !typed.is_empty() {
                    "•".repeat(typed.chars().count())
                } else {
                    password_display_hint(&self.source_id, "")
                }
            }
            ConnField::Database => self.database.clone(),
            ConnField::File => self.file.clone(),
            ConnField::SslMode => self.ssl_mode.clone(),
            ConnField::SslTrustCert => {
                if self.ssl_trust_cert {
                    "yes".into()
                } else {
                    "no".into()
                }
            }
            ConnField::SslCa => self.ssl_ca.clone(),
            ConnField::ReadOnly => {
                if self.read_only {
                    "yes".into()
                } else {
                    "no".into()
                }
            }
            ConnField::Role => self.role.label().to_string(),
            ConnField::SshHost => self.ssh_host.clone(),
            ConnField::SshUser => self.ssh_user.clone(),
            ConnField::SshPort => self.ssh_port.clone(),
            ConnField::SshKey => self.ssh_key.clone(),
        }
    }
}

pub fn render_db_connect_form(frame: &mut Frame, screen: Rect, form: &DbConnectForm) {
    // Same AMOLED black as Home — plate is border-only elevation, not a gray card.
    frame.render_widget(Clear, screen);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::bg())),
        screen,
    );

    let width = screen.width.min(64).max(52);
    let height = screen.height.min(26).max(18);
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 5;
    let area = Rect {
        x,
        y,
        width,
        height,
    };

    let title = if form.is_edit {
        " Edit connection "
    } else {
        " New connection "
    };
    let title_style = Style::default()
        .fg(theme::bg())
        .bg(theme::text())
        .add_modifier(Modifier::BOLD);

    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::text()))
            .style(Style::default().bg(theme::bg()))
            .title(Span::styled(title, title_style)),
        area,
    );

    let has_feedback = form.test_feedback.is_some();
    let constraints: Vec<Constraint> = if has_feedback {
        vec![
            Constraint::Min(10),
            Constraint::Length(1),
            Constraint::Length(1),
        ]
    } else {
        vec![Constraint::Min(12), Constraint::Length(1)]
    };
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .margin(2)
        .split(area);

    let mut lines: Vec<Line> = Vec::new();
    let mut saw_ssl = false;
    let mut saw_ssh = false;
    for field in ConnField::ALL {
        if !form.field_enabled(field) {
            continue;
        }
        // Quiet section breaks — no heavy chrome.
        if matches!(field, ConnField::SslMode | ConnField::SslTrustCert) && !saw_ssl {
            saw_ssl = true;
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("tls", theme::style_faint())));
        }
        if matches!(field, ConnField::SshHost) && !saw_ssh {
            saw_ssh = true;
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("ssh", theme::style_faint())));
        }

        let focused = form.focus == field;
        let cycle = form.is_cycle_field(field);
        let value = form.display_value(field);
        let empty = value.is_empty();

        let cursor = if focused { "›" } else { " " };
        let cursor_style = if focused {
            theme::style_text().add_modifier(Modifier::BOLD)
        } else {
            theme::style_faint()
        };
        let label_style = if focused {
            theme::style_text().add_modifier(Modifier::BOLD)
        } else {
            theme::style_faint()
        };

        // Accent: live color picker swatch.
        if field == ConnField::Accent {
            let label = if empty {
                "none"
            } else {
                form.accent_label()
            };
            let swatch = if empty { "·" } else { "██" };
            let shown = if focused {
                format!("‹ {swatch} {label} ›")
            } else {
                format!("{swatch} {label}")
            };
            let swatch_style = if empty {
                theme::style_faint()
            } else {
                Style::default()
                    .fg(form.accent_color())
                    .add_modifier(if focused {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    })
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{cursor} "), cursor_style),
                Span::styled(format!("{:<11}", field.label()), label_style),
                Span::styled(shown, swatch_style),
            ]));
            continue;
        }

        let value_shown = if cycle {
            if focused {
                format!("‹ {value} ›")
            } else {
                value
            }
        } else if empty {
            field.hint(form.driver).to_string()
        } else if focused {
            format!("{value}▌")
        } else {
            value
        };

        let value_style = if focused {
            theme::style_text().add_modifier(Modifier::BOLD)
        } else if empty && !cycle {
            theme::style_faint()
        } else {
            theme::style_muted()
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{cursor} "), cursor_style),
            Span::styled(format!("{:<11}", field.label()), label_style),
            Span::styled(value_shown, value_style),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner[0]);

    let foot_idx = if let Some((ref msg, err)) = form.test_feedback {
        let badge = if err {
            Span::styled(" ERR ", theme::style_err_badge())
        } else {
            Span::styled(" OK ", theme::style_ok_badge())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                badge,
                Span::styled(format!(" {msg}"), theme::style_text()),
            ])),
            inner[1],
        );
        2
    } else {
        1
    };

    // One key per action (F6 still works; not advertised).
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Tab", theme::style_faint()),
            Span::styled(" · ", theme::style_faint()),
            Span::styled("←→", theme::style_faint()),
            Span::styled(" · ", theme::style_faint()),
            Span::styled("^T", theme::style_faint()),
            Span::styled(" test · ", theme::style_faint()),
            Span::styled("Enter", theme::style_faint()),
            Span::styled(" · ", theme::style_faint()),
            Span::styled("Esc", theme::style_faint()),
        ])),
        inner[foot_idx],
    );
}
