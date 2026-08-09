# Minimal architecture

## First vertical slice

```text
scripts/report-status
  -> zellij pipe (name: codex_status, JSON payload)
  -> Zellij WASM plugin pipe() callback
  -> validate + replace one in-memory AgentReport
  -> ANSI-colored row
```

Zellij's pipe transport is the smallest native boundary: it is session-local,
does not require a daemon or filesystem polling, and can target a particular
plugin URL. Later, the report envelope should gain a schema version, Zellij
session, stable agent ID, pane ID, tab ID, timestamp, and sequence number.

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
- Plugins can rename tabs, but directly owning the user's tab name is likely the
  wrong tab-status integration. A dedicated custom tab-bar plugin (or status-bar
  presentation) should consume the aggregate state without destroying worktree
  names.

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
