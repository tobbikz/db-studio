//! Single LSP server process.

use anyhow::{Context, Result, anyhow};
use dashmap::DashMap;
use ide_config::LanguageServerConfig;
use lsp_types::*;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct DiagnosticItem {
    pub path: PathBuf,
    pub message: String,
    pub severity: DiagnosticSeverity,
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
}

#[derive(Debug, Clone)]
pub struct CompletionItemView {
    pub label: String,
    pub detail: Option<String>,
    pub insert_text: String,
    pub kind: Option<CompletionItemKind>,
    /// Characters before cursor that this completion should replace.
    pub replace_start_col: Option<u32>,
    pub replace_end_col: Option<u32>,
    pub filter_text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InlayHintView {
    pub line: u32,
    pub character: u32,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct SymbolItem {
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub character: u32,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub enum LspEvent {
    Diagnostics {
        path: PathBuf,
        items: Vec<DiagnosticItem>,
    },
    ServerMessage(String),
}

struct Pending {
    tx: oneshot::Sender<Result<Value>>,
}

struct Shared {
    diagnostics: DashMap<PathBuf, Vec<DiagnosticItem>>,
    event_tx: mpsc::UnboundedSender<LspEvent>,
}

pub struct LspClient {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: AtomicI64,
    pending: Arc<DashMap<i64, Pending>>,
    shared: Arc<Shared>,
    root: PathBuf,
    _child: Child,
    initialized: bool,
    pub name: String,
    pub event_rx: mpsc::UnboundedReceiver<LspEvent>,
}

impl LspClient {
    pub async fn start(cfg: &LanguageServerConfig, root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        which_bin(&cfg.command)?;

        let mut child = Command::new(&cfg.command)
            .args(&cfg.args)
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn {}", cfg.command))?;

        let stdin = child.stdin.take().context("lsp stdin")?;
        let stdout = child.stdout.take().context("lsp stdout")?;
        let stderr = child.stderr.take().context("lsp stderr")?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            diagnostics: DashMap::new(),
            event_tx: event_tx.clone(),
        });
        let pending: Arc<DashMap<i64, Pending>> = Arc::new(DashMap::new());

        let pending_r = pending.clone();
        let shared_r = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = read_loop(stdout, pending_r, shared_r).await {
                warn!("lsp read loop ended: {e:#}");
            }
        });

        let cmd_name = cfg.command.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                debug!("{cmd_name}: {line}");
            }
        });

        let mut client = Self {
            stdin: Arc::new(Mutex::new(Some(stdin))),
            next_id: AtomicI64::new(1),
            pending,
            shared,
            root,
            _child: child,
            initialized: false,
            name: cfg.command.clone(),
            event_rx,
        };
        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&mut self) -> Result<()> {
        let root_uri = url_from_path(&self.root)?;
        let params = InitializeParams {
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    synchronization: Some(TextDocumentSyncClientCapabilities {
                        did_save: Some(true),
                        ..Default::default()
                    }),
                    completion: Some(CompletionClientCapabilities {
                        completion_item: Some(CompletionItemCapability {
                            snippet_support: Some(false),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    hover: Some(HoverClientCapabilities {
                        content_format: Some(vec![MarkupKind::PlainText, MarkupKind::Markdown]),
                        ..Default::default()
                    }),
                    publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                        related_information: Some(true),
                        ..Default::default()
                    }),
                    definition: Some(GotoCapability {
                        link_support: Some(false),
                        ..Default::default()
                    }),
                    formatting: Some(DocumentFormattingClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    rename: Some(RenameClientCapabilities {
                        prepare_support: Some(true),
                        ..Default::default()
                    }),
                    code_action: Some(CodeActionClientCapabilities {
                        code_action_literal_support: Some(CodeActionLiteralSupport {
                            code_action_kind: CodeActionKindLiteralSupport {
                                value_set: vec![
                                    "".into(),
                                    "quickfix".into(),
                                    "refactor".into(),
                                    "source".into(),
                                ],
                            },
                        }),
                        ..Default::default()
                    }),
                    inlay_hint: Some(InlayHintClientCapabilities {
                        dynamic_registration: Some(false),
                        resolve_support: None,
                    }),
                    document_symbol: Some(DocumentSymbolClientCapabilities {
                        hierarchical_document_symbol_support: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                window: Some(WindowClientCapabilities {
                    work_done_progress: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            },
            initialization_options: None,
            process_id: Some(std::process::id()),
            root_uri: Some(root_uri.clone()),
            root_path: Some(self.root.to_string_lossy().into_owned()),
            trace: Some(TraceValue::Off),
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: root_uri,
                name: self
                    .root
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("workspace")
                    .to_string(),
            }]),
            client_info: Some(ClientInfo {
                name: "tui-ide".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            locale: None,
            work_done_progress_params: Default::default(),
        };

        let _ = self
            .request("initialize", serde_json::to_value(params)?)
            .await?;
        self.notify("initialized", json!({})).await?;
        self.initialized = true;
        Ok(())
    }

    pub fn is_ready(&self) -> bool {
        self.initialized
    }

    pub fn diagnostics_for(&self, path: &Path) -> Vec<DiagnosticItem> {
        self.shared
            .diagnostics
            .get(path)
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    pub fn all_diagnostics(&self) -> Vec<DiagnosticItem> {
        let mut out = Vec::new();
        for entry in self.shared.diagnostics.iter() {
            out.extend(entry.value().iter().cloned());
        }
        out
    }

    pub async fn did_open(&self, path: &Path, text: &str, language_id: &str) -> Result<()> {
        let uri = url_from_path(path)?;
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: language_id.into(),
                version: 1,
                text: text.into(),
            },
        };
        self.notify("textDocument/didOpen", serde_json::to_value(params)?)
            .await
    }

    pub async fn did_change(&self, path: &Path, text: &str, version: i32) -> Result<()> {
        let uri = url_from_path(path)?;
        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri, version },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.into(),
            }],
        };
        self.notify("textDocument/didChange", serde_json::to_value(params)?)
            .await
    }

    pub async fn did_save(&self, path: &Path, text: &str) -> Result<()> {
        let uri = url_from_path(path)?;
        let params = DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier { uri },
            text: Some(text.into()),
        };
        self.notify("textDocument/didSave", serde_json::to_value(params)?)
            .await
    }

    pub async fn did_close(&self, path: &Path) -> Result<()> {
        let uri = url_from_path(path)?;
        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri },
        };
        self.notify("textDocument/didClose", serde_json::to_value(params)?)
            .await
    }

    pub async fn completion(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<CompletionItemView>> {
        let uri = url_from_path(path)?;
        let params = CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };
        let value = self
            .request("textDocument/completion", serde_json::to_value(params)?)
            .await?;
        Ok(parse_completion(value, character))
    }

    pub async fn hover(&self, path: &Path, line: u32, character: u32) -> Result<Option<String>> {
        let uri = url_from_path(path)?;
        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
        };
        let value = self
            .request("textDocument/hover", serde_json::to_value(params)?)
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        let hover: Hover = serde_json::from_value(value)?;
        Ok(Some(hover_text(&hover)))
    }

    pub async fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Option<(PathBuf, u32, u32)>> {
        let uri = url_from_path(path)?;
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let value = self
            .request("textDocument/definition", serde_json::to_value(params)?)
            .await?;
        Ok(parse_location(value))
    }

    pub async fn formatting(&self, path: &Path) -> Result<Vec<TextEdit>> {
        let uri = url_from_path(path)?;
        let params = DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri },
            options: FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                properties: Default::default(),
                trim_trailing_whitespace: Some(true),
                insert_final_newline: Some(true),
                trim_final_newlines: Some(true),
            },
            work_done_progress_params: Default::default(),
        };
        let value = self
            .request("textDocument/formatting", serde_json::to_value(params)?)
            .await?;
        if value.is_null() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_value(value)?)
    }

    pub async fn rename(
        &self,
        path: &Path,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<Option<WorkspaceEdit>> {
        let uri = url_from_path(path)?;
        let params = RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            new_name: new_name.into(),
            work_done_progress_params: Default::default(),
        };
        let value = self
            .request("textDocument/rename", serde_json::to_value(params)?)
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(value)?))
    }

    pub async fn code_actions(
        &self,
        path: &Path,
        line: u32,
        character: u32,
        end_line: u32,
        end_character: u32,
        diagnostics: Vec<Diagnostic>,
    ) -> Result<Vec<CodeActionOrCommand>> {
        let uri = url_from_path(path)?;
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range {
                start: Position { line, character },
                end: Position {
                    line: end_line,
                    character: end_character,
                },
            },
            context: CodeActionContext {
                diagnostics,
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let value = self
            .request("textDocument/codeAction", serde_json::to_value(params)?)
            .await?;
        if value.is_null() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_value(value).unwrap_or_default())
    }

    pub async fn inlay_hints(
        &self,
        path: &Path,
        end_line: u32,
    ) -> Result<Vec<InlayHintView>> {
        let uri = url_from_path(path)?;
        let params = InlayHintParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: end_line,
                    character: 0,
                },
            },
            work_done_progress_params: Default::default(),
        };
        let value = self
            .request("textDocument/inlayHint", serde_json::to_value(params)?)
            .await?;
        if value.is_null() {
            return Ok(Vec::new());
        }
        let hints: Vec<InlayHint> = serde_json::from_value(value).unwrap_or_default();
        Ok(hints
            .into_iter()
            .map(|h| InlayHintView {
                line: h.position.line,
                character: h.position.character,
                label: match h.label {
                    InlayHintLabel::String(s) => s,
                    InlayHintLabel::LabelParts(parts) => {
                        parts.into_iter().map(|p| p.value).collect::<String>()
                    }
                },
            })
            .collect())
    }

    pub async fn document_symbols(&self, path: &Path) -> Result<Vec<SymbolItem>> {
        let uri = url_from_path(path)?;
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let value = self
            .request("textDocument/documentSymbol", serde_json::to_value(params)?)
            .await?;
        Ok(parse_symbols(value))
    }

    pub fn poll_events(&mut self) -> Vec<LspEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            out.push(ev);
        }
        out
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, Pending { tx });
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&msg).await?;
        rx.await.context("lsp response canceled")?
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&msg).await
    }

    async fn write_message(&self, msg: &Value) -> Result<()> {
        let body = serde_json::to_vec(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut guard = self.stdin.lock();
        let stdin = guard.as_mut().ok_or_else(|| anyhow!("lsp stdin closed"))?;
        stdin.write_all(header.as_bytes()).await?;
        stdin.write_all(&body).await?;
        stdin.flush().await?;
        Ok(())
    }
}

fn which_bin(name: &str) -> Result<PathBuf> {
    let path = std::env::var_os("PATH").ok_or_else(|| anyhow!("PATH unset"))?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(anyhow!("{name} not found in PATH"))
}

fn url_from_path(path: &Path) -> Result<Uri> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let url = url::Url::from_file_path(&path).map_err(|_| anyhow!("bad file path"))?;
    url.as_str()
        .parse()
        .map_err(|_| anyhow!("uri parse failed"))
}

fn path_from_uri(uri: &Uri) -> Option<PathBuf> {
    let url = url::Url::parse(uri.as_str()).ok()?;
    url.to_file_path().ok()
}

async fn read_loop(
    stdout: impl tokio::io::AsyncRead + Unpin,
    pending: Arc<DashMap<i64, Pending>>,
    shared: Arc<Shared>,
) -> Result<()> {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut headers = HashMap::new();
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(());
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        let len: usize = headers
            .get("content-length")
            .ok_or_else(|| anyhow!("missing Content-Length"))?
            .parse()?;
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).await?;
        let value: Value = serde_json::from_slice(&body)?;
        handle_message(value, &pending, &shared);
    }
}

fn handle_message(value: Value, pending: &DashMap<i64, Pending>, shared: &Shared) {
    if let Some(id) = value.get("id").and_then(|v| v.as_i64()) {
        if value.get("method").is_none() {
            if let Some((_, p)) = pending.remove(&id) {
                if let Some(err) = value.get("error") {
                    let _ = p.tx.send(Err(anyhow!("lsp error: {err}")));
                } else {
                    let result = value.get("result").cloned().unwrap_or(Value::Null);
                    let _ = p.tx.send(Ok(result));
                }
            }
            return;
        }
    }

    let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
    match method {
        "textDocument/publishDiagnostics" => {
            if let Some(params) = value.get("params") {
                if let Ok(p) = serde_json::from_value::<PublishDiagnosticsParams>(params.clone()) {
                    if let Some(path) = path_from_uri(&p.uri) {
                        let items: Vec<DiagnosticItem> = p
                            .diagnostics
                            .into_iter()
                            .map(|d| DiagnosticItem {
                                path: path.clone(),
                                message: d.message,
                                severity: d.severity.unwrap_or(DiagnosticSeverity::INFORMATION),
                                line: d.range.start.line,
                                character: d.range.start.character,
                                end_line: d.range.end.line,
                                end_character: d.range.end.character,
                            })
                            .collect();
                        shared.diagnostics.insert(path.clone(), items.clone());
                        let _ = shared.event_tx.send(LspEvent::Diagnostics { path, items });
                    }
                }
            }
        }
        "window/logMessage" | "window/showMessage" => {
            if let Some(msg) = value.pointer("/params/message").and_then(|m| m.as_str()) {
                let _ = shared
                    .event_tx
                    .send(LspEvent::ServerMessage(msg.to_string()));
            }
        }
        _ => {}
    }
}

fn parse_completion(value: Value, cursor_col: u32) -> Vec<CompletionItemView> {
    let items: Vec<CompletionItem> = if value.is_null() {
        Vec::new()
    } else if let Ok(list) = serde_json::from_value::<CompletionList>(value.clone()) {
        list.items
    } else if let Ok(items) = serde_json::from_value::<Vec<CompletionItem>>(value) {
        items
    } else {
        Vec::new()
    };

    items
        .into_iter()
        .map(|i| {
            let (replace_start, replace_end, insert) = match &i.text_edit {
                Some(CompletionTextEdit::Edit(e)) => (
                    Some(e.range.start.character),
                    Some(e.range.end.character),
                    e.new_text.clone(),
                ),
                Some(CompletionTextEdit::InsertAndReplace(e)) => (
                    Some(e.replace.start.character),
                    Some(e.replace.end.character),
                    e.new_text.clone(),
                ),
                None => (
                    None,
                    Some(cursor_col),
                    i.insert_text.clone().unwrap_or_else(|| i.label.clone()),
                ),
            };
            CompletionItemView {
                label: i.label,
                detail: i.detail,
                insert_text: insert,
                kind: i.kind,
                replace_start_col: replace_start,
                replace_end_col: replace_end,
                filter_text: i.filter_text,
            }
        })
        .collect()
}

fn hover_text(hover: &Hover) -> String {
    match &hover.contents {
        HoverContents::Scalar(s) => marked_string(s),
        HoverContents::Array(arr) => arr.iter().map(marked_string).collect::<Vec<_>>().join("\n"),
        HoverContents::Markup(m) => m.value.clone(),
    }
}

fn marked_string(s: &MarkedString) -> String {
    match s {
        MarkedString::String(s) => s.clone(),
        MarkedString::LanguageString(ls) => ls.value.clone(),
    }
}

fn parse_location(value: Value) -> Option<(PathBuf, u32, u32)> {
    if value.is_null() {
        return None;
    }
    if let Ok(loc) = serde_json::from_value::<Location>(value.clone()) {
        let path = path_from_uri(&loc.uri)?;
        return Some((path, loc.range.start.line, loc.range.start.character));
    }
    if let Ok(locs) = serde_json::from_value::<Vec<Location>>(value.clone()) {
        let loc = locs.into_iter().next()?;
        let path = path_from_uri(&loc.uri)?;
        return Some((path, loc.range.start.line, loc.range.start.character));
    }
    if let Ok(links) = serde_json::from_value::<Vec<LocationLink>>(value) {
        let link = links.into_iter().next()?;
        let path = path_from_uri(&link.target_uri)?;
        return Some((
            path,
            link.target_selection_range.start.line,
            link.target_selection_range.start.character,
        ));
    }
    None
}

fn parse_symbols(value: Value) -> Vec<SymbolItem> {
    if value.is_null() {
        return Vec::new();
    }
    if let Ok(syms) = serde_json::from_value::<Vec<DocumentSymbol>>(value.clone()) {
        let mut out = Vec::new();
        fn walk(syms: &[DocumentSymbol], out: &mut Vec<SymbolItem>, depth: usize) {
            for s in syms {
                let pad = "  ".repeat(depth);
                out.push(SymbolItem {
                    name: format!("{pad}{}", s.name),
                    kind: format!("{:?}", s.kind),
                    line: s.selection_range.start.line,
                    character: s.selection_range.start.character,
                    detail: s.detail.clone(),
                });
                if let Some(children) = &s.children {
                    walk(children, out, depth + 1);
                }
            }
        }
        walk(&syms, &mut out, 0);
        return out;
    }
    if let Ok(infos) = serde_json::from_value::<Vec<SymbolInformation>>(value) {
        return infos
            .into_iter()
            .map(|s| SymbolItem {
                name: s.name,
                kind: format!("{:?}", s.kind),
                line: s.location.range.start.line,
                character: s.location.range.start.character,
                detail: None,
            })
            .collect();
    }
    Vec::new()
}
