import runpy
from pathlib import Path
from typing import Any

BADGES = runpy.run_path(str(Path(__file__).parents[1] / "scripts/codex-badges"))


def test_lifecycle_preserves_names_and_does_not_duplicate_badges() -> None:
    assert BADGES["badge_title"]("[🔴 ↻] Existing", "running") == "[🟠 ↻] Existing"
    assert BADGES["badge_title"]("[🟡 !] Existing", "input") == "[🔴 !] Existing"
    state: dict[str, Any] = {}
    panes = [_pane(7, 12)]
    for status, prefix in [
        ("running", "[🟠 ↻] "),
        ("input", "[🔴 !] "),
        ("done", "[🔵 ✓] "),
        ("idle", ""),
    ]:
        state, actions = BADGES["reconcile"](
            state, panes, {"pane_id": 7, "status": status}
        )
        assert actions == [
            ["rename-pane", "--pane-id", "terminal_7", "--", prefix + "My agent"],
            ["rename-tab-by-id", "12", "--", prefix + "feature"],
        ]
        panes[0].update(title=prefix + "My agent", tab_name=prefix + "feature")
        assert (
            BADGES["reconcile"](state, panes, {"pane_id": 7, "status": status})[1] == []
        )
    panes[0].update(title="Custom title", tab_name="renamed")
    _, actions = BADGES["reconcile"](state, panes, {"pane_id": 7, "status": "done"})
    assert actions[0][-1] == "[🔵 ✓] Custom title"
    assert actions[1][-1] == "[🔵 ✓] renamed"


def test_tab_priority_and_closed_pane_cleanup() -> None:
    panes = [_pane(7, 12), _pane(9, 12), _pane(10, 20)]
    state = {"statuses": {"7": "running", "9": "input", "10": "done", "100": "input"}}
    state, actions = BADGES["reconcile"](state, panes, {})
    assert "100" not in state["statuses"]
    assert ["rename-tab-by-id", "12", "--", "[🔴 !] feature"] in actions
    assert ["rename-tab-by-id", "20", "--", "[🔵 ✓] feature"] in actions
    state, actions = BADGES["reconcile"](state, panes, {"pane_id": 9, "remove": True})
    assert ["rename-tab-by-id", "12", "--", "[🟠 ↻] feature"] in actions
    state, actions = BADGES["reconcile"](state, panes[1:], {})
    assert "7" not in state["statuses"]


def test_live_locations_override_stale_report_locations() -> None:
    panes = [_pane(7, 20), _pane(9, 12)]
    panes[1]["tab_name"] = "[🟠 ↻] old"
    state, actions = BADGES["reconcile"](
        {}, panes, {"pane_id": 7, "status": "running", "tab_id": 12}
    )
    assert ["rename-tab-by-id", "20", "--", "[🟠 ↻] feature"] in actions
    assert ["rename-tab-by-id", "12", "--", "old"] in actions
    assert BADGES["reconcile"](state, panes, {"status": "input"})[0] == state


def test_input_hooks_resume_running_after_answer_or_approval() -> None:
    hook = runpy.run_path(str(Path(__file__).parents[1] / "scripts/codex-hook"))
    assert hook["status_for"]("PermissionRequest") == "input"
    assert (
        hook["status_for"]("PreToolUse", {"tool_name": "functions.request_user_input"})
        == "input"
    )
    assert hook["status_for"]("PostToolUse") == "running"
    assert hook["status_for"]("Stop") == "done"
    state: dict[str, Any] = {}
    for event, tool, status, expected in [
        ("PreToolUse", "request_user_input", "input", "input"),
        ("PostToolUse", "exec", "running", "input"),
        ("PostToolUse", "request_user_input", "running", "running"),
        ("PreToolUse", "request_user_input_async", "input", "input"),
        ("PostToolUse", "request_user_input_async", "running", "input"),
        ("Stop", "", "done", "input"),
        ("UserPromptSubmit", "", "running", "running"),
    ]:
        state, _ = BADGES["reconcile"](
            state,
            [_pane(7, 12)],
            {
                "pane_id": 7,
                "status": status,
                "event": event,
                "tool_name": tool,
            },
        )
        assert state["statuses"]["7"] == expected


def test_clear_done_only_acknowledges_selected_pane_and_recomputes_tab() -> None:
    panes = [_pane(7, 12), _pane(8, 12), _pane(10, 20)]
    for pane in panes:
        pane.update(title="[🔵 ✓] My agent", tab_name="[🔵 ✓] feature")
    state = {"statuses": {"7": "done", "8": "done", "10": "done"}}
    state, actions = BADGES["reconcile"](state, panes, {"clear_done_pane_id": 7})
    assert state["statuses"] == {"7": "idle", "8": "done", "10": "done"}
    assert actions == [["rename-pane", "--pane-id", "terminal_7", "--", "My agent"]]
    panes[0]["title"] = "My agent"
    state, actions = BADGES["reconcile"](state, panes, {"clear_done_pane_id": 8})
    assert state["statuses"] == {"7": "idle", "8": "idle", "10": "done"}
    assert actions == [
        ["rename-pane", "--pane-id", "terminal_8", "--", "My agent"],
        ["rename-tab-by-id", "12", "--", "feature"],
    ]
    for status in ("running", "input", "idle"):
        state = {"statuses": {"7": status, "8": "done"}, "waiting": {"7": "question"}}
        updated, _ = BADGES["reconcile"](state, panes, {"clear_done_pane_id": 7})
        assert updated == state


def test_done_wins_until_prompt_or_acknowledgement_then_running_resumes() -> None:
    hook = runpy.run_path(str(Path(__file__).parents[1] / "scripts/codex-hook"))
    for report, expected in [
        (
            {
                "pane_id": 7,
                "event": "UserPromptSubmit",
                "status": hook["status_for"]("UserPromptSubmit"),
            },
            "running",
        ),
        ({"clear_done_pane_id": 7}, "idle"),
    ]:
        panes = [_pane(7, 12), _pane(8, 12)]
        state, actions = BADGES["reconcile"](
            {"statuses": {"7": "done", "8": "running"}}, panes, {}
        )
        assert ["rename-tab-by-id", "12", "--", "[🔵 ✓] feature"] in actions
        panes[0]["title"] = "[🔵 ✓] My agent"
        panes[1]["title"] = "[🟠 ↻] My agent"
        for pane in panes:
            pane["tab_name"] = "[🔵 ✓] feature"
        state, actions = BADGES["reconcile"](state, panes, report)
        assert state["statuses"] == {"7": expected, "8": "running"}
        assert actions == [
            [
                "rename-pane",
                "--pane-id",
                "terminal_7",
                "--",
                BADGES["BADGES"][expected] + "My agent",
            ],
            ["rename-tab-by-id", "12", "--", "[🟠 ↻] feature"],
        ]


def _pane(pane_id: int, tab_id: int) -> dict[str, Any]:
    return {
        "id": pane_id,
        "tab_id": tab_id,
        "tab_name": "feature",
        "title": "My agent",
        "is_plugin": False,
        "exited": False,
    }
