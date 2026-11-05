# db-studio

Terminal **Database Studio** — explorer, structure, SQL console, and results.

Spun out of [tui-ide](https://github.com/tobbikz/tui-ide) as a focused product (no code editor / AstroNvim).

## Run

```bash
cargo run -p ide-app --release
# binary: target/release/db-studio [workspace]
```

## Keys (studio)

| Key | Pane |
|-----|------|
| `1` | Explorer |
| `2` | Structure |
| `3` | Console |
| `4` | Results |
| `Tab` | Cycle panes |
| `f` | Fullscreen pane |
| `Esc` | Back to explorer |

## Config

Uses `~/.config/tui-ide/` connection profiles for now (shared with the parent toolkit).

## License

Apache-2.0
