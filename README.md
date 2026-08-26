# zellij-codex

A Zellij WASM plugin for collecting and displaying Codex agent status.

## Requirements

- Zellij 0.44.3 or a compatible release
- Rust 1.81 or newer
- Codex CLI 0.147.0 or a hook-compatible release
- Python 3 for the lifecycle reporter

## Install

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
`codex` function, and merges five lifecycle handlers into
`~/.codex/hooks.json` without replacing unrelated hooks. The launch reporter
makes a pane visible immediately, before Codex creates its conversation ID;
subsequent lifecycle hooks update that same pane row.

The dashboard also discovers Codex processes from Zellij's live pane list.
This includes Codex panes created by layouts and worktree launchers, even when
they bypass the installed shell function. Accept Zellij's
`ReadApplicationState` and `ReadCliPipes` permission prompt the first time the
updated plugin is opened. Lifecycle reports replace the discovered row in
place when available.

Start Codex once after installation. Codex will show a **Hooks need review**
screen; choose **Trust all and continue** after reviewing the command. New
Codex conversations opened inside Zellij will then register automatically.

Launch the dashboard as a floating pane from inside a Zellij session:

```sh
zellij action launch-plugin --floating -- \
  "file:${XDG_CONFIG_HOME:-$HOME/.config}/zellij/plugins/zellij-codex.wasm"
```

To access it directly with `Alt a`, add this binding inside the
`shared_except "locked"` block in `~/.config/zellij/config.kdl`:

```kdl
bind "Alt a" {
    LaunchOrFocusPlugin "file:/absolute/path/to/zellij/plugins/zellij-codex.wasm" {
        floating true
        move_to_focused_tab true
    }
}
```

The binding applies to new Zellij sessions after the configuration is loaded.
Use an absolute path in the plugin URL; KDL does not expand shell variables.
The action opens or focuses one floating dashboard and moves it to the active
worktree tab. This keeps a single plugin instance available across tabs.
Press `Esc` while the dashboard is focused to hide it; press `Alt a` to show it
again.

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

## Scope

Implemented now:

- a versioned-by-code JSON report shape;
- validation of all seven requested states;
- multiple agents keyed by Codex session ID;
- colored terminal rendering;
- automatic SessionStart, UserPromptSubmit, PermissionRequest, Stop, and
  SessionEnd lifecycle reporting through `zellij pipe`;
- discovery of Codex processes already running in Zellij terminal panes;
- a manual producer for testing.

Deliberately deferred until this transport is proven inside Zellij:

- tab discovery and aggregation;
- persistence, expiry/heartbeat semantics, and navigation;
- tab-bar integration.
