# Sessions architecture (TECH-TUI-SESS-V1-ARCH)

## Crate
- `ide-sessions` — profiles + connect helpers
- Reuses `ide-term::TerminalPane::spawn_command` for SSH
- RDP: `std::process::Command` → `xfreerdp` / `wlfreerdp` / `remmina` (`which`)

## Store
`~/.config/tui-ide/sessions.toml` — list of profiles. **No password fields**; identity file path + ssh-agent only. Save refuses content containing `password`.

## Profile fields
| Field | SSH | RDP |
|-------|-----|-----|
| name, host, user, port | ✓ | ✓ |
| identity_file | ✓ | — |
| jump_host (`ssh -J`) | ✓ | — |
| gateway | — | ✓ |
| rdp_client preference | — | ✓ |

## Host chrome
- Sidebar `Sessions` + focus `Sessions`
- Chord `Ctrl-Shift-S` / palette **Sessions**
- SSH live sessions → bottom Terminal panel with multi-tabs
- RDP → external window; status running/stopped

## Non-goals
RDP/VNC framebuffer in ratatui, mosh, Termius clone.
