//! Central keymap table.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ide_core::{Focus, Mode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyContext {
    Normal,
    Insert,
    Command,
    Palette,
    Tree,
    Diagnostics,
    Git,
    Search,
    Terminal,
    Outline,
    Debug,
    Sessions,
    SessionForm,
    Agents,
    AgentForm,
    Rename,
    Database,
    Results,
    Find,
    Replace,
    Commit,
    DbConnect,
    EditCell,
    Settings,
    OpenFolder,
    ImportCsv,
    RestoreDump,
    SchemaDiff,
    DataCompare,
    QueryParams,
    ConfirmDelete,
    SaveMaterialize,
    BrainAsk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    ForceQuit,
    Save,
    CloseTab,
    ForceCloseTab,
    EnterInsert,
    EnterNormal,
    EnterCommand,
    OpenPalette,
    OpenFilePalette,
    OpenDbObjectPalette,
    Confirm,
    Cancel,
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    MoveLineStart,
    MoveLineEnd,
    PageUp,
    PageDown,
    InsertNewline,
    Backspace,
    Delete,
    Undo,
    Redo,
    NextTab,
    PrevTab,
    ToggleTree,
    FocusEditor,
    FocusTree,
    FocusDiagnostics,
    ToggleOrOpen,
    RequestCompletion,
    GotoDefinition,
    Hover,
    AcceptCompletion,
    NextCompletion,
    PrevCompletion,
    RefreshTree,
    Format,
    Rename,
    CodeAction,
    ToggleInlayHints,
    OpenSearch,
    OpenGit,
    OpenOutline,
    OpenTerminal,
    OpenDebug,
    OpenSessions,
    /// Product ribbon: 0 Code · 1 Database · 2 Sessions · 3 Agents · 4 Git
    ProductTab(u8),
    SessionConnect,
    SessionAddSsh,
    SessionAddRdp,
    SessionEdit,
    SessionDelete,
    SessionNextTab,
    SessionPrevTab,
    SessionCloseTab,
    SessionFormNextField,
    SessionFormPrevField,
    OpenAgents,
    AgentLaunch,
    AgentAdd,
    AgentEdit,
    AgentDelete,
    AgentFormNextField,
    AgentFormPrevField,
    SidebarFiles,
    StageFile,
    UnstageFile,
    Blame,
    SplitVertical,
    FocusOtherSplit,
    AddCursorBelow,
    ClearExtraCursors,
    ToggleBreakpoint,
    DebugContinue,
    DebugStepOver,
    DebugStepIn,
    ApplyQuickFix,
    NextProblem,
    PrevProblem,
    ToggleProductMode,
    EnterCodeEditor,
    EnterDbStudio,
    GoHome,
    OpenFolder,
    OpenProjects,
    /// Home quick jump: 1..=9 recent slot, or OpenSettings via `c`.
    OpenHomeSlot(u8),
    /// Database Home: 1..=9 select source and enter studio.
    OpenDbSourceSlot(u8),
    DbConnectNextField,
    DbConnectPrevField,
    DbConnectDriverNext,
    DbConnectDriverPrev,
    DbConnectTest,
    /// Cycle Explorer → Console → Results in Database studio.
    CycleDbPane,
    FocusDbExplorer,
    OpenDatabase,
    RunQuery,
    AddDataSource,
    EditDataSource,
    Find,
    FindNext,
    FindPrev,
    Replace,
    ReplaceAll,
    GitCommit,
    GitPush,
    GitPull,
    CheckoutBranch,
    ExplainQuery,
    ApplyRowUpdates,
    ExportCsv,
    ExportSql,
    ImportCsv,
    KillSession,
    BookmarkSql,
    DumpDatabase,
    RestoreDump,
    SchemaDiff,
    SchemaDiffDdl,
    DataCompare,
    BeginTransaction,
    CommitTransaction,
    RollbackTransaction,
    EditCell,
    /// Copy focused results cell to host clipboard.
    YankResultCell,
    FocusResults,
    NextBranch,
    PrevBranch,
    RunPlugin,
    OpenSettings,
    RefreshDebugLocals,
    ResultPageNext,
    ResultPagePrev,
    ChartResults,
    SaveResultTemp,
    SaveResultView,
    CancelQuery,
    /// Studio: cycle to next data source and reconnect.
    CycleDbSourceNext,
    /// Studio: cycle to previous data source and reconnect.
    CycleDbSourcePrev,
    /// Toggle `?` keymap help overlay.
    ToggleHelp,
    /// Toggle results compact/comfort density.
    ToggleResultsDensity,
}

pub struct Keymap;

impl Keymap {
    pub fn context(mode: Mode, focus: Focus) -> KeyContext {
        match mode {
            Mode::Insert => KeyContext::Insert,
            Mode::Command => KeyContext::Command,
            Mode::Palette => KeyContext::Palette,
            Mode::Search => KeyContext::Search,
            Mode::Rename => KeyContext::Rename,
            Mode::Find => KeyContext::Find,
            Mode::Replace => KeyContext::Replace,
            Mode::Commit => KeyContext::Commit,
            Mode::DbConnect => KeyContext::DbConnect,
            Mode::EditCell => KeyContext::EditCell,
            Mode::Settings => KeyContext::Settings,
            Mode::OpenFolder => KeyContext::OpenFolder,
            Mode::ImportCsv => KeyContext::ImportCsv,
            Mode::RestoreDump => KeyContext::RestoreDump,
            Mode::SchemaDiff => KeyContext::SchemaDiff,
            Mode::DataCompare => KeyContext::DataCompare,
            Mode::QueryParams => KeyContext::QueryParams,
            Mode::SaveMaterialize => KeyContext::SaveMaterialize,
            Mode::BrainAsk => KeyContext::BrainAsk,
            Mode::SessionForm => KeyContext::SessionForm,
            Mode::AgentForm => KeyContext::AgentForm,
            Mode::ConfirmDeleteSource | Mode::ConfirmQuit => KeyContext::ConfirmDelete,
            Mode::Normal => match focus {
                Focus::Tree => KeyContext::Tree,
                Focus::Diagnostics => KeyContext::Diagnostics,
                Focus::Git => KeyContext::Git,
                Focus::Search => KeyContext::Search,
                Focus::Terminal => KeyContext::Terminal,
                Focus::Outline => KeyContext::Outline,
                Focus::Debug => KeyContext::Debug,
                Focus::Sessions => KeyContext::Sessions,
                Focus::Agents => KeyContext::Agents,
                Focus::Database | Focus::Structure => KeyContext::Database,
                Focus::Results => KeyContext::Results,
                Focus::Editor => KeyContext::Normal,
            },
        }
    }

    pub fn map(ctx: KeyContext, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match ctx {
            KeyContext::Normal => match (ctrl, shift, key.code) {
                (true, false, KeyCode::Char('p')) => Some(Action::OpenFilePalette),
                (true, false, KeyCode::Char('s')) => Some(Action::Save),
                (true, false, KeyCode::Char('w')) => Some(Action::CloseTab),
                (true, false, KeyCode::Char('z')) => Some(Action::Undo),
                (true, false, KeyCode::Char('y')) => Some(Action::Redo),
                (true, false, KeyCode::Char(' ')) => Some(Action::RequestCompletion),
                (true, false, KeyCode::Char('b')) => Some(Action::ToggleTree),
                (true, false, KeyCode::Char('f')) => Some(Action::Find),
                (true, true, KeyCode::Char('f') | KeyCode::Char('F')) => Some(Action::OpenSearch),
                (true, false, KeyCode::Char('h')) => Some(Action::Replace),
                (true, true, KeyCode::Char('g') | KeyCode::Char('G')) => Some(Action::OpenGit),
                (true, true, KeyCode::Char('o') | KeyCode::Char('O')) => Some(Action::OpenOutline),
                // Shift makes 'S'; both open Sessions. Export SQL moved to Ctrl-⇧-L.
                (true, true, KeyCode::Char('s') | KeyCode::Char('S')) => Some(Action::OpenSessions),
                // Shift makes 'A'; both open Agents.
                (true, true, KeyCode::Char('a') | KeyCode::Char('A')) => Some(Action::OpenAgents),
                (true, true, KeyCode::Char('i') | KeyCode::Char('I')) => Some(Action::Format),
                (true, true, KeyCode::Char('d') | KeyCode::Char('D')) => Some(Action::ToggleProductMode),
                (true, true, KeyCode::Char('h') | KeyCode::Char('H')) => Some(Action::GoHome),
                // Product ribbon: Ctrl-1…3 Code/Database/Sessions (Agents under Code).
                (true, false, KeyCode::Char('1')) => Some(Action::ProductTab(0)),
                (true, false, KeyCode::Char('2')) => Some(Action::ProductTab(1)),
                (true, false, KeyCode::Char('3')) => Some(Action::ProductTab(2)),
                (false, false, KeyCode::Char('?')) => Some(Action::ToggleHelp),
                (true, false, KeyCode::Char(']')) => Some(Action::CycleDbSourceNext),
                (true, false, KeyCode::Char('[')) => Some(Action::CycleDbSourcePrev),
                (false, false, KeyCode::Char('e')) => Some(Action::EnterCodeEditor),
                (false, false, KeyCode::Char('o')) => Some(Action::OpenFolder),
                (false, false, KeyCode::Char('p')) => Some(Action::OpenProjects),
                (true, false, KeyCode::Char('e')) => Some(Action::ExplainQuery),
                (true, true, KeyCode::Char('E')) => Some(Action::ExportCsv),
                (true, true, KeyCode::Char('l') | KeyCode::Char('L')) => Some(Action::ExportSql),
                (true, true, KeyCode::Char('u') | KeyCode::Char('U')) => Some(Action::ImportCsv),
                (true, false, KeyCode::Char('.')) => Some(Action::CodeAction),
                (true, false, KeyCode::Char('\\')) => Some(Action::SplitVertical),
                (true, false, KeyCode::Char('`')) => Some(Action::OpenTerminal),
                (true, false, KeyCode::Char('d')) => Some(Action::AddCursorBelow),
                (true, false, KeyCode::Enter) => Some(Action::RunQuery),
                (true, false, KeyCode::Char('\n')) => Some(Action::RunQuery),
                // Many TTYs send Ctrl-Enter as Ctrl-J when kitty protocol is off.
                (true, false, KeyCode::Char('j')) => Some(Action::RunQuery),
                (true, false, KeyCode::Char('r')) => Some(Action::RunQuery),
                (false, false, KeyCode::F(2)) => Some(Action::Rename),
                (false, false, KeyCode::F(9)) => Some(Action::ToggleBreakpoint),
                (false, false, KeyCode::F(5)) => Some(Action::DebugContinue),
                (false, false, KeyCode::F(10)) => Some(Action::DebugStepOver),
                (false, false, KeyCode::F(11)) => Some(Action::DebugStepIn),
                (false, false, KeyCode::Char('i')) => Some(Action::EnterInsert),
                (false, false, KeyCode::Char('a')) => Some(Action::EnterInsert),
                (false, false, KeyCode::Char(':')) => Some(Action::EnterCommand),
                (false, false, KeyCode::Char('q')) => Some(Action::Quit),
                (false, false, KeyCode::Char('/')) => Some(Action::Find),
                (false, false, KeyCode::Char('h')) | (false, false, KeyCode::Left) => {
                    Some(Action::MoveLeft)
                }
                (false, false, KeyCode::Char('l')) | (false, false, KeyCode::Right) => {
                    Some(Action::MoveRight)
                }
                (false, false, KeyCode::Char('k')) | (false, false, KeyCode::Up) => {
                    Some(Action::MoveUp)
                }
                (false, false, KeyCode::Char('j')) | (false, false, KeyCode::Down) => {
                    Some(Action::MoveDown)
                }
                (false, false, KeyCode::Home) | (false, false, KeyCode::Char('0')) => {
                    Some(Action::MoveLineStart)
                }
                (false, false, KeyCode::End) | (false, false, KeyCode::Char('$')) => {
                    Some(Action::MoveLineEnd)
                }
                (false, false, KeyCode::PageUp) => Some(Action::PageUp),
                (false, false, KeyCode::PageDown) => Some(Action::PageDown),
                (false, false, KeyCode::Tab) => Some(Action::NextTab),
                (false, true, KeyCode::BackTab) => Some(Action::PrevTab),
                (false, false, KeyCode::Char(' ')) => Some(Action::OpenPalette),
                (false, false, KeyCode::Char('g')) => Some(Action::GotoDefinition),
                (false, true, KeyCode::Char('K')) | (false, false, KeyCode::Char('K')) => {
                    Some(Action::Hover)
                }
                (false, false, KeyCode::Char('1')) => Some(Action::SidebarFiles),
                (false, false, KeyCode::Char('2')) => Some(Action::FocusEditor),
                (false, false, KeyCode::Char('3')) => Some(Action::FocusDiagnostics),
                (false, false, KeyCode::Char('4')) => Some(Action::OpenGit),
                (false, false, KeyCode::Char('5')) => Some(Action::OpenSearch),
                (false, false, KeyCode::Char('6')) => Some(Action::OpenOutline),
                (false, false, KeyCode::Char('7')) => Some(Action::OpenDebug),
                (false, false, KeyCode::Char('8')) => Some(Action::OpenDatabase),
                (false, false, KeyCode::Char('9')) => Some(Action::FocusResults),
                (false, false, KeyCode::Char('n')) => Some(Action::FindNext),
                (false, true, KeyCode::Char('N')) => Some(Action::FindPrev),
                (false, false, KeyCode::Char('w')) if !ctrl => Some(Action::FocusOtherSplit),
                (false, false, KeyCode::Esc) => Some(Action::ClearExtraCursors),
                (false, false, KeyCode::Char(']')) => Some(Action::NextProblem),
                (false, false, KeyCode::Char('[')) => Some(Action::PrevProblem),
                (false, false, KeyCode::Enter) => Some(Action::ApplyQuickFix),
                _ => None,
            },
            KeyContext::Insert => match (ctrl, shift, key.code) {
                (_, _, KeyCode::Esc) => Some(Action::EnterNormal),
                (true, false, KeyCode::Char('s')) => Some(Action::Save),
                (true, false, KeyCode::Char(' ')) => Some(Action::RequestCompletion),
                (true, false, KeyCode::Char('n')) => Some(Action::NextCompletion),
                (true, false, KeyCode::Char('p')) => Some(Action::PrevCompletion),
                (true, false, KeyCode::Char('d')) => Some(Action::AddCursorBelow),
                (true, false, KeyCode::Char('e')) => Some(Action::ExplainQuery),
                (true, false, KeyCode::Enter) => Some(Action::RunQuery),
                (true, false, KeyCode::Char('\n')) => Some(Action::RunQuery),
                (true, false, KeyCode::Char('j')) => Some(Action::RunQuery),
                (true, false, KeyCode::Char('r')) => Some(Action::RunQuery),
                (_, _, KeyCode::F(5)) => Some(Action::RunQuery),
                (_, _, KeyCode::Enter) => Some(Action::InsertNewline),
                (_, _, KeyCode::Tab) => Some(Action::AcceptCompletion),
                (_, _, KeyCode::Backspace) => Some(Action::Backspace),
                (_, _, KeyCode::Delete) => Some(Action::Delete),
                (_, _, KeyCode::Left) => Some(Action::MoveLeft),
                (_, _, KeyCode::Right) => Some(Action::MoveRight),
                (_, _, KeyCode::Up) => Some(Action::MoveUp),
                (_, _, KeyCode::Down) => Some(Action::MoveDown),
                (_, _, KeyCode::Home) => Some(Action::MoveLineStart),
                (_, _, KeyCode::End) => Some(Action::MoveLineEnd),
                (true, false, KeyCode::Char('z')) => Some(Action::Undo),
                _ => None,
            },
            KeyContext::Command
            | KeyContext::Rename
            | KeyContext::Find
            | KeyContext::Replace
            | KeyContext::Commit
            | KeyContext::EditCell
            | KeyContext::Settings
            | KeyContext::OpenFolder
            | KeyContext::ImportCsv
            | KeyContext::RestoreDump
            | KeyContext::SchemaDiff
            | KeyContext::DataCompare
            | KeyContext::QueryParams
            | KeyContext::SaveMaterialize
            | KeyContext::BrainAsk => match key.code {
                KeyCode::Esc => Some(Action::Cancel),
                KeyCode::Enter => Some(Action::Confirm),
                KeyCode::Backspace => Some(Action::Backspace),
                _ => None,
            },
            KeyContext::DbConnect => match (ctrl, shift, key.code) {
                (_, _, KeyCode::Esc) => Some(Action::Cancel),
                (_, _, KeyCode::Enter) => Some(Action::Confirm),
                (_, _, KeyCode::Backspace) => Some(Action::Backspace),
                (true, _, KeyCode::Char('t')) | (_, _, KeyCode::F(6)) => {
                    Some(Action::DbConnectTest)
                }
                (_, _, KeyCode::Tab) | (_, _, KeyCode::Down) => Some(Action::DbConnectNextField),
                (_, true, KeyCode::BackTab) | (_, _, KeyCode::Up) => {
                    Some(Action::DbConnectPrevField)
                }
                (_, _, KeyCode::Right) => Some(Action::DbConnectDriverNext),
                (_, _, KeyCode::Left) => Some(Action::DbConnectDriverPrev),
                _ => None,
            },
            KeyContext::Palette | KeyContext::Search => match key.code {
                KeyCode::Esc => Some(Action::Cancel),
                KeyCode::Enter => Some(Action::Confirm),
                KeyCode::Up => Some(Action::MoveUp),
                KeyCode::Down => Some(Action::MoveDown),
                KeyCode::Backspace => Some(Action::Backspace),
                _ => None,
            },
            KeyContext::Tree => match (ctrl, key.code) {
                (false, KeyCode::Char('q')) => Some(Action::Quit),
                (false, KeyCode::Char('j')) | (false, KeyCode::Down) => Some(Action::MoveDown),
                (false, KeyCode::Char('k')) | (false, KeyCode::Up) => Some(Action::MoveUp),
                (false, KeyCode::Enter) | (false, KeyCode::Char('l')) => Some(Action::ToggleOrOpen),
                (false, KeyCode::Char('2')) => Some(Action::FocusEditor),
                (false, KeyCode::Char('r')) => Some(Action::RefreshTree),
                (true, KeyCode::Char('p')) => Some(Action::OpenFilePalette),
                (false, KeyCode::Esc) => Some(Action::FocusEditor),
                _ => None,
            },
            KeyContext::Database => match (ctrl, shift, key.code) {
                (false, false, KeyCode::Char('q')) => Some(Action::Quit),
                (false, false, KeyCode::Char('j')) | (false, false, KeyCode::Down) => {
                    Some(Action::MoveDown)
                }
                (false, false, KeyCode::Char('k')) | (false, false, KeyCode::Up) => {
                    Some(Action::MoveUp)
                }
                (false, false, KeyCode::Enter) | (false, false, KeyCode::Char('l')) => {
                    Some(Action::ToggleOrOpen)
                }
                (false, false, KeyCode::Char('2')) => Some(Action::FocusEditor),
                (false, false, KeyCode::Char('3')) => Some(Action::FocusResults),
                (false, false, KeyCode::Char('r')) => Some(Action::RefreshTree),
                (false, false, KeyCode::Char('e')) => Some(Action::EditDataSource),
                (false, false, KeyCode::Char('?')) => Some(Action::ToggleHelp),
                (false, false, KeyCode::Tab) => Some(Action::CycleDbPane),
                (false, false, KeyCode::Esc) => Some(Action::GoHome),
                (true, false, KeyCode::Char('p')) => Some(Action::OpenDbObjectPalette),
                (true, false, KeyCode::Char('e')) => Some(Action::ExplainQuery),
                (true, false, KeyCode::Char(']')) => Some(Action::CycleDbSourceNext),
                (true, false, KeyCode::Char('[')) => Some(Action::CycleDbSourcePrev),
                (true, false, KeyCode::Enter)
                | (true, false, KeyCode::Char('\n'))
                | (true, false, KeyCode::Char('j'))
                | (true, false, KeyCode::Char('r'))
                | (false, false, KeyCode::F(5)) => Some(Action::RunQuery),
                _ => None,
            },
            KeyContext::Results => match (ctrl, shift, key.code) {
                (false, false, KeyCode::Char(']')) => Some(Action::ResultPageNext),
                (false, false, KeyCode::Char('[')) => Some(Action::ResultPagePrev),
                (true, false, KeyCode::Char(']')) => Some(Action::CycleDbSourceNext),
                (true, false, KeyCode::Char('[')) => Some(Action::CycleDbSourcePrev),
                (true, false, KeyCode::Enter)
                | (true, false, KeyCode::Char('\n'))
                | (true, false, KeyCode::Char('j'))
                | (true, false, KeyCode::Char('r'))
                | (false, false, KeyCode::F(5)) => Some(Action::RunQuery),
                (true, false, KeyCode::Char('s')) => Some(Action::ApplyRowUpdates),
                (true, true, KeyCode::Char('E')) => Some(Action::ExportCsv),
                (true, true, KeyCode::Char('S') | KeyCode::Char('l') | KeyCode::Char('L')) => {
                    Some(Action::ExportSql)
                }
                (true, true, KeyCode::Char('I')) => Some(Action::ImportCsv),
                (false, false, KeyCode::Char('K')) => Some(Action::KillSession),
                (false, false, KeyCode::Char('a')) => Some(Action::ApplyRowUpdates),
                (false, false, KeyCode::Char('u')) => Some(Action::Undo),
                (false, false, KeyCode::Char('D')) => Some(Action::ToggleResultsDensity),
                (false, false, KeyCode::Char('?')) => Some(Action::ToggleHelp),
                (false, false, KeyCode::Char('i')) | (false, false, KeyCode::Enter) => {
                    Some(Action::EditCell)
                }
                (false, false, KeyCode::Char('y')) => Some(Action::YankResultCell),
                (false, false, KeyCode::Char('j')) | (false, false, KeyCode::Down) => {
                    Some(Action::MoveDown)
                }
                (false, false, KeyCode::Char('k')) | (false, false, KeyCode::Up) => {
                    Some(Action::MoveUp)
                }
                (false, false, KeyCode::Char('h')) | (false, false, KeyCode::Left) => {
                    Some(Action::MoveLeft)
                }
                (false, false, KeyCode::Char('l')) | (false, false, KeyCode::Right) => {
                    Some(Action::MoveRight)
                }
                (false, false, KeyCode::Char('2')) | (false, false, KeyCode::Esc) => {
                    Some(Action::FocusEditor)
                }
                (false, false, KeyCode::Char('/')) => Some(Action::Find),
                (false, false, KeyCode::Char('q')) => Some(Action::Quit),
                _ => None,
            },
            KeyContext::Diagnostics => match key.code {
                KeyCode::Char('j') | KeyCode::Down => Some(Action::MoveDown),
                KeyCode::Char('k') | KeyCode::Up => Some(Action::MoveUp),
                KeyCode::Enter => Some(Action::Confirm),
                KeyCode::Char('.') => Some(Action::ApplyQuickFix),
                KeyCode::Char('2') | KeyCode::Esc => Some(Action::FocusEditor),
                KeyCode::Char('q') => Some(Action::Quit),
                _ => None,
            },
            KeyContext::Git => match (shift, key.code) {
                (false, KeyCode::Char('j')) | (false, KeyCode::Down) => Some(Action::MoveDown),
                (false, KeyCode::Char('k')) | (false, KeyCode::Up) => Some(Action::MoveUp),
                (false, KeyCode::Char('s')) => Some(Action::StageFile),
                (false, KeyCode::Char('u')) => Some(Action::UnstageFile),
                (false, KeyCode::Char('b')) => Some(Action::Blame),
                (false, KeyCode::Char('c')) => Some(Action::GitCommit),
                (false, KeyCode::Char('p')) => Some(Action::GitPush),
                (true, KeyCode::Char('P')) => Some(Action::GitPull),
                (false, KeyCode::Char('[')) => Some(Action::PrevBranch),
                (false, KeyCode::Char(']')) => Some(Action::NextBranch),
                (false, KeyCode::Char('o')) => Some(Action::CheckoutBranch),
                (false, KeyCode::Char('r')) => Some(Action::RefreshTree),
                (false, KeyCode::Enter) => Some(Action::Confirm),
                (false, KeyCode::Esc) | (false, KeyCode::Char('2')) => Some(Action::FocusEditor),
                _ => None,
            },
            KeyContext::Outline | KeyContext::Debug => match key.code {
                KeyCode::Char('j') | KeyCode::Down => Some(Action::MoveDown),
                KeyCode::Char('k') | KeyCode::Up => Some(Action::MoveUp),
                KeyCode::Enter => Some(Action::Confirm),
                KeyCode::F(5) => Some(Action::DebugContinue),
                KeyCode::F(9) => Some(Action::ToggleBreakpoint),
                KeyCode::F(10) => Some(Action::DebugStepOver),
                KeyCode::Char('r') if ctx == KeyContext::Debug => Some(Action::RefreshDebugLocals),
                KeyCode::Esc | KeyCode::Char('2') => Some(Action::FocusEditor),
                _ => None,
            },
            KeyContext::Sessions => match (ctrl, shift, key.code) {
                (_, _, KeyCode::Char('j') | KeyCode::Down) => Some(Action::MoveDown),
                (_, _, KeyCode::Char('k') | KeyCode::Up) => Some(Action::MoveUp),
                (_, _, KeyCode::Enter) => Some(Action::SessionConnect),
                (_, _, KeyCode::Char('a')) => Some(Action::SessionAddSsh),
                (_, true, KeyCode::Char('A')) => Some(Action::SessionAddRdp),
                (_, _, KeyCode::Char('e')) => Some(Action::SessionEdit),
                (_, _, KeyCode::Char('x') | KeyCode::Delete) => Some(Action::SessionDelete),
                (_, _, KeyCode::Char(']')) => Some(Action::SessionNextTab),
                (_, _, KeyCode::Char('[')) => Some(Action::SessionPrevTab),
                (_, _, KeyCode::Char('w')) if !ctrl => Some(Action::SessionCloseTab),
                (_, _, KeyCode::Esc) | (_, _, KeyCode::Char('2')) => Some(Action::FocusEditor),
                (true, true, KeyCode::Char('s')) => Some(Action::OpenSessions),
                _ => None,
            },
            KeyContext::SessionForm => match (ctrl, key.code) {
                (_, KeyCode::Esc) => Some(Action::Cancel),
                (_, KeyCode::Enter) => Some(Action::Confirm),
                (_, KeyCode::Tab) => Some(Action::SessionFormNextField),
                (_, KeyCode::BackTab) => Some(Action::SessionFormPrevField),
                (_, KeyCode::Backspace) => Some(Action::Backspace),
                _ => None,
            },
            KeyContext::Agents => match (ctrl, shift, key.code) {
                (_, _, KeyCode::Char('j') | KeyCode::Down) => Some(Action::MoveDown),
                (_, _, KeyCode::Char('k') | KeyCode::Up) => Some(Action::MoveUp),
                (_, _, KeyCode::Enter) => Some(Action::AgentLaunch),
                (true, true, KeyCode::Char('a') | KeyCode::Char('A')) => Some(Action::OpenAgents),
                (false, false, KeyCode::Char('a')) => Some(Action::AgentAdd),
                (_, _, KeyCode::Char('e')) => Some(Action::AgentEdit),
                (_, _, KeyCode::Char('x') | KeyCode::Delete) => Some(Action::AgentDelete),
                (_, _, KeyCode::Char(']')) => Some(Action::SessionNextTab),
                (_, _, KeyCode::Char('[')) => Some(Action::SessionPrevTab),
                (_, _, KeyCode::Char('w')) if !ctrl => Some(Action::SessionCloseTab),
                (_, _, KeyCode::Esc) | (_, _, KeyCode::Char('2')) => Some(Action::FocusEditor),
                _ => None,
            },
            KeyContext::AgentForm => match (ctrl, key.code) {
                (_, KeyCode::Esc) => Some(Action::Cancel),
                (_, KeyCode::Enter) => Some(Action::Confirm),
                (_, KeyCode::Tab) => Some(Action::AgentFormNextField),
                (_, KeyCode::BackTab) => Some(Action::AgentFormPrevField),
                (_, KeyCode::Backspace) => Some(Action::Backspace),
                _ => None,
            },
            KeyContext::ConfirmDelete => match key.code {
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => Some(Action::Cancel),
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => Some(Action::Confirm),
                KeyCode::Char('!') => Some(Action::ForceQuit),
                _ => None,
            },
            KeyContext::Terminal => match (ctrl, key.code) {
                (true, KeyCode::Char('`')) => Some(Action::FocusEditor),
                (_, KeyCode::Esc) if ctrl => Some(Action::FocusEditor),
                (true, KeyCode::Char('2')) => Some(Action::FocusEditor),
                (true, KeyCode::Char(']')) => Some(Action::SessionNextTab),
                (true, KeyCode::Char('[')) => Some(Action::SessionPrevTab),
                (true, KeyCode::Char('w')) => Some(Action::SessionCloseTab),
                _ => None, // raw bytes forwarded by app
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn results_focus_apply_and_cell_edit() {
        let ctx = KeyContext::Results;
        assert_eq!(
            Keymap::map(ctx, ctrl(KeyCode::Char('s'))),
            Some(Action::ApplyRowUpdates)
        );
        assert_eq!(
            Keymap::map(ctx, key(KeyCode::Char('i'))),
            Some(Action::EditCell)
        );
    }

    #[test]
    fn git_branch_keys() {
        let ctx = KeyContext::Git;
        assert_eq!(
            Keymap::map(ctx, key(KeyCode::Char('['))),
            Some(Action::PrevBranch)
        );
        assert_eq!(
            Keymap::map(ctx, key(KeyCode::Char(']'))),
            Some(Action::NextBranch)
        );
    }

    #[test]
    fn edit_cell_and_settings_contexts() {
        assert_eq!(
            Keymap::context(Mode::EditCell, Focus::Results),
            KeyContext::EditCell
        );
        assert_eq!(
            Keymap::context(Mode::Settings, Focus::Editor),
            KeyContext::Settings
        );
    }

    fn ctrl_shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL | KeyModifiers::SHIFT)
    }

    #[test]
    fn sessions_bindings() {
        let normal = KeyContext::Normal;
        assert_eq!(
            Keymap::map(normal, ctrl_shift(KeyCode::Char('s'))),
            Some(Action::OpenSessions)
        );
        // Real TTY: Ctrl-Shift-S arrives as Char('S') — must NOT be ExportSql.
        assert_eq!(
            Keymap::map(normal, ctrl_shift(KeyCode::Char('S'))),
            Some(Action::OpenSessions)
        );
        assert_eq!(
            Keymap::map(normal, ctrl_shift(KeyCode::Char('L'))),
            Some(Action::ExportSql)
        );
        // Ctrl-S remains Save (must not steal Sessions)
        assert_eq!(
            Keymap::map(normal, ctrl(KeyCode::Char('s'))),
            Some(Action::Save)
        );

        let sess = KeyContext::Sessions;
        assert_eq!(
            Keymap::context(Mode::Normal, Focus::Sessions),
            KeyContext::Sessions
        );
        assert_eq!(
            Keymap::map(sess, key(KeyCode::Enter)),
            Some(Action::SessionConnect)
        );
        assert_eq!(
            Keymap::map(sess, key(KeyCode::Char('a'))),
            Some(Action::SessionAddSsh)
        );
        assert_eq!(
            Keymap::map(sess, KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            Some(Action::SessionAddRdp)
        );
        assert_eq!(
            Keymap::map(sess, key(KeyCode::Char('e'))),
            Some(Action::SessionEdit)
        );
        assert_eq!(
            Keymap::map(sess, key(KeyCode::Char('x'))),
            Some(Action::SessionDelete)
        );
    }

    #[test]
    fn agents_bindings() {
        let normal = KeyContext::Normal;
        assert_eq!(
            Keymap::map(normal, ctrl_shift(KeyCode::Char('a'))),
            Some(Action::OpenAgents)
        );
        assert_eq!(
            Keymap::map(normal, ctrl_shift(KeyCode::Char('A'))),
            Some(Action::OpenAgents)
        );
        let agents = KeyContext::Agents;
        assert_eq!(
            Keymap::context(Mode::Normal, Focus::Agents),
            KeyContext::Agents
        );
        assert_eq!(
            Keymap::map(agents, key(KeyCode::Enter)),
            Some(Action::AgentLaunch)
        );
        assert_eq!(
            Keymap::map(agents, key(KeyCode::Char('a'))),
            Some(Action::AgentAdd)
        );
    }
}
