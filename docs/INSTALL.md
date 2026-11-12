# Install & update — DB Studio

Requires a [Rust toolchain](https://rustup.rs/) (1.75+) and a terminal with truecolor + a Nerd Font recommended.

## Linux

```bash
git clone https://github.com/tobbikz/db-studio.git && cd db-studio
./scripts/install.sh
# binary → ~/.local/bin/db-studio
```

Ensure `~/.local/bin` is on `PATH`:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc   # or ~/.zshrc
```

Update later:

```bash
cd db-studio && ./scripts/update.sh
```

## Windows

In PowerShell (Developer / user):

```powershell
git clone https://github.com/tobbikz/db-studio.git
cd db-studio
.\scripts\install.ps1
# binary → %LOCALAPPDATA%\db-studio\db-studio.exe
```

Reopen the terminal after first install so `PATH` picks up the folder.

Update:

```powershell
cd db-studio
.\scripts\update.ps1
```

## From source (any OS)

```bash
cargo build -p ide-app --release
./target/release/db-studio          # Unix
.\target\release\db-studio.exe      # Windows
```

## Config

User config and connections: `~/.config/tui-ide/` (Linux/macOS) or `%APPDATA%\tui-ide\` (Windows via `dirs` crate).

Themes: `…/tui-ide/themes/*.toml` — see [THEMES.md](./THEMES.md).
