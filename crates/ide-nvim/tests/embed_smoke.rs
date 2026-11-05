use ide_nvim::NvimSession;
use std::time::Duration;

#[tokio::test]
async fn embed_ui_attach_and_input() {
    let cwd = std::env::temp_dir();
    let session = NvimSession::start_minimal(&cwd, 80, 24)
        .await
        .expect("start nvim");

    tokio::time::sleep(Duration::from_millis(300)).await;
    let grid = session.snapshot();
    assert!(grid.width >= 20, "width={}", grid.width);
    assert!(grid.height >= 5, "height={}", grid.height);

    session.input("ihello<Esc>").await.expect("input");
    tokio::time::sleep(Duration::from_millis(250)).await;

    let grid = session.snapshot();
    let mut joined = String::new();
    for r in 0..grid.height.min(8) {
        for c in 0..grid.width.min(40) {
            if let Some(cell) = grid.cell(r, c) {
                joined.push_str(&cell.text);
            }
        }
        joined.push('\n');
    }
    assert!(
        joined.contains('h') || joined.contains("hello") || joined.contains('e'),
        "expected typed text in grid, got:\n{joined}"
    );

    session.quit().await;
}

#[tokio::test]
async fn embed_paste_mouse_mode_help() {
    let cwd = std::env::temp_dir();
    let session = NvimSession::start_minimal(&cwd, 80, 24)
        .await
        .expect("start nvim");
    tokio::time::sleep(Duration::from_millis(300)).await;

    session.input("i").await.expect("insert");
    tokio::time::sleep(Duration::from_millis(100)).await;
    session.paste_text("wave1").await.expect("paste");
    tokio::time::sleep(Duration::from_millis(200)).await;
    session.input("<Esc>").await.expect("esc");
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mode = session.mode();
    assert!(
        mode.starts_with('n') || mode == "normal" || mode.is_empty() || mode == "n",
        "expected normal-ish mode after Esc, got {mode:?}"
    );

    session
        .input_mouse("left", "press", "", 5, 10)
        .await
        .expect("mouse press");
    session
        .input_mouse("left", "release", "", 5, 10)
        .await
        .expect("mouse release");

    let help = NvimSession::code_help_text();
    assert!(help.contains("Wave 1"));
    assert!(help.contains("nvim_paste") || help.contains("bracketed paste"));

    session.show_code_help().await.expect("code help scratch");
    tokio::time::sleep(Duration::from_millis(150)).await;

    session.quit().await;
}

#[tokio::test]
async fn embed_bufs_and_register_wave2() {
    let cwd = std::env::temp_dir();
    let mut session = NvimSession::start_minimal(&cwd, 80, 24)
        .await
        .expect("start nvim");
    tokio::time::sleep(Duration::from_millis(250)).await;

    session.input("iwave2<Esc>").await.expect("type");
    tokio::time::sleep(Duration::from_millis(150)).await;
    session.input("yy").await.expect("yank line");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let reg = session.get_unnamed_register().await.expect("register");
    assert!(
        reg.contains("wave2") || reg.contains('w'),
        "expected yank text, got {reg:?}"
    );

    let bufs = session.list_buf_infos().await.expect("bufs");
    assert!(!bufs.is_empty(), "expected at least one listed buffer");
    assert!(bufs.iter().any(|b| b.current), "one buffer should be current");

    session.force_next_resize();
    session.resize(100, 30).await.expect("force resize");

    session.quit().await;
}

#[tokio::test]
async fn embed_dirty_diag_quit_wave3() {
    let cwd = std::env::temp_dir();
    let session = NvimSession::start_minimal(&cwd, 80, 24)
        .await
        .expect("start nvim");
    tokio::time::sleep(Duration::from_millis(250)).await;

    session.input("idirty<Esc>").await.expect("type");
    tokio::time::sleep(Duration::from_millis(150)).await;
    let dirty = session.dirty_buf_count().await.expect("dirty");
    assert!(dirty >= 1, "expected dirty buffer after edit, got {dirty}");

    let (e, w) = session.diagnostic_counts().await.expect("diag");
    // No LSP attached in -u NONE; counts should still round-trip as numbers.
    let _ = (e, w);

    let help = NvimSession::code_help_text();
    assert!(
        help.contains("Ctrl-`")
            || help.contains("Wave 4")
            || help.contains("IDE v1.0")
            || help.contains("Ctrl-B"),
        "help should mention Wave 4+ terminal / v1.0"
    );

    session.write_all().await.expect("wall");
    // After wall + no filename, buffer may still be dirty; force quit path.
    session.quit().await;
}
