# Agents architecture (TECH-TUI-AGENTS-V1)

## Crate

- `ide-agents` — profiles + PTY launch helpers
- Reuses `ide-term::TerminalPane::spawn_command` (same path as Sessions SSH)

## Config

`~/.config/tui-ide/agents.toml` — list of profiles.

**Never** store API keys / passwords / tokens in TOML. Use env (`CURSOR_API_KEY`, Claude/OpenCode env, etc.).

### Profile fields

| Field | Meaning |
|-------|---------|
| `name` | Display label |
| `kind` | `cursor` · `claude` · `opencode` · `custom` |
| `binary` | Optional override (`agent`, `claude`, …) |
| `args` | Extra argv |
| `seed_prompt` | Optional text typed into the PTY after spawn |

Defaults on first open: Cursor Agent, Claude Code, OpenCode.

## Host chrome

- Sidebar **Agents** tab (`Sidebar::Agents` / `Focus::Agents`)
- Open: `Ctrl-Shift-A`, Home `g`, palette **Agents**
- Enter → spawn CLI in bottom Terminal (multi-tab `Ctrl-]` / `Ctrl-[` / `Ctrl-W`)
- CRUD: `a` add · `e` edit · `x` delete

## Code vs DB

- **Code:** cwd = workspace root
- **DB:** same cwd + optional seed blurb with driver/host/port/database (no secrets)

## Non-goals (v1)

- Custom ratatui chat UI / Cursor SDK embed
- Parsing `--print` / stream-json into host panels
- Installing third-party CLIs
