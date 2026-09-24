# Minimal architecture

## Default native badges

`codex-hook` and `codex-launch` now invoke the short-lived `codex-badges`
reporter instead of launching the WASM dashboard. It locks a session-scoped
cache, reads live pane locations, and renames only titles whose badge changed.
The session socket identity prevents stale state from a previous session with
the same name. Concurrent lifecycle reports share the lock; pane moves and
closures are reconciled on the next report or explicit `--refresh`.

The reporter prefixes native tab and pane names with status markers. The
`zellij-codex-tab-bar` renderer preserves the emoji markers and Zellij’s native
theme styling. Both tab and pane-header emoji colors depend on the terminal’s
color emoji font support. No pane background color, resident monitor, or
pane-content subscription is needed. This trades continuous title tracking and focus-based acknowledgment
for a process that exits after every update. Done persists until another
lifecycle event or Alt+A acknowledgment; terminal OSC titles are overridden while a pane has a custom
name. The legacy dashboard below remains optional and is not launched by hooks.

Alt+A broadcasts `zellij_codex_clear_done` to existing plugins. The tab bar in
the active tab invokes the reporter through `run_command`, passing the session
and selected terminal pane ID, captured before launching the helper. This requires
RunCommands permission and creates no terminal pane. Only that pane’s done state
is cleared; the tab is recomputed from all panes. Running and input states are preserved.
Tab priority is input, error, stuck, done, running, paused, idle. New prompts
replace that pane's done state with running; acknowledging the last done pane
reveals any remaining running state.

Portable configuration templates live in `config/`. `install-zellij-config`
renders machine-specific paths, writes the default layout and explicit new-tab
binding, and optionally installs the workbench layouts. All layouts use the
custom tab-bar file URL directly; they do not rely on aliases being reloaded
inside an existing Zellij session. See the README for installation and backups.

## Optional status panel

The panel is the separate `zellij-codex.wasm` binary. Opening it starts a
background instance with `role "monitor"`, which owns dashboard state and
subscribes to pane content updates. Hiding the visible panel does not stop this
monitor; the `stop_dashboard` pipe message stops both. It does not consume the
badge cache, and local lifecycle hooks do not send it reports. Explicit reports
and the optional SSH receiver use the pipeline below. Activation and shutdown
commands are in the README.

```text
scripts/report-status --plugin /path/to/zellij-codex.wasm
  -> zellij pipe (name: codex_status, JSON payload)
  -> Zellij WASM plugin pipe() callback
  -> validate + replace one in-memory AgentReport
  -> ANSI-colored row

Zellij PaneUpdate
  -> discover live Codex terminal panes
  -> reconcile discovered rows with lifecycle reports by pane ID
  -> remove pane-backed rows whose terminal pane no longer exists
  -> map each result to its stable Zellij tab identity
  -> clear only results in the active, visibly tiled Git/Codex view
```

Zellij's pipe transport is session-local and can target a particular plugin
URL. The local panel monitor runs inside Zellij; only remote reporting requires
a separate receiver process. Live pane discovery covers processes started by layouts and
workbench launchers, while pane IDs de-duplicate both sources within the
session. Later, the report envelope should gain a schema version, Zellij
session, stable agent ID, timestamp, and sequence number.

## Codex 0.147.0 findings

The installed CLI is `codex-cli 0.147.0`; its `hooks` feature is stable. Binary
metadata exposes these hook event types:

- `SessionStart`, `SessionEnd`
- `PreToolUse`, `PostToolUse`
- `PreToolUsePermissionRequest`
- `UserPromptSubmit`
- `Stop`
- `PreCompact`, `PostCompact`
- `SubagentStart`, `SubagentStop`

This is enough to automate some transitions, but it does not by itself settle
the state model. In particular, `running` vs `idle`, terminal exit/crash,
`stuck`, and an approval prompt's resolution need explicit transition and
timeout rules. Hook configuration and payload schemas must be captured from the
installed CLI before enabling automatic reports; the official OpenAI docs search
did not expose a page documenting these current lifecycle hooks.

## Zellij 0.44.3 findings

- `zellij pipe --plugin file:... --name ... PAYLOAD` routes producer data to a
  plugin and can launch it when absent.
- The Rust plugin trait has a dedicated `pipe(PipeMessage)` callback.
- `get_pane_info`, `get_tab_info`, `get_pane_cwd`, and
  `get_pane_running_command` provide discovery primitives.
- `focus_pane_with_id` / `focus_terminal_pane` and tab-switching commands cover
  later dashboard navigation.
- Pane and tab update subscriptions can keep a future registry synchronized.
- Native badges prefix the existing tab name using stable tab IDs. A future
  custom tab bar could keep status metadata entirely separate from names.

## Main uncertainties to resolve next

1. Whether hook subprocesses inherit enough Zellij context to identify the
   session and source pane reliably. (`ZELLIJ`, `ZELLIJ_SESSION_NAME`, and
   `ZELLIJ_PANE_ID` should be verified empirically.)
2. Exact Codex 0.147.0 hook payloads, ordering, timeout behavior, and which hooks
   fire on cancellation, approval, errors, and abrupt process exit.
3. Whether a pipe targeted by plugin URL reaches one canonical dashboard
   instance or can create per-tab instances. The final addressing strategy must
   guarantee one state owner per Zellij session.
4. How to represent `stuck`: it is probably derived from a heartbeat/timeout,
   not emitted directly by Codex.
5. How tab status should compose with the user's existing tab bar. Zellij does
   not offer an API for appending arbitrary metadata to the built-in tab label.
