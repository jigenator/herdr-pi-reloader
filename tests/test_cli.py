"""Fake CLI + private socket only. Never execute pane commands or contact live Herdr.

cargo build --locked && python3 -m unittest discover -s tests -v
RELOADER_BINARY can select a release/installed binary.
"""
import contextlib
import json
import os
from pathlib import Path
import shlex
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('RELOADER_BINARY', ROOT / 'target/debug/herdr-pi-reloader')).resolve()
FAKE_HERDR = r'''
import json, os, shlex, sys, time
from pathlib import Path
root = Path(os.environ['FIXTURE_DIR'])
args = sys.argv[1:]
with (root / 'calls.jsonl').open('a') as log: log.write(json.dumps(args) + '\n')
fixture = json.loads((root / 'fixture.json').read_text())
state_path = root / 'state.json'
state = json.loads(state_path.read_text()) if state_path.exists() else {}
if args == ['agent', 'list']:
    count = state['lists'] = state.get('lists', 0) + 1
    agents = []
    for item in fixture['agents']:
        agent = dict(item)
        phase = state.get(agent['pane_id'], {})
        if phase.get('quit') and not phase.get('started'): continue
        if fixture.get('becomes_busy') and count >= 2: agent['agent_status'] = 'working'
        if fixture.get('changes_session') and count >= 2:
            agent['agent_session'] = dict(agent['agent_session'], value='/changed.jsonl')
        if phase.get('started'):
            if fixture.get('wrong_resume'):
                agent['agent_session'] = dict(agent['agent_session'], value='/wrong-resumed-session.jsonl')
            agent['agent_status'] = 'done'
            if time.monotonic() - phase['started'] < fixture.get('ready_delay', 0):
                agent['agent_status'] = 'unknown'
        agents.append(agent)
    result = {'agents': agents}
elif args[:3] == ['pane', 'process-info', '--pane']:
    if fixture.get('process_error'): sys.exit('mock process-info failure')
    phase = state.get(args[3], {})
    process = fixture['process']
    if phase.get('quit') and not phase.get('started'):
        process = {'name': 'zsh', 'argv0': 'zsh', 'pid': 10}
        if fixture.get('foreign_process'): process = {'name': 'sleep', 'argv0': 'sleep', 'pid': 30}
    result = {'process_info': {'shell_pid': 10, 'foreground_processes': [process]}}
elif args[:2] == ['pane', 'run']:
    # Record, NEVER execute or forward these commands.
    pane = state.setdefault(args[2], {})
    assert args[3].startswith('pi --session '), 'Slash commands must NEVER enter the editor'
    if args[3].startswith('pi --session '):
        if fixture.get('start_error') == args[2]: sys.exit('mock start failure')
        expected = next(a['agent_session']['value'] for a in fixture['agents'] if a['pane_id'] == args[2])
        assert shlex.split(args[3]) == ['pi', '--session', expected]
        pane['started'] = time.monotonic()
    result = {'type': 'ok'}
else:
    sys.exit('unexpected fake Herdr call: ' + repr(args))
temporary = root / f'state-{os.getpid()}.tmp'
temporary.write_text(json.dumps(state))
temporary.replace(state_path)
if args[:2] != ['pane', 'run']: print(json.dumps({'result': result}))
'''


@contextlib.contextmanager
def fixture(process=None, status='idle', session=True, count=1, **options):
    with tempfile.TemporaryDirectory(dir='/tmp', prefix='pir-') as directory:
        root = Path(directory)
        agents = []
        for i in range(count):
            agent = {'agent': 'pi', 'agent_status': status, 'pane_id': f'w{i + 2}:p1'}
            if session:
                agent['agent_session'] = {'agent': 'pi', 'kind': 'path', 'value': str(root / f"session {i} 'quotes' $(no-evaluation).jsonl")}
            agents.append(agent)
        data = dict(agents=agents, process=dict(process or {'name': 'node', 'argv0': 'pi'}, pid=os.getpid()), **options)
        (root / 'fixture.json').write_text(json.dumps(data))
        (root / 'calls.jsonl').touch()
        herdr = root / 'fake-herdr'
        herdr.write_text(f'#!{sys.executable}\n' + FAKE_HERDR)
        herdr.chmod(0o700)
        path = root / 'herdr.sock'
        env = {'HOME': directory, 'PATH': os.defpath, 'HERDR_BIN_PATH': str(herdr),
               'HERDR_SOCKET_PATH': str(path), 'FIXTURE_DIR': directory,
               'HERDR_PLUGIN_CONTEXT_JSON': json.dumps({'focused_pane_id': 'w1:p1'})}
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(str(path)); listener.listen(); listener.settimeout(.1)
        stopped = threading.Event()
        snapshots = []

        def serve():
            while not stopped.is_set():
                try: connection, _ = listener.accept()
                except socket.timeout: continue
                with connection:
                    request = json.loads(connection.makefile('rb').readline())
                    method, params = request['method'], request['params']
                    with (root / 'calls.jsonl').open('a') as log:
                        log.write(json.dumps(['api', method, params]) + '\n')
                    progress = Path(str(path) + '.pi-reloader-status')
                    snapshots.append(progress.read_text() if progress.exists() else '')
                    if method == 'pane.current':
                        response = {'result': {'pane': {'pane_id': 'w1:p1'}}}
                        if options.get('origin_error'): response = {'error': {'message': 'original pane unavailable'}}
                    elif method == 'pane.focus':
                        response = {'result': {'pane': {'pane_id': params['pane_id']}}}
                        if options.get('focus_error') == params['pane_id']:
                            response = {'error': {'message': 'mock focus failure'}}
                    else:
                        response = {'error': {'message': 'unexpected test API method'}}
                    connection.sendall((json.dumps(response) + '\n').encode())
        guard_dir = Path(f'/tmp/herdr-pi-reloader-{os.getuid()}')
        guard_dir.mkdir(mode=0o700, exist_ok=True)
        guard_path = guard_dir / f'{os.getpid()}.sock'
        guard_listener = socket.socket(socket.AF_UNIX)
        if not options.get('guard_missing'):
            guard_listener.bind(str(guard_path)); guard_listener.listen(); guard_listener.settimeout(.1)
            guard_path.chmod(0o666 if options.get('guard_public') else 0o600)
        generations = {}

        def serve_guard():
            while not stopped.is_set() and not options.get('guard_missing'):
                try: connection, _ = guard_listener.accept()
                except socket.timeout: continue
                with connection:
                    request = json.loads(connection.makefile('rb').readline())
                    pane, action = request['pane_id'], request['action']
                    with (root / 'calls.jsonl').open('a') as log:
                        log.write(json.dumps(['guard', action, pane]) + '\n')
                    generation = generations.get(pane, 'initial')
                    status = 'ready'
                    if options.get('draft') or (action != 'probe' and options.get('guard_becomes_draft')): status = 'draft'
                    if options.get('guard_busy') or (action != 'probe' and options.get('guard_becomes_busy')): status = 'busy'
                    response = dict(request, status=status, generation=generation)
                    if action != 'probe' and status == 'ready':
                        response['status'] = 'accepted'
                        if action == 'quit':
                            state_path = root / 'state.json'
                            state = json.loads(state_path.read_text())
                            state.setdefault(pane, {})['quit'] = True
                            state_path.write_text(json.dumps(state))
                        elif not options.get('reload_no_new_generation'):
                            generations[pane] = 'replacement'
                    if options.get('guard_wrong_identity'): response['pid'] += 1
                    connection.sendall((json.dumps(response) + '\n').encode())
        thread = threading.Thread(target=serve)
        guard_thread = threading.Thread(target=serve_guard)
        thread.start(); guard_thread.start()
        try:
            yield root, env, agents, snapshots
        finally:
            stopped.set(); thread.join(3); guard_thread.join(3)
            listener.close(); guard_listener.close()
            if not options.get('guard_missing'): guard_path.unlink()


def calls_at(root):
    return [json.loads(line) for line in (root / 'calls.jsonl').read_text().splitlines()]


def focus_calls(calls):
    return [c[2]['pane_id'] for c in calls if c[:2] == ['api', 'pane.focus']]


class CliTests(unittest.TestCase):
    def run_cli(self, command, **options):
        with fixture(**options) as (root, env, agents, snapshots):
            result = subprocess.run([str(BINARY), command], env=env, capture_output=True, text=True, timeout=50)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(Path(env['HERDR_SOCKET_PATH'] + '.pi-reloader-status').exists())
            return result.stdout, calls_at(root), agents, snapshots

    def test_current_and_legacy_pi_identity(self):
        for process in ({'name': 'node', 'argv0': 'pi'}, {'name': 'node', 'argv0': '/opt/bin/pi'},
                        {'name': 'node', 'argv': ['/opt/bin/pi', '--session', 'file']},
                        {'name': 'node', 'argv0': None, 'argv': ['pi']}, {'name': 'pi'}):
            with self.subTest(process=process):
                output, calls, _, _ = self.run_cli('reload', process=process)
                self.assertIn('reloaded: 1', output)
                self.assertFalse(any(c[:2] == ['pane', 'run'] for c in calls))
                self.assertIn(['guard', 'reload', 'w2:p1'], calls)

    def test_reset_waits_for_shell_and_ready_session_then_restores_focus(self):
        output, calls, agents, progress = self.run_cli('reset', count=2, ready_delay=.4)
        self.assertIn('reset: 2', output); self.assertIn('visited: 2', output)
        self.assertEqual(focus_calls(calls), ['w2:p1', 'w3:p1', 'w1:p1'])
        runs = [c for c in calls if c[:2] == ['pane', 'run']]
        self.assertEqual([c[2] for c in runs], ['w2:p1', 'w3:p1'])
        for index, agent in enumerate(agents):
            quit_index = calls.index(['guard', 'quit', agent['pane_id']])
            self.assertEqual(calls[quit_index + 1], ['pane', 'process-info', '--pane', agent['pane_id']])
            self.assertEqual(shlex.split(runs[index][3]), ['pi', '--session', agent['agent_session']['value']])
            self.assertIn('agent', calls[quit_index + 2])  # Old lifecycle must disappear before restart.
        self.assertTrue(any('1/2' in text and 'viewing' in text for text in progress))
        self.assertIn('returning', progress[-1])

    def test_existing_done_panes_are_not_deliberately_acknowledged(self):
        output, calls, _, _ = self.run_cli('reset', status='done')
        self.assertIn('reset: 1', output); self.assertIn('visited: 0', output)
        self.assertEqual(focus_calls(calls), [])

    def test_non_pi_processes_are_skipped_and_counted(self):
        for process in ({'name': 'node', 'argv0': 'node'}, {'name': 'node', 'argv0': 'not-pi'},
                        {'name': 'node', 'argv': ['node', 'pi']}, {'name': 'node', 'argv0': None}):
            for command in ('reload', 'reset'):
                with self.subTest(process=process, command=command):
                    output, calls, _, _ = self.run_cli(command, process=process)
                    self.assertIn('skipped_non_pi: 1', output)
                    self.assertFalse(any(c[:2] == ['pane', 'run'] for c in calls))

    def test_working_agents_and_missing_reset_sessions_are_untouched(self):
        for command in ('reload', 'reset'):
            output, calls, _, _ = self.run_cli(command, status='working')
            self.assertIn('skipped_unsafe_status: 1', output)
            self.assertFalse(any(c[:2] == ['pane', 'run'] for c in calls))
        output, calls, _, _ = self.run_cli('reset', session=False)
        self.assertIn('failed: 1', output)
        self.assertFalse(any(c[:2] == ['pane', 'run'] for c in calls))

    def test_process_inspection_failure_never_runs_commands(self):
        for command in ('reload', 'reset'):
            output, calls, _, _ = self.run_cli(command, process_error=True)
            self.assertIn('failed: 1', output); self.assertIn('mock process-info failure', output)
            self.assertFalse(any(c[:2] == ['pane', 'run'] for c in calls))

    def test_rechecks_busy_and_session_identity_before_quitting(self):
        for option, expected in [('becomes_busy', 'skipped_unsafe_status: 1'), ('changes_session', 'changed session; not reset')]:
            output, calls, _, _ = self.run_cli('reset', **{option: True})
            self.assertIn(expected, output)
            self.assertFalse(any(c[:2] == ['pane', 'run'] for c in calls))

    def test_foreign_process_after_quit_never_receives_start_command(self):
        output, calls, _, _ = self.run_cli('reset', foreign_process=True)
        self.assertIn('shell not ready', output)
        self.assertEqual([c for c in calls if c[:2] == ['pane', 'run']], [])
        self.assertIn(['guard', 'quit', 'w2:p1'], calls)
        self.assertEqual(focus_calls(calls), [])

    def test_wrong_resumed_session_is_not_successful_or_focused(self):
        output, calls, _, _ = self.run_cli('reset', wrong_resume=True)
        self.assertIn('reset: 0', output); self.assertIn('failed: 1', output)
        self.assertIn('resumed Pi/session not ready', output)
        self.assertEqual(focus_calls(calls), [])

    def test_restore_failure_is_reported_without_picking_another_pane(self):
        output, calls, _, _ = self.run_cli('reset', focus_error='w1:p1')
        self.assertIn('Could not restore original pane w1:p1', output)
        self.assertEqual(focus_calls(calls), ['w2:p1', 'w1:p1'])

    def test_focus_failure_stops_cycling_and_restores_origin(self):
        output, calls, _, _ = self.run_cli('reset', count=2, focus_error='w2:p1')
        self.assertIn('mock focus failure', output)
        self.assertEqual(focus_calls(calls), ['w2:p1', 'w1:p1'])
        self.assertFalse(any(c[:3] == ['pane', 'run', 'w3:p1'] for c in calls))

    def test_later_start_failure_still_restores_original_pane(self):
        output, calls, _, _ = self.run_cli('reset', count=2, start_error='w3:p1')
        self.assertIn('reset: 1', output); self.assertIn('failed: 1', output)
        self.assertEqual(focus_calls(calls), ['w2:p1', 'w1:p1'])

    def test_status_is_read_only_scoped_and_crash_safe_with_reset_lock(self):
        with fixture(ready_delay=20) as (root, env, _, _):
            reset = subprocess.Popen([str(BINARY), 'reset'], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    status = subprocess.check_output([str(BINARY), 'status'], env=env, text=True)
                    if 'waiting' in status: break
                    time.sleep(.05)
                self.assertIn("DON'T TYPE", status); self.assertIn('waiting', status)
                read_only_env = dict(env, HERDR_BIN_PATH='/must-not-execute-herdr')
                self.assertIn(b"DON'T TYPE", subprocess.check_output([str(BINARY), 'status'], env=read_only_env))
                other_env = dict(read_only_env, HERDR_SOCKET_PATH=env['HERDR_SOCKET_PATH'] + '-other')
                self.assertEqual(subprocess.check_output([str(BINARY), 'status'], env=other_env), b'')
                second = subprocess.run([str(BINARY), 'reset'], env=env, capture_output=True, text=True, timeout=5)
                self.assertNotEqual(second.returncode, 0)
                self.assertIn('another reset may be running', second.stderr)
            finally:
                reset.kill(); reset.communicate(timeout=5)
            self.assertEqual(subprocess.check_output([str(BINARY), 'status'], env=env), b'')

    def test_drafts_and_guard_busy_skip_without_terminal_input_or_focus(self):
        for option, expected in [('draft', 'skipped_drafts: 1'), ('guard_becomes_draft', 'skipped_drafts: 1'),
                                 ('guard_busy', 'skipped_unsafe_status: 1'), ('guard_becomes_busy', 'skipped_unsafe_status: 1')]:
            for command in ('reload', 'reset'):
                with self.subTest(option=option, command=command):
                    output, calls, _, _ = self.run_cli(command, **{option: True})
                    self.assertIn(expected, output)
                    self.assertIn('failed: 0', output)
                    self.assertFalse(any(c[:2] == ['pane', 'run'] for c in calls))
                    self.assertEqual(focus_calls(calls), [])

    def test_missing_public_or_wrong_guard_has_no_unsafe_fallback(self):
        for option in ('guard_missing', 'guard_public', 'guard_wrong_identity'):
            for command in ('reload', 'reset'):
                with self.subTest(option=option, command=command):
                    output, calls, _, _ = self.run_cli(command, **{option: True})
                    self.assertIn('failed: 1', output)
                    self.assertFalse(any(c[:2] == ['pane', 'run'] or c[:2] in (['guard', 'quit'], ['guard', 'reload']) for c in calls))
                    self.assertEqual(focus_calls(calls), [])

    def test_reload_requires_a_replacement_runtime_not_just_acceptance(self):
        output, calls, _, _ = self.run_cli('reload', reload_no_new_generation=True)
        self.assertIn('reloaded: 0', output)
        self.assertIn('reload did not confirm a new guard runtime', output)
        self.assertFalse(any(c[:2] == ['pane', 'run'] for c in calls))

    def test_missing_origin_aborts_before_mutation(self):
        with fixture(origin_error=True) as (root, env, _, _):
            result = subprocess.run([str(BINARY), 'reset'], env=env, capture_output=True, text=True, timeout=5)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('original pane unavailable', result.stderr)
            self.assertFalse(any(c[:2] == ['pane', 'run'] for c in calls_at(root)))


if __name__ == '__main__':
    unittest.main()
