import os
import subprocess
import sys
from pathlib import Path

import pytest


@pytest.mark.parametrize("with_workbench", [False, True])
def test_install_portable_config_and_preserve_original(
    tmp_path: Path, with_workbench: bool
) -> None:
    root = Path(__file__).parents[1]
    config_home = tmp_path / "machine config"
    config_dir = config_home / "zellij"
    (config_dir / "plugins").mkdir(parents=True)
    (config_dir / "plugins/zellij-codex-tab-bar.wasm").write_bytes(b"test plugin")
    config = config_dir / "config.kdl"
    config.write_text('default_shell "/bin/sh"\n')
    command = [
        sys.executable,
        str(root / "scripts/install-zellij-config"),
        "--shell",
        "/bin/sh",
    ]
    workbench = tmp_path / "user's workbench"
    if with_workbench:
        (workbench / "bin").mkdir(parents=True)
        for name in ("surface-workbench", "nvim-workbench"):
            path = workbench / "bin" / name
            path.write_text("#!/bin/sh\nexit 0\n")
            path.chmod(0o755)
        plugin = tmp_path / "worktree.wasm"
        plugin.write_bytes(b"worktree plugin")
        command.extend(
            ["--workbench-root", str(workbench), "--worktree-plugin", str(plugin)]
        )
    env = {**os.environ, "XDG_CONFIG_HOME": str(config_home)}
    subprocess.run(command, env=env, check=True, capture_output=True)
    installed = config.read_text()
    assert "__" not in installed
    assert str(config_dir / "layouts/default.kdl") in installed
    assert ('bind "Ctrl k"' in installed) == with_workbench
    assert ('bind "Alt t"' in installed) == with_workbench
    assert 'bind "n"' in installed and 'bind "Alt a"' in installed
    if with_workbench:
        assert (
            config_dir / "plugins/zellij-worktree.wasm"
        ).read_bytes() == b"worktree plugin"
        for path in (workbench / "layouts").glob("*.kdl"):
            assert (
                str(config_dir / "plugins/zellij-codex-tab-bar.wasm")
                in path.read_text()
            )
            assert 'name "Codex"' not in path.read_text()
    subprocess.run(
        ["zellij", "--config", str(config), "setup", "--check"],
        env=env,
        check=True,
        capture_output=True,
    )
    subprocess.run(command, env=env, check=True, capture_output=True)
    assert config.read_text() == installed
    assert (
        config.with_name("config.kdl.before-zellij-codex").read_text()
        == 'default_shell "/bin/sh"\n'
    )


def test_missing_workbench_does_not_replace_config(tmp_path: Path) -> None:
    config_dir = tmp_path / "zellij"
    (config_dir / "plugins").mkdir(parents=True)
    (config_dir / "plugins/zellij-codex-tab-bar.wasm").write_bytes(b"test plugin")
    config = config_dir / "config.kdl"
    config.write_text("original config")
    result = subprocess.run(
        [
            sys.executable,
            str(Path(__file__).parents[1] / "scripts/install-zellij-config"),
            "--shell",
            "/bin/sh",
            "--workbench-root",
            str(tmp_path / "missing"),
        ],
        env={**os.environ, "XDG_CONFIG_HOME": str(tmp_path)},
        check=False,
        capture_output=True,
        text=True,
    )
    assert result.returncode != 0 and "Missing executable" in result.stderr
    assert config.read_text() == "original config"
