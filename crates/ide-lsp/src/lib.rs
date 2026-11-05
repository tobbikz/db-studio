//! Multi-language LSP client manager (JSON-RPC over stdio).

mod client;

pub use client::{CompletionItemView, DiagnosticItem, InlayHintView, LspClient, LspEvent, SymbolItem};
pub use lsp_types;

use anyhow::Result;
use ide_config::{IdeConfig, LanguageServerConfig};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Manages one LSP process per configured server name.
pub struct LspManager {
    root: PathBuf,
    config: IdeConfig,
    clients: HashMap<String, LspClient>,
    /// language_id -> server name
    lang_to_server: HashMap<String, String>,
}

impl LspManager {
    pub fn new(root: PathBuf, config: IdeConfig) -> Self {
        let mut lang_to_server = HashMap::new();
        for (name, cfg) in &config.languages {
            for lang in &cfg.languages {
                lang_to_server.insert(lang.clone(), name.clone());
            }
        }
        Self {
            root,
            config,
            clients: HashMap::new(),
            lang_to_server,
        }
    }

    pub fn server_name_for_lang(&self, language_id: &str) -> Option<&str> {
        self.lang_to_server.get(language_id).map(|s| s.as_str())
    }

    pub async fn ensure_for_language(&mut self, language_id: &str) -> Result<Option<&LspClient>> {
        let Some(server) = self.lang_to_server.get(language_id).cloned() else {
            return Ok(None);
        };
        if !self.clients.contains_key(&server) {
            let Some(cfg) = self.config.languages.get(&server).cloned() else {
                return Ok(None);
            };
            match LspClient::start(&cfg, &self.root).await {
                Ok(client) => {
                    info!("LSP `{server}` ready for {language_id}");
                    self.clients.insert(server.clone(), client);
                }
                Err(e) => {
                    warn!("LSP `{server}` failed: {e:#}");
                    return Ok(None);
                }
            }
        }
        Ok(self.clients.get(&server))
    }

    pub async fn ensure_mut_for_language(
        &mut self,
        language_id: &str,
    ) -> Result<Option<&mut LspClient>> {
        let Some(server) = self.lang_to_server.get(language_id).cloned() else {
            return Ok(None);
        };
        if !self.clients.contains_key(&server) {
            let Some(cfg) = self.config.languages.get(&server).cloned() else {
                return Ok(None);
            };
            match LspClient::start(&cfg, &self.root).await {
                Ok(client) => {
                    self.clients.insert(server.clone(), client);
                }
                Err(e) => {
                    warn!("LSP `{server}` failed: {e:#}");
                    return Ok(None);
                }
            }
        }
        Ok(self.clients.get_mut(&server))
    }

    pub fn client_for(&self, language_id: &str) -> Option<&LspClient> {
        let server = self.lang_to_server.get(language_id)?;
        self.clients.get(server)
    }

    pub fn all_diagnostics(&self) -> Vec<DiagnosticItem> {
        let mut out = Vec::new();
        for c in self.clients.values() {
            out.extend(c.all_diagnostics());
        }
        out.sort_by(|a, b| a.line.cmp(&b.line).then(a.path.cmp(&b.path)));
        out
    }

    pub fn poll_events(&mut self) -> Vec<LspEvent> {
        let mut out = Vec::new();
        for c in self.clients.values_mut() {
            out.extend(c.poll_events());
        }
        out
    }

    pub fn config_ref(&self) -> &IdeConfig {
        &self.config
    }
}

/// Resolve which server config would handle a path extension.
pub fn config_for_path<'a>(
    config: &'a IdeConfig,
    path: &Path,
) -> Option<(&'a str, &'a LanguageServerConfig)> {
    let ext = path.extension().and_then(|e| e.to_str())?;
    config.server_for_extension(ext)
}
