# /// script
# requires-python = ">=3.11"
# dependencies = ["pexpect", "pyte", "rich"]
# ///
"""Check the installed dashboard with real Alt+A input in an isolated session."""

import argparse
import json
import os
import re
import subprocess
import time
import uuid
from pathlib import Path

import pexpect
import pyte
import rich

SESSION = f"codex_check_{uuid.uuid4().hex[:10]}"
SOCKET_DIR = os.environ.get("ZELLIJ_SOCKET_DIR", f"/tmp/zellij-{os.getuid()}")
PLUGIN = f"file:{Path(os.environ.get('XDG_CONFIG_HOME', Path.home() / '.config')) / 'zellij/plugins/zellij-codex.wasm'}"


_SCREENS: dict[int, tuple[pyte.Screen, pyte.Stream]] = {}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--refresh-only",
        action="store_true",
        help="Check refresh and stale entry removal only.",
    )
    parser.add_argument(
        "--latency-only",
        action="store_true",
        help="Check opening latency under tab load.",
    )
    args = parser.parse_args()
    try:
        if args.latency_only:
            _check_latency()
        else:
            _check_session(refresh_only=args.refresh_only)
    finally:
        subprocess.run(
            ["zellij", "delete-session", "--force", SESSION],
            check=False,
            capture_output=True,
            timeout=10,
            env={**os.environ, "ZELLIJ_SOCKET_DIR": SOCKET_DIR},
        )


def _cli(*args: str) -> str:
    return subprocess.check_output(
        ["zellij", "--session", SESSION, *args],
        text=True,
        timeout=8,
        env={**os.environ, "ZELLIJ_SOCKET_DIR": SOCKET_DIR},
    )


def _attach(create: bool = False) -> pexpect.spawn:
    env = {k: v for k, v in os.environ.items() if not k.startswith("ZELLIJ")}
    env["TERM"] = "xterm-256color"
    env["ZELLIJ_SOCKET_DIR"] = SOCKET_DIR
    if config := os.environ.get("ZELLIJ_CONFIG_FILE"):
        env["ZELLIJ_CONFIG_FILE"] = config
    args = ["--session", SESSION] if create else ["attach", SESSION]
    child = pexpect.spawn(
        "zellij", args, env=env, dimensions=(45, 180), encoding="utf-8", timeout=5
    )
    child.delaybeforesend = None
    return child


def _screen_text(
    child: pexpect.spawn, seconds: float = 2, *, until_quiet: bool = False
) -> str:
    if child.pid not in _SCREENS:
        screen = pyte.Screen(180, 45)
        _SCREENS[child.pid] = (screen, pyte.Stream(screen))
    screen, stream = _SCREENS[child.pid]
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            stream.feed(
                re.sub(
                    r"\x1b\[\?[0-9;]*n",
                    "",
                    child.read_nonblocking(
                        65536, timeout=0.05 if until_quiet else 0.01
                    ),
                )
            )
        except pexpect.TIMEOUT:
            if until_quiet:
                break
    return "\n".join(screen.display)


def _check_session(refresh_only: bool = False) -> None:
    child = _attach(True)
    _screen_text(child)
    child.send("\x1b")
    _cli(
        "action",
        "launch-plugin",
        "--floating",
        "--skip-plugin-cache",
        PLUGIN,
    )
    initial = _screen_text(child)
    if "Allow?" in initial:
        child.send("y")
        _screen_text(child)
    child.send("\x1b")
    _screen_text(child)
    _cli("action", "new-tab", "--name", "discard")
    _cli("action", "new-tab", "--name", "target")
    _cli("action", "close-tab-by-id", "1")
    tab = json.loads(_cli("action", "current-tab-info", "--json"))
    assert tab["tab_id"] == 2 and tab["position"] == 1, tab
    _cli(
        "action",
        "new-pane",
        "--",
        "bash",
        "-c",
        "printf '• Waiting for background terminal (6m • esc to interrupt)\\n› Ask Codex to do anything\\ngpt-6-astra low · ~/code/recovery\\n'; exec -a codex sleep 600",
    ).strip()
    _screen_text(child, 1)
    child.send("\x1ba")
    initial = _screen_text(child)
    if "Allow?" in initial:
        child.send("y")
    before = initial + _screen_text(child)
    assert "running" in before, before
    _check_refresh(child)
    _check_reopen_refresh(child)
    if refresh_only:
        rich.get_console().print(
            "PASS: r removes stale agents, preserves live agents, and keeps entries cleared after reopening."
        )
        return
    child.send("\x1b")
    _screen_text(child)
    _check_manual_floating_panes(child)
    _cli("action", "new-tab", "--name", "decoy")
    _cli("action", "go-to-tab-by-id", "2")
    _screen_text(child, 1)
    child.send("\x1ba")
    _screen_text(child)
    assert json.loads(_cli("action", "current-tab-info", "--json"))["tab_id"] == 2
    _cli("action", "detach")
    child.expect(pexpect.EOF)
    child = _attach()
    after = _screen_text(child)
    child.send("\x1b")
    _screen_text(child)
    _screen_text(child, 1)
    child.send("\x1ba")
    after += _screen_text(child)
    assert "running" in after, after
    tab_after = json.loads(_cli("action", "current-tab-info", "--json"))
    assert tab_after["tab_id"] == 2, tab_after
    dashboard = next(
        p
        for p in json.loads(_cli("action", "list-panes", "--all", "--json"))
        if p.get("plugin_url") == PLUGIN and p["tab_id"] == 2
    )
    assert dashboard["tab_id"] == 2 and dashboard["is_floating"], dashboard
    child.send("\x1b")
    _screen_text(child)
    _cli("action", "go-to-tab-by-id", "3")
    _screen_text(child, 1)
    child.send("\x1ba")
    _screen_text(child)
    assert json.loads(_cli("action", "current-tab-info", "--json"))["tab_id"] == 3
    dashboards = [
        p
        for p in json.loads(_cli("action", "list-panes", "--all", "--json"))
        if p.get("plugin_url") == PLUGIN
    ]
    assert {p["tab_id"] for p in dashboards} == {0, 2, 3}, dashboards
    assert any(p["tab_id"] == 3 and p["is_floating"] for p in dashboards)
    _check_shared_status(child, len(dashboards))
    latencies: list[float] = []
    for index in range(5):
        child.send("\x1b")
        _screen_text(child, 0.2)
        _cli("action", "new-tab", "--name", f"load_{index}")
        for _ in range(3):
            _cli("action", "new-pane", "--", "sleep", "600")
        _screen_text(child, 0.2)
        tab_id = json.loads(_cli("action", "current-tab-info", "--json"))["tab_id"]
        latencies.append(_time_dashboard(child, tab_id))
    child.send("\x1b")
    _screen_text(child, 0.2)
    _cli("action", "go-to-tab-by-id", "2")
    _screen_text(child, 0.2)
    latencies.append(_time_dashboard(child, 2))
    rich.get_console().print(
        "PASS: Alt+A preserves manual floating panes, tab ID gaps, reattachment, and agent recovery.",
        f"Opening latency with 8 dashboards: {max(latencies):.3f}s max; {latencies[-1]:.3f}s reopening.",
    )


def _check_latency() -> None:
    child = _attach(True)
    _screen_text(child)
    child.send("\x1b")
    _cli("action", "launch-plugin", "--floating", "--skip-plugin-cache", PLUGIN)
    if "Allow?" in _screen_text(child):
        child.send("y")
        _screen_text(child)
    states = [
        json.loads(line)
        for line in _cli(
            "pipe",
            "--name",
            "list_agents",
            "--args",
            "include_metadata=true",
            "--",
            "{}",
        ).splitlines()
        if line.startswith("{")
    ]
    monitors = [state for state in states if state["is_monitor"]]
    assert len(monitors) == 1, states
    assert all(
        state["status_source"] == monitors[0]["plugin_id"] for state in states
    ), states
    latencies: list[float] = []
    for index in range(8):
        child.send("\x1b")
        _screen_text(child, 0.2)
        _cli("action", "new-tab", "--name", f"load_{index}")
        for _ in range(3):
            _cli("action", "new-pane", "--", "sleep", "600")
        _screen_text(child, 0.2)
        tab_id = json.loads(_cli("action", "current-tab-info", "--json"))["tab_id"]
        latency = _time_dashboard(child, tab_id)
        latencies.append(latency)
        rich.get_console().print(f"Tab {tab_id}: {latency:.3f}s")
    reopens: list[float] = []
    for tab_id in range(1, 9):
        child.send("\x1b")
        _screen_text(child, 0.2)
        _cli("action", "go-to-tab-by-id", str(tab_id))
        _screen_text(child, 0.2)
        reopens.append(_time_dashboard(child, tab_id))
    rich.get_console().print(
        f"PASS: first opening {max(latencies):.3f}s max; reopening {max(reopens):.3f}s max"
    )


def _check_refresh(child: pexpect.spawn) -> None:
    _cli(
        "pipe",
        "--name",
        "codex_status",
        "--",
        json.dumps(
            {
                "id": "closed-agent",
                "agent": "closed-agent",
                "worktree": "closed-tab",
                "status": "idle",
                "pane_id": 9999999,
                "tab_id": 9999999,
            }
        ),
    )
    # PaneUpdate may prune the injected row before input arrives. A report
    # error persists until refresh, so it also verifies that r was handled.
    _cli("pipe", "--name", "codex_status", "--", "invalid-json")
    before = _screen_text(child)
    assert "invalid status report" in before, before
    child.send("r")
    refreshed = _screen_text(child)
    assert (
        "closed-agent" not in refreshed and "invalid status report" not in refreshed
    ), refreshed
    assert "running" in refreshed and "r: refresh agents" in refreshed, refreshed
    child.send("\x1b")
    _screen_text(child, 0.5)
    child.send("\x1ba")
    reopened = _screen_text(child)
    assert "closed-agent" not in reopened and "running" in reopened, reopened


def _check_manual_floating_panes(child: pexpect.spawn) -> None:
    pane_ids = {
        int(
            _cli(
                "action",
                "new-pane",
                "--floating",
                "--name",
                title,
                "--",
                "sleep",
                "600",
            )
            .strip()
            .removeprefix("terminal_")
        )
        for title in ("[host] project / editor", "manual floating shell")
    }
    _screen_text(child, 0.5)
    geometry_keys = ("pane_x", "pane_y", "pane_columns", "pane_rows")
    originals = {
        pane["id"]: tuple(pane[key] for key in geometry_keys)
        for pane in json.loads(_cli("action", "list-panes", "--all", "--json"))
        if not pane["is_plugin"] and pane["id"] in pane_ids
    }
    _cli("action", "hide-floating-panes")
    for _ in range(2):
        _screen_text(child, 0.5)
        child.send("\x1ba")
        screen = _screen_text(child)
        assert "TAB" in screen and "target" in screen, screen
        panes = json.loads(_cli("action", "list-panes", "--all", "--json"))
        hidden = [p for p in panes if not p["is_plugin"] and p["id"] in pane_ids]
        assert len(hidden) == 2 and all(p["is_suppressed"] for p in hidden), hidden
        child.send("\x1b")
        _screen_text(child, 0.5)
        panes = json.loads(_cli("action", "list-panes", "--all", "--json"))
        restored = [p for p in panes if not p["is_plugin"] and p["id"] in pane_ids]
        assert all(p["is_floating"] and not p["is_suppressed"] for p in restored), (
            restored
        )
        assert all(
            tuple(p[key] for key in geometry_keys) == originals[p["id"]]
            for p in restored
        )
        assert not json.loads(_cli("action", "current-tab-info", "--json"))[
            "are_floating_panes_visible"
        ]
    for pane_id in pane_ids:
        _cli("action", "close-pane", "--pane-id", f"terminal_{pane_id}")


def _check_reopen_refresh(child: pexpect.spawn) -> None:
    child.send("\x1b")
    _screen_text(child, 0.2)
    report = {
        "id": "cached-view",
        "agent": "cached-view",
        "status": "running",
        "task": "cached!",
    }
    _cli("pipe", "--name", "codex_status", "--", json.dumps(report))
    _screen_text(child, 0.2)
    child.send("\x1ba")
    cached = _screen_text(child)
    assert "cached!" in cached, cached
    _cli("pipe", "--name", "codex_status", "--", json.dumps({**report, "remove": True}))
    _screen_text(child, 0.2)


def _check_shared_status(child: pexpect.spawn, dashboard_count: int) -> None:
    panes = json.loads(_cli("action", "list-panes", "--all", "--json"))
    pane = next(p for p in panes if p["tab_id"] == 0 and not p["is_plugin"])
    report = {
        "id": "shared-status",
        "agent": "shared-status",
        "status": "running",
        "pane_id": pane["id"],
        "tab_id": 0,
        "tab_position": 0,
    }
    for status in ("running", "done"):
        report["status"] = status
        _cli(
            "pipe",
            "--plugin",
            PLUGIN,
            "--name",
            "codex_status",
            "--",
            json.dumps(report),
        )
        _assert_shared_status(child, dashboard_count, status)
    child.send("\x1b")
    _screen_text(child, 0.2)
    _cli("action", "go-to-tab-by-id", "0")
    _screen_text(child, 0.2)
    child.send("\x1ba")
    _assert_shared_status(child, dashboard_count, "idle")
    _cli(
        "pipe",
        "--plugin",
        PLUGIN,
        "--name",
        "codex_status",
        "--",
        json.dumps({**report, "remove": True}),
    )
    child.send("\x1b")
    _screen_text(child, 0.2)
    _cli("action", "go-to-tab-by-id", "3")
    _screen_text(child, 0.2)
    child.send("\x1ba")
    _screen_text(child, 0.2)


def _assert_shared_status(
    child: pexpect.spawn, dashboard_count: int, status: str
) -> None:
    deadline = time.monotonic() + 3
    snapshots: list[dict[str, dict[str, object]]] = []
    while time.monotonic() < deadline:
        _screen_text(child, 0.1)
        states = [
            json.loads(line)
            for line in _cli(
                "pipe",
                "--name",
                "list_agents",
                "--args",
                "include_metadata=true",
                "--",
                "{}",
            ).splitlines()
            if line.startswith("{")
        ]
        monitors = [state for state in states if state["is_monitor"]]
        assert len(monitors) == 1, states
        assert all(
            state["status_source"] == monitors[0]["plugin_id"] for state in states
        ), states
        snapshots = [state["agents"] for state in states]
        if len(snapshots) >= dashboard_count and all(
            snapshot == snapshots[0]
            and snapshot.get("shared-status", {}).get("status") == status
            for snapshot in snapshots
        ):
            return
    raise AssertionError(f"Dashboards disagree about {status}: {snapshots}")


def _time_dashboard(child: pexpect.spawn, tab_id: int) -> float:
    _screen_text(child, 5, until_quiet=True)
    started = time.monotonic()
    child.send("\x1ba")
    while time.monotonic() - started < 1.5:
        screen = _screen_text(child, 0.025)
        if "Codex Dashboard" in screen and "r: refresh agents" in screen:
            elapsed = time.monotonic() - started
            assert elapsed < 1.5, f"Dashboard opening took {elapsed:.3f}s"
            panes = json.loads(_cli("action", "list-panes", "--all", "--json"))
            visible = [
                pane
                for pane in panes
                if pane.get("plugin_url") == PLUGIN
                and pane["tab_id"] == tab_id
                and pane["is_floating"]
                and not pane["is_suppressed"]
                and pane["is_focused"]
            ]
            assert len(visible) == 1, visible
            return elapsed
    raise AssertionError(
        f"Alt+A did not open tab {tab_id}'s dashboard within 1.5s\n{screen}"
    )


if __name__ == "__main__":
    main()
