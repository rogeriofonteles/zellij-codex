from collections.abc import Callable
import os
from pathlib import Path
import runpy
from tempfile import TemporaryDirectory
from typing import cast
from unittest import mock


def test_launcher_overrides_inherited_pane_identity() -> None:
    script = runpy.run_path(str(Path(__file__).parents[1] / "scripts" / "codex-launch"))
    pane_context = cast(
        Callable[[Path], tuple[str, int | None, int | None]],
        script["pane_context"],
    )
    pane_manifest = [
        {
            "id": 12,
            "is_plugin": False,
            "tab_id": 4,
            "tab_position": 3,
        }
    ]
    with (
        mock.patch.dict(
            os.environ,
            {"ZELLIJ_PANE_ID": "12", "ZELLIJ_CODEX_PANE_ID": "7"},
        ),
        mock.patch.dict(
            pane_context.__globals__, {"_list_panes": lambda: pane_manifest}
        ),
    ):
        assert pane_context(Path.cwd()) == ("12", 4, 3)


def test_launcher_recovers_missing_zellij_socket_dir() -> None:
    script = runpy.run_path(str(Path(__file__).parents[1] / "scripts" / "codex-launch"))
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


def test_linked_worktree_inherits_trust_from_primary() -> None:
    script = runpy.run_path(str(Path(__file__).parents[1] / "scripts" / "codex-launch"))
    codex_arguments = cast(
        Callable[[list[str], Path], list[str]], script["codex_arguments"]
    )
    primary = Path("/repo/main")
    linked = Path("/repo/feature")
    globals_ = codex_arguments.__globals__
    with (
        mock.patch.dict(
            globals_,
            {
                "_git_worktrees": lambda cwd: [primary, linked],
                "_codex_projects": lambda: {str(primary): {"trust_level": "trusted"}},
            },
        ),
    ):
        assert codex_arguments(["--yolo"], linked) == [
            "--config",
            'projects."/repo/feature".trust_level="trusted"',
            "--yolo",
        ]


def test_untrusted_primary_does_not_trust_linked_worktree() -> None:
    script = runpy.run_path(str(Path(__file__).parents[1] / "scripts" / "codex-launch"))
    codex_arguments = cast(
        Callable[[list[str], Path], list[str]], script["codex_arguments"]
    )
    primary = Path("/repo/main")
    linked = Path("/repo/feature")
    globals_ = codex_arguments.__globals__
    with (
        mock.patch.dict(
            globals_,
            {
                "_git_worktrees": lambda cwd: [primary, linked],
                "_codex_projects": lambda: {},
            },
        ),
    ):
        assert codex_arguments(["--yolo"], linked) == ["--yolo"]


if __name__ == "__main__":
    test_launcher_overrides_inherited_pane_identity()
    test_launcher_recovers_missing_zellij_socket_dir()
    test_linked_worktree_inherits_trust_from_primary()
    test_untrusted_primary_does_not_trust_linked_worktree()
