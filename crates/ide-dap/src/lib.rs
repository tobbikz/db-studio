//! Minimal Debug Adapter Protocol client (stdio JSON-RPC-ish DAP framing).

use anyhow::{Context, Result, anyhow};
use dashmap::DashMap;
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
pub struct Breakpoint {
    pub path: PathBuf,
    pub line: u32, // 1-based
    pub verified: bool,
}

#[derive(Debug, Clone)]
pub struct StackFrameView {
    pub id: i64,
    pub name: String,
    pub path: Option<String>,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct VariableView {
    pub name: String,
    pub value: String,
    pub ty: Option<String>,
}

#[derive(Debug, Clone)]
pub enum DapEvent {
    Stopped { reason: String, thread_id: i64 },
    Output(String),
    Terminated,
    Exited(i64),
}

struct Pending {
    tx: oneshot::Sender<Result<Value>>,
}

pub struct DapClient {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: AtomicI64,
    pending: Arc<DashMap<i64, Pending>>,
    breakpoints: Arc<Mutex<HashMap<PathBuf, Vec<u32>>>>,
    pub event_rx: mpsc::UnboundedReceiver<DapEvent>,
    event_tx: mpsc::UnboundedSender<DapEvent>,
    _child: Child,
    pub stopped: bool,
}

impl DapClient {
    /// Start `lldb-dap` or `codelldb` / `rust-lldb` style adapter. Default: `lldb-dap`.
    pub async fn start(adapter: &str, cwd: &Path) -> Result<Self> {
        let mut child = Command::new(adapter)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn DAP adapter `{adapter}`"))?;

        let stdin = child.stdin.take().context("dap stdin")?;
        let stdout = child.stdout.take().context("dap stdout")?;
        let stderr = child.stderr.take().context("dap stderr")?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let pending: Arc<DashMap<i64, Pending>> = Arc::new(DashMap::new());
        let pending_r = pending.clone();
        let event_tx_r = event_tx.clone();

        tokio::spawn(async move {
            if let Err(e) = read_loop(stdout, pending_r, event_tx_r).await {
                warn!("dap read loop: {e:#}");
            }
        });
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                debug!("dap stderr: {line}");
            }
        });

        let client = Self {
            stdin: Arc::new(Mutex::new(Some(stdin))),
            next_id: AtomicI64::new(1),
            pending,
            breakpoints: Arc::new(Mutex::new(HashMap::new())),
            event_rx,
            event_tx,
            _child: child,
            stopped: false,
        };

        let _ = client
            .request(
                "initialize",
                json!({
                    "clientID": "tui-ide",
                    "clientName": "tui-ide",
                    "adapterID": "lldb",
                    "pathFormat": "path",
                    "linesStartAt1": true,
                    "columnsStartAt1": true,
                    "supportsVariableType": true,
                    "supportsVariablePaging": false,
                }),
            )
            .await?;
        Ok(client)
    }

    pub async fn launch(&self, program: &Path, args: &[String]) -> Result<()> {
        let _ = self
            .request(
                "launch",
                json!({
                    "program": program,
                    "args": args,
                    "cwd": program.parent().map(|p| p.display().to_string()),
                    "name": "tui-ide-debug",
                }),
            )
            .await;
        // Many adapters want configurationDone
        let _ = self.request("configurationDone", json!({})).await;
        Ok(())
    }

    pub fn toggle_breakpoint(&self, path: &Path, line: u32) -> Vec<u32> {
        let mut map = self.breakpoints.lock();
        let entry = map.entry(path.to_path_buf()).or_default();
        if let Some(pos) = entry.iter().position(|l| *l == line) {
            entry.remove(pos);
        } else {
            entry.push(line);
            entry.sort_unstable();
        }
        entry.clone()
    }

    pub fn breakpoints_for(&self, path: &Path) -> Vec<u32> {
        self.breakpoints
            .lock()
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn apply_breakpoints(&self, path: &Path) -> Result<()> {
        let lines = self.breakpoints_for(path);
        let bps: Vec<Value> = lines
            .iter()
            .map(|l| json!({ "line": l }))
            .collect();
        let _ = self
            .request(
                "setBreakpoints",
                json!({
                    "source": { "path": path },
                    "breakpoints": bps,
                    "sourceModified": false,
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn continue_exec(&mut self, thread_id: i64) -> Result<()> {
        self.request("continue", json!({ "threadId": thread_id }))
            .await?;
        self.stopped = false;
        Ok(())
    }

    pub async fn next(&mut self, thread_id: i64) -> Result<()> {
        self.request("next", json!({ "threadId": thread_id })).await?;
        Ok(())
    }

    pub async fn step_in(&mut self, thread_id: i64) -> Result<()> {
        self.request("stepIn", json!({ "threadId": thread_id }))
            .await?;
        Ok(())
    }

    pub async fn step_out(&mut self, thread_id: i64) -> Result<()> {
        self.request("stepOut", json!({ "threadId": thread_id }))
            .await?;
        Ok(())
    }

    pub async fn stack_trace(&self, thread_id: i64) -> Result<Vec<StackFrameView>> {
        let body = self
            .request(
                "stackTrace",
                json!({ "threadId": thread_id, "startFrame": 0, "levels": 30 }),
            )
            .await?;
        let frames = body
            .get("stackFrames")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(frames
            .into_iter()
            .filter_map(|f| {
                Some(StackFrameView {
                    id: f.get("id")?.as_i64()?,
                    name: f.get("name")?.as_str()?.to_string(),
                    path: f
                        .pointer("/source/path")
                        .and_then(|p| p.as_str())
                        .map(|s| s.to_string()),
                    line: f.get("line")?.as_u64()? as u32,
                })
            })
            .collect())
    }

    pub async fn variables_for_frame(&self, frame_id: i64) -> Result<Vec<VariableView>> {
        let scopes = self
            .request("scopes", json!({ "frameId": frame_id }))
            .await?;
        let scopes = scopes
            .get("scopes")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for scope in scopes.iter().take(3) {
            let Some(vars_ref) = scope.get("variablesReference").and_then(|v| v.as_i64()) else {
                continue;
            };
            if vars_ref <= 0 {
                continue;
            }
            let vars = self
                .request("variables", json!({ "variablesReference": vars_ref }))
                .await
                .unwrap_or(json!({}));
            if let Some(arr) = vars.get("variables").and_then(|v| v.as_array()) {
                for v in arr {
                    out.push(VariableView {
                        name: v
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("?")
                            .to_string(),
                        value: v
                            .get("value")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string(),
                        ty: v
                            .get("type")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string()),
                    });
                }
            }
        }
        Ok(out)
    }

    pub fn poll_events(&mut self) -> Vec<DapEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            if matches!(ev, DapEvent::Stopped { .. }) {
                self.stopped = true;
            }
            out.push(ev);
        }
        out
    }

    async fn request(&self, command: &str, arguments: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, Pending { tx });
        let msg = json!({
            "seq": id,
            "type": "request",
            "command": command,
            "arguments": arguments,
        });
        self.write_message(&msg).await?;
        rx.await.context("dap response canceled")?
    }

    async fn write_message(&self, msg: &Value) -> Result<()> {
        let body = serde_json::to_vec(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut guard = self.stdin.lock();
        let stdin = guard.as_mut().ok_or_else(|| anyhow!("dap stdin closed"))?;
        stdin.write_all(header.as_bytes()).await?;
        stdin.write_all(&body).await?;
        stdin.flush().await?;
        Ok(())
    }
}

async fn read_loop(
    stdout: impl tokio::io::AsyncRead + Unpin,
    pending: Arc<DashMap<i64, Pending>>,
    event_tx: mpsc::UnboundedSender<DapEvent>,
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
        let typ = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match typ {
            "response" => {
                if let Some(id) = value.get("request_seq").and_then(|v| v.as_i64()) {
                    if let Some((_, p)) = pending.remove(&id) {
                        let ok = value.get("success").and_then(|s| s.as_bool()).unwrap_or(false);
                        if ok {
                            let body = value.get("body").cloned().unwrap_or(Value::Null);
                            let _ = p.tx.send(Ok(body));
                        } else {
                            let msg = value
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("dap error");
                            let _ = p.tx.send(Err(anyhow!("{msg}")));
                        }
                    }
                }
            }
            "event" => {
                let event = value.get("event").and_then(|e| e.as_str()).unwrap_or("");
                let body = value.get("body").cloned().unwrap_or(Value::Null);
                match event {
                    "stopped" => {
                        let reason = body
                            .get("reason")
                            .and_then(|r| r.as_str())
                            .unwrap_or("stopped")
                            .to_string();
                        let thread_id = body.get("threadId").and_then(|t| t.as_i64()).unwrap_or(1);
                        let _ = event_tx.send(DapEvent::Stopped { reason, thread_id });
                    }
                    "output" => {
                        if let Some(out) = body.get("output").and_then(|o| o.as_str()) {
                            let _ = event_tx.send(DapEvent::Output(out.to_string()));
                        }
                    }
                    "terminated" => {
                        let _ = event_tx.send(DapEvent::Terminated);
                    }
                    "exited" => {
                        let code = body.get("exitCode").and_then(|c| c.as_i64()).unwrap_or(0);
                        let _ = event_tx.send(DapEvent::Exited(code));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Best-effort discover a DAP adapter binary.
pub fn find_adapter() -> Option<&'static str> {
    for name in ["lldb-dap", "codelldb", "llvm-vscode"] {
        if which(name).is_some() {
            return Some(name);
        }
    }
    None
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let c = dir.join(name);
        if c.is_file() {
            return Some(c);
        }
    }
    None
}
