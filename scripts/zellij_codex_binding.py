"""Conversation identity shared by terminal clients and daemon lifecycle hooks."""
import fcntl
import json
import os
import re
import tempfile
from pathlib import Path

UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")


def root():
    return Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache')) / 'zellij-codex'


def atomic_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as stream:
        json.dump(value, stream)
    os.replace(stream.name, path)


def read_json(path):
    try:
        value = json.loads(path.read_text())
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError):
        return {}


def owner_token(pid):
    try:
        # A PID alone can be reused after a client exits.
        return Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19]
    except (OSError, IndexError):
        return None


def active_bindings():
    result = []
    for path in (root() / 'bindings').glob('*.json'):
        binding = read_json(path)
        pid = binding.get('pid')
        if not isinstance(pid, int) or not binding.get('owner'):
            continue
        if owner_token(pid) == binding['owner']:
            result.append(binding)
    return result


def record_event(session_id, event, tool=''):
    """Keep lifecycle state even when SessionStart precedes the client's title.

    No prompts, tool arguments, or assistant messages are stored here.
    """
    if not UUID.fullmatch(session_id):
        return
    directory = root() / 'events'
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    path = directory / f'{session_id}.json'
    with path.with_suffix('.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        previous = read_json(path)
        status = {'SessionStart': 'idle', 'UserPromptSubmit': 'running',
                  'PermissionRequest': 'input', 'Stop': 'done',
                  'SessionEnd': 'idle'}.get(event, 'running')
        waiting = previous.get('waiting')
        if event == 'PermissionRequest' or (event == 'PreToolUse' and 'request_user_input' in tool):
            status, waiting = 'input', tool
        elif event == 'PostToolUse' and waiting is not None:
            if waiting != tool or waiting.endswith('request_user_input_async'):
                status = 'input'
            else:
                waiting = None
        else:
            waiting = None
        atomic_json(path, {'thread': session_id, 'status': status, 'waiting': waiting})


def state_for_prefix(prefix):
    if len(prefix) < 29 or not re.fullmatch(r'[0-9a-f-]+', prefix):
        return {}
    matches = list((root() / 'events').glob(prefix + '*.json'))
    return read_json(matches[0]) if len(matches) == 1 else {}
