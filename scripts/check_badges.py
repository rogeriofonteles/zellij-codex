# /// script
# requires-python = ">=3.11"
# dependencies = ["pexpect", "pyte", "rich"]
# ///
"""Exercise native badges in a disposable Zellij session without a monitor."""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

import pexpect
import pyte
import rich


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--tab-bar",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "target/wasm32-wasip1/release/zellij-codex-tab-bar.wasm",
        help="Verify emoji rendering, theme colors, and live tab-bar replacement.",
    )
    parser.add_argument(
        "--installed-helper",
        action="store_true",
        help="Exercise the default installed helper instead of a delayed test helper.",
    )
    parser.add_argument(
        "--cache-home",
        type=Path,
        help="Use this cache to verify installed plugin permissions.",
    )
    args = parser.parse_args()
    session = f"badge_check_{uuid.uuid4().hex[:10]}"
    root = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="zellij_badge_check_") as directory:
        config = Path(directory) / "config.kdl"
        layout = Path(directory) / "layout.kdl"
        layout.write_text(
            'layout {\npane size=1 borderless=true { plugin location="zellij:tab-bar"; }\n'
            'pane\npane size=1 borderless=true { plugin location="zellij:status-bar"; }\n}\n'
        )
        helper = Path(directory) / "clear-badges"
        helper.write_text(
            f"#!{sys.executable}\nimport os, sys, time\ntime.sleep(0.5)\n"
            f"os.execv(sys.executable, [sys.executable, {str(root / 'scripts/codex-badges')!r}, *sys.argv[1:]])\n"
        )
        helper.chmod(0o755)
        config.write_text(
            'default_shell "/bin/bash"\nshow_startup_tips false\nshow_release_notes false\n'
            'keybinds {\nshared_except "locked" {\nbind "Alt a" {\n'
            'MessagePlugin {\nname "zellij_codex_clear_done"\n}\n}\n}\n}\n'
        )
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("ZELLIJ")
        }
        env.update(
            TERM="xterm-256color", XDG_CACHE_HOME=str(args.cache_home or directory)
        )
        child = pexpect.spawn(
            "zellij",
            [
                "--config",
                str(config),
                "--new-session-with-layout",
                str(layout),
                "--session",
                session,
            ],
            env=env,
            cwd=str(root),
            dimensions=(32, 160),
            encoding="utf-8",
        )
        screen = pyte.Screen(160, 32)
        stream = pyte.Stream(screen)
        command = ["zellij", "--session", session, "action"]
        try:
            deadline = time.monotonic() + 10
            while True:
                _drain(child, stream)
                panes = json.loads(
                    subprocess.check_output(
                        [*command, "list-panes", "--all", "--json"], env=env
                    )
                )
                if any(not p["is_plugin"] for p in panes) and any(
                    p.get("plugin_url") in ("tab-bar", "zellij:tab-bar") for p in panes
                ):
                    break
                assert time.monotonic() < deadline, panes
            title_column = screen.display[0].index("Tab #1")
            native_title_style = screen.buffer[0][title_column]
            pane = next(p for p in panes if not p["is_plugin"])
            pane_id, tab_id = pane["id"], pane["tab_id"]
            if args.tab_bar:
                bar = next(
                    p
                    for p in panes
                    if p.get("plugin_url") in ("tab-bar", "zellij:tab-bar")
                )
                subprocess.run(
                    [
                        *command,
                        "launch-plugin",
                        "--skip-plugin-cache",
                        "--tab-id",
                        str(tab_id),
                        "--configuration",
                        f"replace_pane_id={bar['id']}"
                        + ("" if args.installed_helper else f",badge_command={helper}"),
                        f"file:{args.tab_bar.resolve()}",
                    ],
                    check=True,
                    capture_output=True,
                    env=env,
                )
                _drain(child, stream)
                if "Allow" in "\n".join(screen.display):
                    child.send("y")
                    _drain(child, stream)
                replaced = json.loads(
                    subprocess.check_output(
                        [*command, "list-panes", "--all", "--json"], env=env
                    )
                )
                custom = next(
                    p
                    for p in replaced
                    if p.get("plugin_url") == f"file:{args.tab_bar.resolve()}"
                )
                assert custom["pane_rows"] == 1 and custom["pane_y"] == 0, custom
                assert not any(
                    p["is_plugin"] and p["id"] == bar["id"] for p in replaced
                ), replaced
            subprocess.run(
                [*command, "rename-pane", "--pane-id", str(pane_id), "Badge pane"],
                check=True,
                env=env,
            )
            subprocess.run(
                [*command, "rename-tab-by-id", str(tab_id), "Badge tab"],
                check=True,
                env=env,
            )
            hook_env = {
                **env,
                "ZELLIJ_SESSION_NAME": session,
                "ZELLIJ_PANE_ID": str(pane_id),
            }
            for event, prefix in [
                ("UserPromptSubmit", "[🟠 ↻] "),
                ("PermissionRequest", "[🔴 !] "),
                ("PostToolUse", "[🟠 ↻] "),
                ("Stop", "[🔵 ✓] "),
                ("SessionStart", ""),
            ]:
                hook = subprocess.Popen(
                    [sys.executable, str(root / "scripts/codex-hook")],
                    stdin=subprocess.PIPE,
                    text=True,
                    env=hook_env,
                )
                assert hook.stdin is not None
                hook.stdin.write(
                    json.dumps(
                        {
                            "hook_event_name": event,
                            "session_id": "test",
                            "cwd": str(root),
                        }
                    )
                )
                hook.stdin.close()
                deadline = time.monotonic() + 10
                while hook.poll() is None and time.monotonic() < deadline:
                    _drain(child, stream)
                assert hook.wait(timeout=1) == 0
                _drain(child, stream)
                panes = json.loads(
                    subprocess.check_output(
                        [*command, "list-panes", "--all", "--json"], env=env
                    )
                )
                actual = next(
                    p for p in panes if not p["is_plugin"] and p["id"] == pane_id
                )
                assert actual["title"] == prefix + "Badge pane", actual
                assert actual["tab_name"] == prefix + "Badge tab", actual
                assert not any(
                    (p.get("plugin_url") or "").endswith("/zellij-codex.wasm")
                    for p in panes
                )
                rendered = "\n".join(screen.display)
                assert prefix + "Badge pane" in rendered, rendered
                row = screen.display[0]
                title = prefix + "Badge tab"
                assert title in row, row
                column = row.index(title)
                assert screen.buffer[0][column].bg == native_title_style.bg, row
                assert screen.buffer[0][column].fg == native_title_style.fg, row
                rich.get_console().print(
                    f"PASS {event}: tab and pane {prefix or '(idle)'}"
                )
                child.send("\x1ba")
                deadline = time.monotonic() + 1
                while time.monotonic() < deadline:
                    during = json.loads(
                        subprocess.check_output(
                            [*command, "list-panes", "--all", "--json"],
                            env=env,
                        )
                    )
                    assert {(p["is_plugin"], p["id"]) for p in during} == {
                        (p["is_plugin"], p["id"]) for p in panes
                    }, during
                    _drain(child, stream, duration=0.05)
                _drain(child, stream)
                after = json.loads(
                    subprocess.check_output(
                        [*command, "list-panes", "--all", "--json"],
                        env=env,
                    )
                )
                expected_prefix = "" if event == "Stop" else prefix
                actual = next(
                    p for p in after if not p["is_plugin"] and p["id"] == pane_id
                )
                assert actual["title"] == expected_prefix + "Badge pane", actual
                assert actual["tab_name"] == expected_prefix + "Badge tab", actual
                assert actual["is_focused"] and not actual["is_suppressed"], actual
                assert len(after) == len(panes), after
                rich.get_console().print(
                    f"PASS Alt+A after {event}: correct status, no pane created, focus preserved"
                )
            second = subprocess.check_output(
                [*command, "new-pane", "--name", "Second pane"],
                env=env,
                text=True,
            ).strip()
            second_id = int(second.removeprefix("terminal_"))
            _drain(child, stream)
            for target in (pane_id, second_id):
                reporter = subprocess.Popen(
                    [
                        sys.executable,
                        str(root / "scripts/codex-badges"),
                        "--session",
                        session,
                    ],
                    stdin=subprocess.PIPE,
                    text=True,
                    env=env,
                )
                assert reporter.stdin is not None
                reporter.stdin.write(json.dumps({"pane_id": target, "status": "done"}))
                reporter.stdin.close()
                deadline = time.monotonic() + 10
                while reporter.poll() is None and time.monotonic() < deadline:
                    _drain(child, stream)
                assert reporter.wait(timeout=1) == 0
            for target, remaining in ((second_id, {pane_id}), (pane_id, set())):
                if target == pane_id:
                    subprocess.run(
                        [*command, "focus-pane-id", f"terminal_{target}"],
                        check=True,
                        env=env,
                    )
                _drain(child, stream)
                child.send("\x1ba")
                _drain(child, stream, duration=2)
                after = json.loads(
                    subprocess.check_output(
                        [*command, "list-panes", "--all", "--json"],
                        env=env,
                    )
                )
                terminals = [p for p in after if not p["is_plugin"]]
                assert len(terminals) == 2, terminals
                assert {
                    p["id"] for p in terminals if p["title"].startswith("[🔵 ✓] ")
                } == remaining, terminals
                assert next(p for p in terminals if p["id"] == target)["is_focused"]
                assert all(
                    p["tab_name"] == ("[🔵 ✓] " if remaining else "") + "Badge tab"
                    for p in terminals
                ), terminals
            rich.get_console().print(
                "PASS two done panes: selected pane clears first; tab clears last"
            )
            for target, event, expected_tab in [
                (pane_id, "Stop", "[🔵 ✓] "),
                (second_id, "UserPromptSubmit", "[🔵 ✓] "),
                (pane_id, "UserPromptSubmit", "[🟠 ↻] "),
                (pane_id, "Stop", "[🔵 ✓] "),
            ]:
                started = time.monotonic()
                hook = subprocess.Popen(
                    [sys.executable, str(root / "scripts/codex-hook")],
                    stdin=subprocess.PIPE,
                    text=True,
                    env={**hook_env, "ZELLIJ_PANE_ID": str(target)},
                )
                assert hook.stdin is not None
                hook.stdin.write(
                    json.dumps(
                        {
                            "hook_event_name": event,
                            "session_id": "test",
                            "cwd": str(root),
                        }
                    )
                )
                hook.stdin.close()
                while hook.poll() is None and time.monotonic() - started < 10:
                    _drain(child, stream, duration=0.05)
                assert hook.wait(timeout=1) == 0
                _drain(child, stream)
                after = json.loads(
                    subprocess.check_output(
                        [*command, "list-panes", "--all", "--json"], env=env
                    )
                )
                actual = next(
                    p for p in after if not p["is_plugin"] and p["id"] == target
                )
                expected_pane = "[🔵 ✓] " if event == "Stop" else "[🟠 ↻] "
                assert actual["title"].startswith(expected_pane), actual
                assert actual["tab_name"] == expected_tab + "Badge tab", actual
                rich.get_console().print(
                    f"PASS mixed panes {event}: updated in {time.monotonic() - started:.2f}s"
                )
            child.send("\x1ba")
            _drain(child, stream, duration=2)
            after = json.loads(
                subprocess.check_output(
                    [*command, "list-panes", "--all", "--json"], env=env
                )
            )
            terminals = {p["id"]: p for p in after if not p["is_plugin"]}
            assert terminals[pane_id]["title"] == "Badge pane", terminals
            assert terminals[second_id]["title"] == "[🟠 ↻] Second pane", terminals
            assert all(
                p["tab_name"] == "[🟠 ↻] Badge tab" for p in terminals.values()
            ), terminals
            rich.get_console().print(
                "PASS Alt+A clears done pane; running pane keeps tab running"
            )
        finally:
            subprocess.run(
                ["zellij", "delete-session", "--force", session],
                check=False,
                env=env,
                capture_output=True,
                timeout=10,
            )
            child.close(force=True)


def _drain(child: pexpect.spawn, stream: pyte.Stream, duration: float = 1) -> None:
    deadline = time.monotonic() + duration
    while time.monotonic() < deadline:
        try:
            stream.feed(
                re.sub(
                    r"\x1b\[\?[0-9;]*n", "", child.read_nonblocking(65536, timeout=0.05)
                )
            )
        except pexpect.TIMEOUT:
            continue


if __name__ == "__main__":
    main()
