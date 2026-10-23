import hashlib
import io
import json
import os
import runpy
import sys
from pathlib import Path
from typing import Any

import pytest
from pytest_mock import MockerFixture

SCRIPTS = Path(__file__).parents[1] / "scripts"
SOURCE = "01a12c70-8ab5-7691-9d74-7cb7391fbdf8"
BRANCH = "01a12c54-b3a6-7bf3-84b8-a80b8a912345"
SOCKET = "test-socket"


@pytest.fixture
def reporter(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mocker: MockerFixture
) -> dict[str, Any]:
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path))
    monkeypatch.setattr(sys, "argv", ["codex-badges", "--session", "work"])
    script = runpy.run_path(str(SCRIPTS / "codex-badges"))
    identity = hashlib.sha256(SOCKET.encode()).hexdigest()
    mocker.patch.dict(
        script["main"].__globals__, {"_session_identity": lambda _: identity}
    )
    mocker.patch.object(
        script["subprocess"],
        "check_output",
        return_value=json.dumps(
            [
                {
                    "id": 7,
                    "is_plugin": False,
                    "exited": False,
                    "title": "Branch",
                    "tab_id": 2,
                    "tab_name": "work",
                }
            ]
        ),
    )
    mocker.patch.object(script["subprocess"], "run")
    return script


@pytest.mark.parametrize("event", ["SessionEnd", "Stop", "UserPromptSubmit"])
def test_branch_rejects_delayed_source_hooks(
    reporter: dict[str, Any], monkeypatch: pytest.MonkeyPatch, event: str
) -> None:
    binding = reporter["binding"]
    path = _register(reporter, SOURCE)
    _report(
        reporter, monkeypatch, {"pane_id": 7, "thread_id": SOURCE, "status": "done"}
    )
    binding.record_event(BRANCH, "UserPromptSubmit")
    _register(reporter, BRANCH)
    _report(
        reporter,
        monkeypatch,
        {
            "pane_id": 7,
            "binding_prefix": BRANCH[:29],
            "binding_path": str(path),
            "base_title": "Branch",
        },
    )
    state = _state(reporter)
    assert state["statuses"]["7"] == "running"
    reporter["subprocess"].run.reset_mock()
    _report(
        reporter,
        monkeypatch,
        {
            "pane_id": 7,
            "thread_id": SOURCE,
            "event": event,
            "status": "done" if event != "UserPromptSubmit" else "running",
            "remove": event == "SessionEnd",
        },
    )
    assert _state(reporter) == state
    reporter["subprocess"].run.assert_not_called()
    _report(
        reporter,
        monkeypatch,
        {
            "pane_id": 7,
            "thread_id": BRANCH,
            "event": "Stop",
            "status": "done",
        },
    )
    assert _state(reporter)["statuses"]["7"] == "done"


def test_pending_branch_events_apply_after_title_switch(
    reporter: dict[str, Any], monkeypatch: pytest.MonkeyPatch
) -> None:
    binding = reporter["binding"]
    path = _register(reporter, SOURCE)
    _report(
        reporter, monkeypatch, {"pane_id": 7, "thread_id": SOURCE, "status": "done"}
    )
    state = _state(reporter)
    binding.record_event(BRANCH, "Stop")
    _report(
        reporter,
        monkeypatch,
        {
            "pane_id": 7,
            "thread_id": BRANCH,
            "event": "Stop",
            "status": "done",
        },
    )
    assert _state(reporter) == state
    _register(reporter, BRANCH)
    _report(
        reporter,
        monkeypatch,
        {
            "pane_id": 7,
            "binding_prefix": BRANCH[:29],
            "binding_path": str(path),
            "base_title": "Branch",
        },
    )
    assert _state(reporter)["threads"]["7"] == BRANCH[:29]
    assert _state(reporter)["statuses"]["7"] == "done"


@pytest.mark.parametrize("attached", [True, False])
def test_session_end_keeps_badge_until_terminal_bridge_exits(
    reporter: dict[str, Any], monkeypatch: pytest.MonkeyPatch, attached: bool
) -> None:
    if attached:
        _register(reporter, SOURCE)
    _report(
        reporter, monkeypatch, {"pane_id": 7, "thread_id": SOURCE, "status": "done"}
    )
    _report(
        reporter,
        monkeypatch,
        {
            "pane_id": 7,
            "thread_id": SOURCE,
            "event": "SessionEnd",
            "remove": True,
        },
    )
    assert _state(reporter)["statuses"] == ({"7": "idle"} if attached else {})
    _report(reporter, monkeypatch, {"pane_id": 7, "remove": True})
    assert _state(reporter)["statuses"] == {}


def _register(reporter: dict[str, Any], thread: str) -> Path:
    binding = reporter["binding"]
    path = binding.root() / "bindings/client.json"
    binding.atomic_json(
        path,
        {
            "prefix": thread[:29],
            "session": "work",
            "pane": "7",
            "socket": SOCKET,
            "pid": os.getpid(),
            "owner": binding.owner_token(os.getpid()),
        },
    )
    return path


def _report(
    reporter: dict[str, Any], monkeypatch: pytest.MonkeyPatch, report: dict[str, Any]
) -> None:
    monkeypatch.setattr(sys, "stdin", io.StringIO(json.dumps(report)))
    reporter["main"]()


def _state(reporter: dict[str, Any]) -> dict[str, Any]:
    identity = hashlib.sha256(SOCKET.encode()).hexdigest()
    return json.loads(
        (reporter["binding"].root() / f"badges/{identity}.json").read_text()
    )
