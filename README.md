# db-studio

Terminal **Database Studio** — Home hub, explorer, structure, SQL console, and results.

AMOLED mono TUI. Database product only (no Code / Sessions chrome).

## Run

```bash
cargo run -p ide-app --release
# binary: target/release/db-studio [workspace]
```

Release helper:

```bash
./scripts/release.sh
# → target/release/db-studio
```

## Surfaces

| Surface | How |
|---------|-----|
| **Home** | boot · `Ctrl-⇧H` · Esc from Studio Explorer |
| **Studio** | Enter / `1–9` source · or Recents / Favorites |

## Home keys

| Key | Action |
|-----|--------|
| `Tab` | Sources · Recents · Favorites |
| `j` / `k` | Move selection |
| `Enter` / `e` | Connect → Studio (or load recent SQL) |
| `1`–`9` | Open source slot |
| `a` | Add connection |
| `p` | Pin / unpin source |
| `r` | Refresh health |
| `?` | Help overlay |

## Studio keys

| Key | Action |
|-----|--------|
| `1`–`4` | Explorer · Structure · Console · Results |
| `Tab` | Cycle panes |
| `f` | Fullscreen pane |
| `Esc` | Pane → Explorer → **Home** (cancels running query) |
| `Ctrl-]` / `Ctrl-[` | Switch connection |
| `F5` / `Ctrl-↵` | Run SQL |
| `D` | Results density (compact / comfort) |
| `y` | Copy focused result cell |
| `i` | Edit cell |
| `/` | Filter results |
| `Ctrl-⇧E` | Export CSV |
| `Ctrl-⇧L` | Export SQL |
| `?` | Help overlay |

## Drivers & SSH

Postgres · MySQL · SQLite · DuckDB · and more in the connection form (`←`/`→` on Driver).

Optional **SSH jump**: set SSH host / user / port / key on the connection form (empty host = direct).

Passwords: prefer `env:VAR` or vault — plaintext is not required.

## Config

Connections live under `~/.config/tui-ide/` (shared toolkit path for now).

## License

Apache-2.0
