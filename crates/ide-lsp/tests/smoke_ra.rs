use ide_config::LanguageServerConfig;
use ide_lsp::LspClient;
use std::time::Duration;

#[tokio::test]
async fn rust_analyzer_initialize() {
    if std::process::Command::new("rust-analyzer")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skip: rust-analyzer not installed");
        return;
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();

    let cfg = LanguageServerConfig {
        command: "rust-analyzer".into(),
        args: vec![],
        languages: vec!["rust".into()],
        file_extensions: vec!["rs".into()],
    };

    let mut client = LspClient::start(&cfg, &root)
        .await
        .expect("start rust-analyzer");
    assert!(client.is_ready());

    let sample = root.join("crates/ide-core/src/lib.rs");
    let text = std::fs::read_to_string(&sample).unwrap();
    client
        .did_open(&sample, &text, "rust")
        .await
        .expect("did_open");

    tokio::time::sleep(Duration::from_millis(1500)).await;
    let _ = client.poll_events();
    let _ = client.all_diagnostics();
}
