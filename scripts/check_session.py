# /// script
# requires-python = ">=3.11"
# dependencies = ["pexpect", "pyte", "rich"]
# ///
"""Check the installed dashboard with real Alt+A input in an isolated session."""

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
    try:
        _check_session()
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
    args = ["--session", SESSION] if create else ["attach", SESSION]
    return pexpect.spawn(
        "zellij", args, env=env, dimensions=(45, 180), encoding="utf-8", timeout=5
    )


def _screen_text(child: pexpect.spawn, seconds: float = 2) -> str:
    if child.pid not in _SCREENS:
        screen = pyte.Screen(180, 45)
        _SCREENS[child.pid] = (screen, pyte.Stream(screen))
    screen, stream = _SCREENS[child.pid]
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            stream.feed(
                re.sub(
                    r"\x1b\[\?[0-9;]*n", "", child.read_nonblocking(65536, timeout=0.1)
                )
            )
        except pexpect.TIMEOUT:
            pass
    return "\n".join(screen.display)


def _check_session() -> None:
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
    child.send("\x1b")
    _screen_text(child)
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
        "PASS: Alt+A across tabs with ID gaps, reattachment, and recovery without lifecycle reports.",
        f"Opening latency with 8 dashboards: {max(latencies):.3f}s max; {latencies[-1]:.3f}s reopening.",
    )


def _time_dashboard(child: pexpect.spawn, tab_id: int) -> float:
    started = time.monotonic()
    child.send("\x1ba")
    while time.monotonic() - started < 1.5:
        _screen_text(child, 0.025)
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
        if visible:
            assert len(visible) == 1, visible
            elapsed = time.monotonic() - started
            assert elapsed < 1.5, f"Dashboard opening took {elapsed:.3f}s"
            return elapsed
    raise AssertionError(f"Alt+A did not open tab {tab_id}'s dashboard within 1.5s")


if __name__ == "__main__":
    main()
