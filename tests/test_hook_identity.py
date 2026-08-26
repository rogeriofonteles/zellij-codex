from collections.abc import Callable
import os
from pathlib import Path
import runpy
from tempfile import TemporaryDirectory
from typing import cast
from unittest import mock


def test_hook_finds_worktree_pane_without_zellij_environment() -> None:
    script = runpy.run_path(str(Path(__file__).parents[1] / "scripts" / "codex-hook"))
    pane_context = cast(
        Callable[[Path], tuple[str, str, int | None, int | None]],
        script["pane_context"],
    )
    pane_manifest = [
        {
            "id": 10,
            "is_plugin": False,
            "exited": False,
            "pane_cwd": "/repo/feature",
            "pane_command": "node /usr/bin/codex --yolo",
            "tab_id": 4,
            "tab_position": 3,
        }
    ]
    globals_ = pane_context.__globals__
    with (
        mock.patch.dict(os.environ, {}, clear=True),
        mock.patch.dict(
            globals_,
            {
                "_list_sessions": lambda: ["workbench"],
                "_list_panes": lambda session: pane_manifest,
            },
        ),
    ):
        assert pane_context(Path("/repo/feature")) == ("workbench", "10", 4, 3)


def test_hook_rejects_ambiguous_worktree_panes() -> None:
    script = runpy.run_path(str(Path(__file__).parents[1] / "scripts" / "codex-hook"))
    pane_context = cast(
        Callable[[Path], tuple[str, str, int | None, int | None]],
        script["pane_context"],
    )
    pane_manifest = [
        {
            "id": 10,
            "is_plugin": False,
            "exited": False,
            "pane_cwd": "/repo/feature",
            "pane_command": "codex --yolo",
        }
    ]
    globals_ = pane_context.__globals__
    with (
        mock.patch.dict(os.environ, {}, clear=True),
        mock.patch.dict(
            globals_,
            {
                "_list_sessions": lambda: ["first", "second"],
                "_list_panes": lambda session: pane_manifest,
            },
        ),
    ):
        assert pane_context(Path("/repo/feature")) == ("", "", None, None)


def test_hook_recovers_missing_zellij_socket_dir() -> None:
    script = runpy.run_path(str(Path(__file__).parents[1] / "scripts" / "codex-hook"))
    restore_zellij_socket_dir = cast(
        Callable[[], str | None], script["restore_zellij_socket_dir"]
    )
    globals_ = restore_zellij_socket_dir.__globals__
    with TemporaryDirectory() as directory:
        socket_dir = Path(directory)
        session_socket = socket_dir / "contract_version_1" / "workbench"
        session_socket.parent.mkdir()
        session_socket.touch()
        with (
            mock.patch.dict(
                os.environ,
                {"ZELLIJ_SESSION_NAME": "workbench"},
                clear=True,
            ),
            mock.patch.dict(
                globals_,
                {"_zellij_socket_dir_candidates": lambda: [socket_dir]},
            ),
        ):
            assert restore_zellij_socket_dir() == str(socket_dir)
            assert os.environ["ZELLIJ_SOCKET_DIR"] == str(socket_dir)


if __name__ == "__main__":
    test_hook_finds_worktree_pane_without_zellij_environment()
    test_hook_rejects_ambiguous_worktree_panes()
    test_hook_recovers_missing_zellij_socket_dir()
