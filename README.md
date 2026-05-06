# matcurses

A keyboard-driven, braille-friendly terminal client for [Matrix](https://matrix.org), written in Rust on top of [ratatui](https://ratatui.rs) and the [matrix-rust-sdk](https://github.com/matrix-org/matrix-rust-sdk).

## What it is

- **TUI Matrix client.** End-to-end encryption, multi-room sessions, threads, reactions, voice notes, redactions, search.
- **irssi-style multi-window model.** Each open room is a window; cycle through them, see activity markers in the status line, persist the layout across restarts.
- **Designed for screen readers and braille displays.** The cursor lands on a deterministic, semantically-meaningful column for every widget (button label, checkbox bracket, list row); no hidden focus, no animated state. Everything is reachable from the keyboard.
- **Native voice notes.** OGG/Opus playback via `libopus` + `rodio`, inline in the conversation.
- **Optional integrations.** OS keychain for the E2EE recovery key, an arbitrary password-manager command (`bw`, `pass`, `op`, …) for unattended restore, and `$EDITOR` for composing or reading long messages.

## Install / run

### Prerequisites

- Rust (stable, edition 2021).
- `libopus` headers (voice notes):
  - macOS: `brew install opus`
  - Debian / Ubuntu: `sudo apt install libopus-dev pkg-config`
  - Fedora: `sudo dnf install opus-devel pkgconf-pkg-config`

### Build

```sh
git clone https://github.com/appsuite-eu/matcurses.git
cd matcurses
cargo build --release
```

The binary lands at `target/release/matcurses`.

### Run

```sh
cargo run --release
# or
./target/release/matcurses
```

Press `Esc` to quit; any quit confirmation prompt walks you through it.

### Configuration

Settings live at `~/.config/matcurses/settings.toml` (or the platform-equivalent config directory). Open the in-app settings form with `,` from the conversation view; `Enregistrer` writes the file. The form covers TTS, NATO alphabet for keys/SAS, voice-note mode, `$EDITOR` path, keychain persistence of the recovery key, password-manager command, event sounds, multi-line input, and "reopen last windows on startup".

## Keyboard shortcuts

### Global (work from any focus)

| Key | Action |
|-----|--------|
| `F3` | Open spaces tree |
| `F4` | Open rooms list |
| `F5` | Open members list |
| `Alt+1` … `Alt+9` | Switch to window N |
| `Alt+n` / `Alt+p` | Next / previous window |
| `Ctrl+N` / `Ctrl+P` | Resume last search (next / previous match) |

### Conversation focus

| Key | Action |
|-----|--------|
| `i` or `Enter` | Move focus to the input bar |
| `Up` / `Down` | Previous / next message |
| `PgUp` / `PgDn` | Page up / down |
| `Home` / `End`, `g` / `G` | First / last message |
| `Right` or `+` | Expand thread |
| `Left` or `-` | Collapse thread |
| `[` / `]` | Jump to previous / next date boundary |
| `u` | Jump to next unread message (focus auto-marks read) |
| `<` / `>` | Previous / next window |
| `r` | Reply to selected message |
| `t` | Start (or continue) a thread on the selected message |
| `R` (Shift+r) | Reaction picker |
| `d` | Message details (reactions, event id, raw content) |
| `D` (Shift+d) | Redact (delete) selected message |
| `v` / `V` | Play / stop voice note |
| `Space` | Pause / resume the active voice note |
| `Esc` | Stop the active voice note |
| `(` / `)` | Slow down / speed up voice playback (0.25× steps, 0.5×–2.0×) |
| `e` | Open the message body in `$EDITOR` |
| `/` / `?` | Search forward / backward |
| `,` | Settings |
| `q` | Quit (confirm) |

### Input focus

| Key | Action |
|-----|--------|
| `Esc` | Back to conversation; drop pending reply / thread target |
| `Enter` | Send (or insert newline if multi-line input is on) |
| `Ctrl+S` | Send (always, regardless of multi-line setting) |
| `Up` on first line | Back to conversation |
| `Up` / `Down` | Move between lines (multi-line) |
| `Left` / `Right` | Character cursor |
| `Home` / `End`, `Ctrl+A` / `Ctrl+E` | Line start / end |
| `Ctrl+Home` / `Ctrl+End` | Buffer start / end |
| `Ctrl+K` | Kill to end of line |
| `Backspace` / `Delete` | Delete previous / next character |
| `Tab` / `Shift+Tab` | Autocomplete (`/command` or `@user`); cycle |
| `Ctrl+G` | Open `$EDITOR` with the current input as initial content |

### Modals

| Key | Action |
|-----|--------|
| `Esc` (or `q` for read-only modals) | Close |
| `Enter` | Confirm / activate |
| `Up` / `Down`, `Tab` / `Shift+Tab` | Navigate fields or list rows |

### Slash commands (typed in the input bar)

| Command | Description |
|---------|-------------|
| `/quit`, `/q` | Quit |
| `/help`, `/h`, `/?` | Help |
| `/version` | Show version |
| `/me <action>` | Send an emote |
| `/join <room>`, `/j` | Join a room |
| `/rooms [server]`, `/discover [server]` | Browse the public room directory of `server` (or local) |
| `/spaces [server]` | Browse the public spaces directory of `server` (or local) |
| `/leave`, `/part` | Leave the active room |
| `/redact`, `/del` | Redact the selected message |
| `/react <emoji>` | React to the selected message |
| `/restore` | Restore E2EE keys from a recovery key |
| `/setup`, `/enable-recovery` | Provision a fresh recovery key |
| `/recovery` | Show current recovery state |
| `/verify <user>` | Start an interactive (SAS) verification |
| `/logout` | Log out and forget the session |
| `/window N`, `/win`, `/w` | Switch / list windows |

Send a literal `/foo` as a regular message by typing `//foo`.

## License

matcurses is released under the **GNU General Public License version 3 or later** (GPL-3.0-or-later).

You are free to use, study, share, and modify the program; if you distribute modified versions you must do so under the same license and make the source available. There is no warranty — see the full license text at <https://www.gnu.org/licenses/gpl-3.0.html>.
