# db-studio

Terminal **Database Studio** — explorer, structure, SQL console, and results.

AMOLED mono TUI. No code editor, no Sessions product chrome.

## Run

```bash
cargo run -p ide-app --release
# binary: target/release/db-studio [workspace]
```

## Surfaces

| Surface | How |
|---------|-----|
| **Home** | `Ctrl-⇧H` — sources + connect |
| **Studio** | `e` / `Ctrl-⇧D` — explorer · structure · console · results |

## Studio keys

| Key | Action |
|-----|--------|
| `1`–`4` | Explorer · Structure · Console · Results |
| `Tab` | Cycle panes |
| `f` | Fullscreen pane |
| `Esc` | Back to explorer (or cancel query) |
| `a` | New connection |
| `F5` / `Ctrl-↵` | Run SQL |
| `y` | Copy focused result cell |
| `i` | Edit cell |
| `/` | Filter results |

## Config

Connections live under `~/.config/tui-ide/` for now (shared toolkit path).

## License

Apache-2.0
