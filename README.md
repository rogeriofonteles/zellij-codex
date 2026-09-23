# zellij-codex

Codex status badges for Zellij tabs and pane titles, with an optional legacy
WASM dashboard.

## Reproduce this setup on another machine

The configuration is versioned here, including the changes that otherwise live
in `~/.config/zellij` and the Neovim workbench checkout. Templates use placeholders
instead of a particular username or checkout path. Run the installer on each
machine; copying an already-rendered `config.kdl` retains that machine's paths.

Requirements: Zellij 0.44.3 (the tested plugin API), a Rust toolchain with the
`wasm32-wasip1` target, Python 3.11+, and a Codex CLI with lifecycle hooks enabled.
The scripts target Linux/macOS, or WSL on Windows. Put `~/.local/bin` on `PATH`.

```sh
git clone https://github.com/rogeriofonteles/zellij-codex.git
cd zellij-codex
rustup target add wasm32-wasip1
cargo build --locked --release --target wasm32-wasip1
./scripts/install
./scripts/install-tab-bar
./scripts/install-zellij-config
zellij setup --check
```

`install-zellij-config` **replaces your Zellij configuration** with this setup.
It saves each original file beside it as `NAME.before-zellij-codex` before the
first replacement; rerunning it preserves that backup. It uses `$XDG_CONFIG_HOME`
or `~/.config`, resolves local paths, and selects fish when available, otherwise
`$SHELL`. Pass `--shell /path/to/shell` to choose a shell explicitly. The other
installers merge Codex hooks and install helpers; credentials, trust decisions,
session state, and compiled plugins are not copied from another machine.

Start a new Zellij session for the complete setup. Review and allow the tab-bar
plugin's permissions (read state, change state, run commands), then start Codex
and review its hooks with `/hooks`. The installed fish `codex` function uses
the launch wrapper; in other shells run `zellij-codex-launch` explicitly.

| Versioned file | Installed destination / purpose |
| --- | --- |
| `config/zellij/config.kdl` | Zellij `config.kdl`: bindings, clipboard, shell, default layout |
| `config/zellij/layouts/default.kdl` | Zellij `layouts/default.kdl`: theme-styled active tab with emoji status badges |
| `config/zellij/workbench_keybinds.kdl` | Optional Alt+T and Ctrl+K bindings merged into `config.kdl` |
| `config/workbench_layouts/*.kdl` | Optional workbench layouts: custom tab bar and automatic Codex pane titles |

Ctrl+T, N explicitly loads the installed default layout. Every bundled layout
loads the custom tab bar by its full `file:` URL, avoiding stale plugin aliases in
existing sessions. This keeps the theme styling and emoji badges consistent across
ordinary tabs and workbench tabs. Alt+A acknowledges only the selected done pane;
Ctrl+Tab and Ctrl+Shift+Tab switch tabs.

### Optional Neovim and worktree integration

Alt+T and Ctrl+K require the separate
[Neovim workbench](https://github.com/rogeriofonteles/nvim-zellij-workbench) and
[worktree plugin fork](https://github.com/rogeriofonteles/zellij-worktree).
Follow the workbench's requirements for Neovim and its tools. For sibling checkouts:

```sh
git clone https://github.com/rogeriofonteles/nvim-zellij-workbench.git ../nvim-zellij-workbench
git clone https://github.com/rogeriofonteles/zellij-worktree.git ../zellij-worktree
(cd ../zellij-worktree && cargo build --release --target wasm32-wasip1)
./scripts/install-zellij-config \
  --workbench-root ../nvim-zellij-workbench \
  --worktree-plugin ../zellij-worktree/target/wasm32-wasip1/release/zellij-worktree.wasm
```

The installer copies the worktree plugin and renders the three bundled workbench
layouts into that checkout, backing up existing layouts. Alt+T types the workbench
launch command into the focused shell, so use it at a shell prompt. Ctrl+K opens
the worktree picker. Neither requires the checkouts to live at a fixed path.
Workbench panes have no fixed `Codex` name, allowing Codex's conversation title
to appear when no custom pane name overrides it.

To upgrade, pull this repository, rebuild, and rerun the same installer commands,
including the optional workbench arguments if used. To update native tab bars in
an existing session without restarting terminals, run
`./scripts/install-tab-bar --session SESSION`. The Ctrl+T, N binding reloads with
the config; the session's other default-layout settings take effect after restart.

### Existing Codex sessions after a hook upgrade

An already-running Codex process can keep its previous hook configuration even
when `hooks.json` and the trusted hook settings on disk are up to date. Its pane
can keep showing idle or done while Codex is working. Refresh each affected
Codex process; refreshing Zellij or reinstalling the reporter alone is insufficient.

Without interrupting the current task, open `/hooks`, select `PostToolUse`, and
open the handler whose command ends in `zellij-codex-hook`. If it is already
trusted and enabled, press Space to disable it, then Space again to enable it.
Confirm it is checked, then press Esc twice to return to the conversation. The
toggle reloads that process's hook configuration, and its next lifecycle event
updates the badges. Review new or modified hooks before enabling them. Another
option is to exit and resume Codex after its current task finishes.

The badge reporter does not infer status from screen text: if no lifecycle event
arrives, `--refresh` only recomputes existing statuses and cannot detect running work.

## Status badges (default)

Run `./scripts/install`, then start a new Codex conversation and review its
updated hooks. No resident monitor, polling, or dashboard is needed.
The short-lived reporter runs only when a Codex lifecycle hook fires.

| State | Badge |
| --- | --- |
| Idle | Original title, no badge |
| Running | `[🟠 ↻]` |
| Waiting for input or approval | `[🔴 !]` |
| Done | `[🔵 ✓]` |

Badges appear beside the existing tab and pane names. The custom tab bar retains
Zellij's native theme colors and displays the same Unicode emoji markers as pane
headers. Emoji colors depend on the terminal's color emoji font support.
A tab summarizes its agents, prioritizing input, error, stuck, done, running, paused, then idle.
`PreToolUse` detects `request_user_input`; `PermissionRequest` detects approvals;
`PostToolUse` resumes running. Alt+A clears the completed badge only on the selected pane. The tab keeps its
done badge until the last completed pane is cleared; running/input badges remain.
One done pane plus one running pane shows done. A new prompt immediately marks
its pane running; if no done panes remain, the tab shows running. Clearing the
last done pane with Alt+A also reveals another pane's running status.
Done also clears on the next
prompt, session start, or session end. Plain-text questions without an input-tool event
appear as done when the turn ends.

The reporter decorates Zellij titles through its native rename commands. Title
changes made by the user are retained on the next report. While decorated,
terminal-generated OSC titles are overridden by the pane name. Closing or moving
a pane is reconciled at the next lifecycle report, or immediately with
`~/.local/bin/zellij-codex-badges --refresh`. State is isolated by the session
socket identity and updates are serialized across simultaneous hooks.

To disable the legacy dashboard, remove its Alt+A `MessagePlugin` binding and
the `zellij-codex.wasm` entry with `role "monitor"` from Zellij's `load_plugins`
configuration, and stop existing dashboard/monitor instances with
`zellij pipe --name stop_dashboard -- ''` (requires the current WASM build).
Badge hooks never launch that plugin.

For emoji tab badges with pane-specific Alt+A clearing, build and install the tab bar:

```sh
cargo build --release --target wasm32-wasip1 --bin zellij-codex-tab-bar
./scripts/install-tab-bar
```

The portable installer above writes direct plugin URLs into its layouts.
For other layouts, pass `--layout /path/to/layout.kdl` to replace `tab-bar` or
`zellij:tab-bar` references with the installed plugin's full URL.
After granting the plugin read-state, tab-navigation, and run-command permissions, use
`./scripts/install-tab-bar --session SESSION` to replace existing native tab bars
without restarting terminal processes. Alt+A continues to clear completed work.

The renderer preserves Zellij 0.44.3's native tab-bar layout, scrolling, and mouse
navigation. Its source in `src/tab_bar` is adapted from Zellij's
MIT-licensed `default-plugins/tab-bar` (license included in that directory).
It reacts to tab updates; it neither monitors Codex processes nor opens the
legacy dashboard.

Bind Alt+A to acknowledge completed work through the custom tab bar:

```kdl
keybinds {
    shared_except "locked" {
        bind "Alt a" {
            MessagePlugin {
                name "zellij_codex_clear_done"
            }
        }
    }
}
```

The tab bar runs the installed helper without opening a pane or changing focus.
Grant its RunCommands permission when prompted. Only the selected pane’s completed badge is cleared; the tab badge is
recomputed from all remaining pane statuses. From a shell, `zellij-codex-badges --clear-done` performs
the same action without requiring the custom tab bar.

Manual report from a Zellij pane:

```sh
./scripts/report-status running
./scripts/report-status input
./scripts/report-status done
./scripts/report-status idle
```

Use `--session NAME --pane-id ID` to target another pane. The existing WASM
dashboard remains available explicitly through `--plugin /path/to/plugin.wasm`.

## Legacy dashboard requirements

- Zellij 0.44.3 or a compatible release
- Rust 1.81 or newer
- Codex CLI 0.147.0 or a hook-compatible release
- Python 3 for the lifecycle reporter

## Optional legacy dashboard build

Clone the repository, build the WASM plugin, and run the installer:

```sh
git clone https://github.com/rogeriofonteles/zellij-codex.git
cd zellij-codex

rustup update stable
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
./scripts/install
```

The installer copies the plugin to Zellij's user configuration directory,
installs the lifecycle and launch reporters in `~/.local/bin`, installs a fish
`codex` function, and merges lifecycle handlers into
`~/.codex/hooks.json` without replacing unrelated hooks. The launch reporter
makes a pane visible immediately, before Codex creates its conversation ID;
subsequent lifecycle hooks update that pane's native badge. Automatic hooks now
target badges; the legacy dashboard can still receive explicit pipe reports.

The dashboard also discovers Codex processes from Zellij's live pane list, so
workbench-created panes appear even if they bypass the launch reporter. A
completed response is shown as `done` while it is unread and changes to `idle`
when its Codex pane is focused or visible in the active Git/Codex view. A
full-screen Neovim overlay keeps the result unread until that view is closed.
Each lifecycle report includes its stable Zellij tab identity, so visiting one
workbench does not acknowledge completed results from any other workbench.
Closing a Codex pane removes its row from the dashboard even when Codex exits
before its `SessionEnd` hook can report the closure.

Start Codex once after installation. Codex will show a **Hooks need review**
screen; choose **Trust all and continue** after reviewing the command. New
Codex conversations opened inside Zellij will then register automatically.
When the launcher runs in a linked Git worktree, it enables the reviewed hooks
for that invocation only if the repository's main worktree is explicitly
trusted. It does not edit Codex's global configuration or extend trust to
unrelated repositories.

Launch the dashboard as a floating pane from inside a Zellij session:

```sh
zellij action launch-plugin --floating -- \
  "file:${XDG_CONFIG_HOME:-$HOME/.config}/zellij/plugins/zellij-codex.wasm"
```

To access it directly with `Alt a`, add this binding inside the
`shared_except "locked"` block in `~/.config/zellij/config.kdl`:

```kdl
bind "Alt a" {
    MessagePlugin "file:/absolute/path/to/zellij/plugins/zellij-codex.wasm" {
        name "show_dashboard"
        floating false
    }
}
```

Use an absolute path in the plugin URL; KDL does not expand shell variables.
The action asks the background plugin to open a centered floating dashboard in
the active worktree tab. If the Git/Codex view is active, its hidden Neovim
overlay stays hidden. If Neovim is active, the dashboard appears over it.
Press `Esc` while the dashboard is focused to restore the previous view; press
`Alt a` to show it again.

## Report Codex sessions running over SSH

Remote Codex hooks cannot use the local Zellij pipe directly. An authenticated
loopback receiver plus an SSH reverse tunnel bridges them without exposing a
public listening port.

On the local workstation, rerun `./scripts/install`, then start the receiver:

```sh
zellij-codex-receiver --session workbench-v2-agent
```

Leave it running (a dedicated terminal or a user service is fine). It listens
only on `127.0.0.1:47832` and reads its token from
`~/.config/zellij-codex/relay-token`.

Connect to the remote machine with a reverse tunnel:

```sh
ssh -R 127.0.0.1:47832:127.0.0.1:47832 your-server
```

The first address is remote; the second is the receiver on this workstation.
Add this to the remote host's entry in local `~/.ssh/config` to make the tunnel
automatic:

```sshconfig
Host your-server
    RemoteForward 127.0.0.1:47832 127.0.0.1:47832
    ExitOnForwardFailure yes
```

Copy this repository (or just the `scripts/remote-hook` and
`scripts/install-remote` files) to the remote machine. On the workstation,
print the secret once:

```sh
cat ~/.config/zellij-codex/relay-token
```

Then, in the repository checkout on the remote machine, install the hook using
that value:

```sh
./scripts/install-remote --host your-server
```

Paste the token at its hidden prompt. Avoid passing `--token` unless necessary,
because command-line values can remain in shell history.

The token is stored with mode `0600` in
`~/.config/zellij-codex/remote.json`. Start a new remote Codex process and
approve the hooks when prompted. Its rows will appear in the local floating
panel with agent names such as `your-server:codex`. Reports are best-effort:
Codex continues normally if the tunnel or local Zellij session is unavailable.

For manual testing, send a status report using the same installed plugin URL:

```sh
./scripts/report-status idle \
  --plugin "${XDG_CONFIG_HOME:-$HOME/.config}/zellij/plugins/zellij-codex.wasm" \
  --agent codex \
  --worktree "$(basename "$PWD")" \
  --task "Waiting for work"
```

The plugin is installed per user and is not tied to the repository being
reported. To upgrade, pull the new source, rebuild, rerun `./scripts/install`,
and relaunch with `--skip-plugin-cache` once so Zellij reads the new WASM
artifact. Rerunning the installer preserves unrelated hooks and does not
duplicate its own handlers.

## Development launch

Build and launch directly from this checkout:

```sh
rustup update stable
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
zellij action launch-plugin --skip-plugin-cache --floating -- \
  -- "file:$PWD/target/wasm32-wasip1/release/zellij-codex.wasm"
./scripts/report-status running --agent implementation \
  --worktree grpc-migration --task "Migrating grpc"
```

The producer uses Zellij's native pipe transport. Passing `--plugin` addresses
this plugin specifically and launches it if it is not already running.

Find and close the dashboard pane when needed:

```sh
zellij action list-panes
zellij action close-pane --pane-id plugin_ID
```

## Legacy dashboard scope

Implemented now:

- a versioned-by-code JSON report shape;
- validation of all seven requested states;
- multiple agents keyed by Codex session ID;
- colored terminal rendering;
- automatic SessionStart, UserPromptSubmit, PermissionRequest, Stop, and
  SessionEnd lifecycle reporting through `zellij pipe`;
- discovery and removal of workbench-launched Codex panes and unread results;
- pane and tab tracking for a workbench-aware floating overlay;
- a manual producer for testing.

Expiry and heartbeat semantics remain outside the legacy dashboard's scope.

## Validation

```sh
cargo test --all-targets
cargo fmt --check
uv run --with pytest pytest -q tests
uv run scripts/check_badges.py --installed-helper \
  --tab-bar "${XDG_CONFIG_HOME:-$HOME/.config}/zellij/plugins/zellij-codex-tab-bar.wasm" \
  --cache-home "${XDG_CACHE_HOME:-$HOME/.cache}"
```

The live check uses a disposable Zellij session and verifies lifecycle updates,
emoji badges and native theme colors, mixed done/running panes, and Alt+A without
changing pane focus.
The installation tests render configs under temporary paths, including paths
with spaces, validate them with Zellij, and check that backups survive reruns.
