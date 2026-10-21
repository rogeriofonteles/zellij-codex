import json
import os
from pathlib import Path
import runpy
import sys
from unittest import mock

import pytest

SCRIPTS = Path(__file__).parents[1] / 'scripts'
sys.path.insert(0, str(SCRIPTS))
import zellij_codex_binding as binding

PTY = runpy.run_path(str(SCRIPTS / 'codex-pty'))
THREAD = '01a12c70-8ab5-7691-9d74-7cb7391fbdf8'


@pytest.mark.parametrize('terminator', [b'\x07', b'\x1b\\'])
@pytest.mark.parametrize('identity', [THREAD.encode(), (THREAD[:29] + '...').encode()])
def test_title_filter_handles_every_chunk_boundary(terminator, identity):
    original = b'before\x1b]0;' + identity + b' | Conversation | project' + terminator + b'after'
    expected = b'before\x1b]0;Conversation | project' + terminator + b'after'
    for boundary in range(len(original) + 1):
        titles = []
        parser = PTY['TitleFilter'](lambda *args: titles.append(args))
        assert parser.feed(original[:boundary]) + parser.feed(original[boundary:]) == expected
        assert titles == [(THREAD[:29], 'Conversation | project')]


def test_other_osc_sequences_and_escape_codes_pass_through():
    data = b'\x1b[31mtext\x1b]52;c;YWJj\x07\x1b]0;ordinary title\x07'
    parser = PTY['TitleFilter'](lambda *_: pytest.fail('not an identity title'))
    assert b''.join(parser.feed(bytes([byte])) for byte in data) == data


def test_pending_lifecycle_state_is_available_before_binding(tmp_path, monkeypatch):
    monkeypatch.setenv('XDG_CACHE_HOME', str(tmp_path))
    binding.record_event(THREAD, 'UserPromptSubmit')
    assert binding.state_for_prefix(THREAD[:29])['status'] == 'running'
    binding.record_event(THREAD, 'PreToolUse', 'request_user_input_async')
    binding.record_event(THREAD, 'PostToolUse', 'exec')
    assert binding.state_for_prefix(THREAD[:29])['status'] == 'input'
    binding.record_event(THREAD, 'Stop')
    assert binding.state_for_prefix(THREAD[:29])['status'] == 'done'
    assert 'prompt' not in binding.state_for_prefix(THREAD[:29])


def test_daemon_hooks_follow_client_switch_and_reject_old_cache(tmp_path, monkeypatch):
    monkeypatch.setenv('XDG_CACHE_HOME', str(tmp_path))
    monkeypatch.delenv('ZELLIJ_PANE_ID', raising=False)
    hook = runpy.run_path(str(SCRIPTS / 'codex-hook'))
    resolve = hook['session_pane_context']
    active = {'prefix': THREAD[:29], 'session': 'work', 'pane': '7', 'socket': 'socket',
              'pid': os.getpid(), 'owner': binding.owner_token(os.getpid())}
    path = binding.root() / 'bindings/client.json'
    binding.atomic_json(path, active)
    with mock.patch.dict(resolve.__globals__, {
        'session_socket_identity': lambda _: 'socket',
        '_list_panes': lambda _: [{'id': 7, 'tab_id': 2, 'tab_position': 1}],
        'pane_context': lambda _: ('', '', None, None),
    }):
        assert resolve(Path('/unrelated-cwd'), THREAD) == ('work', '7', 2, 1)
        import hashlib
        binding.atomic_json(binding.root() / 'identities' / (hashlib.sha256(THREAD.encode()).hexdigest() + '.json'),
                            {'session': 'work', 'pane': '7', 'socket': 'socket'})
        active['prefix'] = '01a12c54-b3a6-7bf3-84b8-a80b8a9'
        binding.atomic_json(path, active)
        assert resolve(Path('/unrelated-cwd'), THREAD) == ('', '', None, None)
        active['owner'] = 'expired'
        binding.atomic_json(path, active)
        assert not binding.active_bindings()


def test_command_install_is_idempotent_and_preserves_symlink(tmp_path):
    install = runpy.run_path(str(SCRIPTS / 'install-command'))['install_command']
    real = tmp_path / 'stock'
    real.write_text('#!/bin/sh\nexit 0\n')
    (tmp_path / 'codex').symlink_to(real)
    install(tmp_path, SCRIPTS / 'codex-command')
    install(tmp_path, SCRIPTS / 'codex-command')
    assert (tmp_path / 'codex-real').is_symlink()
    assert (tmp_path / 'codex-real').resolve() == real
    assert 'zellij-codex command wrapper' in (tmp_path / 'codex').read_text()


@pytest.mark.parametrize('args,expected', [([], True), (['resume', '--last'], True),
    (['fork', THREAD], True), (['agents'], True), (['exec', 'hello'], False),
    (['--model', 'test', 'exec', 'hello'], False), (['--version'], False),
    (['--', 'exec'], True), (['--profile', 'exec'], True)])
def test_only_interactive_commands_use_bridge(args, expected):
    launch = runpy.run_path(str(SCRIPTS / 'codex-launch'))
    with mock.patch.object(sys.stdin, 'isatty', return_value=True), mock.patch.object(sys.stdout, 'isatty', return_value=True):
        assert launch['uses_terminal_bridge'](args) == expected


def test_same_thread_in_two_clients_maps_both_panes(tmp_path, monkeypatch):
    monkeypatch.setenv('XDG_CACHE_HOME', str(tmp_path))
    hook = runpy.run_path(str(SCRIPTS / 'codex-hook'))
    contexts = hook['active_pane_contexts']
    for pane in (7, 8):
        binding.atomic_json(binding.root() / f'bindings/{pane}.json', {
            'prefix': THREAD[:29], 'session': 'work', 'pane': str(pane), 'socket': 'socket',
            'pid': os.getpid(), 'owner': binding.owner_token(os.getpid()),
        })
    with mock.patch.dict(contexts.__globals__, {
        'session_socket_identity': lambda _: 'socket',
        '_list_panes': lambda _: [{'id': p, 'tab_id': 2, 'tab_position': 1} for p in (7, 8)],
    }):
        assert set(contexts(THREAD)) == {('work', '7', 2, 1), ('work', '8', 2, 1)}


def test_title_generation_spinner_does_not_trigger_repeated_registration():
    titles = []
    parser = PTY['TitleFilter'](lambda *args: titles.append(args))
    for spinner in ('⠙', '⠹'):
        parser.feed(f'\x1b]0;{THREAD[:29]}... {spinner} | Name {spinner} | Project\x07'.encode())
    assert titles[0] == titles[1]


def test_title_refresh_preserves_cleared_done_and_pending_question():
    badges = runpy.run_path(str(SCRIPTS / 'codex-badges'))
    panes = [{'id': 7, 'is_plugin': False, 'exited': False, 'title': 'old', 'tab_id': 2, 'tab_name': 'work'}]
    state, _ = badges['reconcile']({}, panes, {'pane_id': 7, 'status': 'input',
        'binding_prefix': THREAD[:29], 'binding_waiting': 'request_user_input_async'})
    state, _ = badges['reconcile'](state, panes, {'pane_id': 7, 'binding_prefix': THREAD[:29], 'base_title': 'renamed'})
    assert state['waiting']['7'] == 'request_user_input_async'
    state, _ = badges['reconcile'](state, panes, {'pane_id': 7, 'status': 'done', 'event': 'Stop'})
    state, _ = badges['reconcile'](state, panes, {'clear_done_pane_id': 7})
    state, _ = badges['reconcile'](state, panes, {'pane_id': 7, 'binding_prefix': THREAD[:29], 'base_title': 'renamed again'})
    assert state['statuses']['7'] == 'idle'
