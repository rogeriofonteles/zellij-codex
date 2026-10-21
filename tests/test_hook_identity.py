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


def test_hook_accepts_explicit_identity_without_cwd() -> None:
    script = runpy.run_path(str(Path(__file__).parents[1] / "scripts" / "codex-hook"))
    pane_context = script["pane_context"]
    manifest = [{"id": 2, "is_plugin": False, "exited": False, "tab_id": 7, "tab_position": 0}]
    with (
        mock.patch.dict(os.environ, {"ZELLIJ_SESSION_NAME": "work", "ZELLIJ_PANE_ID": "2"}, clear=True),
        mock.patch.dict(pane_context.__globals__, {"_list_panes": lambda session: manifest}),
    ):
        assert pane_context(Path("/repo")) == ("work", "2", 7, 0)
    with (
        mock.patch.dict(os.environ, {"ZELLIJ_PANE_ID": "2"}, clear=True),
        mock.patch.dict(pane_context.__globals__, {"_list_sessions": lambda: ["work"], "_list_panes": lambda session: manifest}),
    ):
        assert pane_context(Path("/repo")) == ("", "", None, None)


def test_session_identity_survives_missing_commands_and_ambiguous_cwd(tmp_path):
    script = runpy.run_path(str(Path(__file__).parents[1] / "scripts" / "codex-hook"))
    resolve = script["session_pane_context"]
    manifest = [{"id": 2, "pane_cwd": "/repo", "pane_command": "codex", "tab_id": 1}]
    with (
        mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(tmp_path)}, clear=True),
        mock.patch.dict(resolve.__globals__, {
            "_list_sessions": lambda: ["work"],
            "_list_panes": lambda session: manifest,
            "session_socket_identity": lambda session: "socket-1",
        }),
    ):
        assert resolve(Path("/repo"), "agent")[1] == "2"
        manifest[0].pop("pane_command")
        manifest.append({"id": 3, "pane_cwd": "/repo", "pane_command": "codex"})
        assert resolve(Path("/repo"), "agent")[1] == "2"
        # A restarted Zellij session must not reuse a stale pane identity.
        with mock.patch.dict(resolve.__globals__, {"session_socket_identity": lambda session: "socket-2"}):
            assert resolve(Path("/repo"), "agent")[1] == "3"


def test_pane_lookup_tolerates_slow_zellij_response():
    import subprocess
    import sys
    script = runpy.run_path(str(Path(__file__).parents[1] / "scripts" / "codex-hook"))
    lookup = script["_list_panes"]
    real_run = subprocess.run

    def slow_zellij(command, **kwargs):
        return real_run([sys.executable, "-c",
            "import time, json; time.sleep(1.2); print(json.dumps([dict(id=17)]))"], **kwargs)

    with mock.patch.object(subprocess, "run", side_effect=slow_zellij):
        assert lookup("work") == [{"id": 17}]


def test_recover_pane_from_codex_ancestor(tmp_path):
    script = runpy.run_path(str(Path(__file__).parents[1] / 'scripts/codex-hook'))
    restore = script['restore_pane_environment']
    shell = tmp_path / '100'
    shell.mkdir()
    (shell / 'exe').symlink_to('/usr/bin/bash')
    (shell / 'stat').write_text('100 (shell with spaces) S 200 0')
    codex = tmp_path / '200'
    codex.mkdir()
    (codex / 'exe').symlink_to('/runtime/bin/codex (deleted)')
    (codex / 'environ').write_text('ZELLIJ_SESSION_NAME=work\0ZELLIJ_PANE_ID=17\0SECRET=ignored\0')
    with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(os, 'getppid', return_value=100):
        restore(tmp_path)
        assert dict(os.environ) == {
            'ZELLIJ_SESSION_NAME': 'work', 'ZELLIJ_PANE_ID': '17',
            'ZELLIJ_CODEX_PANE_ID': '17',
        }


def test_desktop_codex_does_not_inherit_outer_pane(tmp_path):
    script = runpy.run_path(str(Path(__file__).parents[1] / 'scripts/codex-hook'))
    codex = tmp_path / '100'
    codex.mkdir()
    (codex / 'exe').symlink_to('/app/bin/codex')
    (codex / 'environ').write_text('')
    with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(os, 'getppid', return_value=100):
        script['restore_pane_environment'](tmp_path)
        assert not os.environ


def test_explicit_pane_does_not_need_proc(tmp_path):
    script = runpy.run_path(str(Path(__file__).parents[1] / 'scripts/codex-hook'))
    with mock.patch.dict(os.environ, {'ZELLIJ_SESSION_NAME': 'work', 'ZELLIJ_PANE_ID': '4'}, clear=True):
        script['restore_pane_environment'](tmp_path)
        assert os.environ['ZELLIJ_PANE_ID'] == '4'
