# DB Studio

Terminal **Database Studio** — Home hub, explorer, structure, SQL console, and results grid.

Truecolor TUI with DataRig-inspired themes (AMOLED mono, Nord, Catppuccin, Tokyo Night, Dracula, Light, customs).

**Version:** 0.5.0

## Install

| OS | Command |
|----|---------|
| **Linux** | `./scripts/install.sh` → `~/.local/bin/db-studio` |
| **Windows** | `.\scripts\install.ps1` → `%LOCALAPPDATA%\db-studio\` |
| **Update** | `./scripts/update.sh` / `.\scripts\update.ps1` |

Full guide: [docs/INSTALL.md](docs/INSTALL.md). Themes: [docs/THEMES.md](docs/THEMES.md).

```bash
# from source
cargo build -p ide-app --release
./target/release/db-studio
```

## Quick keys (Studio)

| Key | Action |
|-----|--------|
| `Ctrl-P` | Command palette |
| `Ctrl-W` | Close SQL tab |
| `Space` | Discover next keys |
| `Shift-T` | Cycle theme |
| `1`–`4` | Explorer · Structure · Console · Results |
| `Tab` | Cycle panes |
| `F5` / `Ctrl-↵` | Run SQL |
| `Esc` | Cancel / back / Home |
| `?` | Help |

Home: `a` add connection · `Enter` open Studio · `?` help.

## Themes

Palette (`Ctrl-P`) → filter `theme`:

- **AMOLED mono** / **AMOLED light mono**
- **Dark** (default) · Nord · Mocha · Tokyo Night · Dracula · **Light** · High contrast
- **Theme: new custom…** → TOML under `~/.config/tui-ide/themes/`

↑↓ previews · Enter keeps · Esc restores.

## Drivers

Postgres · MySQL · SQLite · DuckDB · and more in the connection form.

Passwords: prefer `env:VAR` or vault — plaintext is not required.

## Config

`~/.config/tui-ide/` (Linux) · `%APPDATA%\tui-ide\` (Windows).

## Docs

| Doc | Topic |
|-----|--------|
| [docs/INSTALL.md](docs/INSTALL.md) | Linux / Windows install & update |
| [docs/THEMES.md](docs/THEMES.md) | Built-ins + custom TOML |
| [docs/AGENTS.md](docs/AGENTS.md) | Agents surface |
| [docs/SESSIONS.md](docs/SESSIONS.md) | Sessions |

## License

Apache-2.0
