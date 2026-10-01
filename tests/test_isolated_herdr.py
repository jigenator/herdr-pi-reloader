"""Opt-in real Herdr client/server test, exclusively in a temporary HOME/socket.

HERDR_RELOADER_ISOLATED=1 python3 -m unittest discover -s tests -p test_isolated_herdr.py -v
Uses a fake `pi` first on PATH; no real Pi or user session is touched. Evidence stays in /tmp.
"""
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
import shutil
import signal
import socket
import struct
import subprocess
import tempfile
import termios
import time
import unicodedata
import unittest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('RELOADER_BINARY', ROOT / 'target/debug/herdr-pi-reloader')).resolve()
HERDR, NODE, BASH = (shutil.which(name) for name in ('herdr', 'node', 'bash'))


class Screen:
    """Small text-only VT recorder for Herdr's cursor-positioned screen output."""
    def __init__(self):
        self.grid = [[' '] * 160 for _ in range(50)]
        self.y = self.x = 0
        self.pending = ''
        self.decoder = codecs.getincrementaldecoder('utf8')('replace')

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        while self.pending:
            if self.pending.startswith('\x1b'):
                match = re.match(r'\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1bP.*?\x1b\\|\x1b[()][A-Z0-9]|\x1b[^\[\]P()]', self.pending, re.S)
                if not match: break
                esc = match[0]; self.pending = self.pending[len(esc):]
                if esc.startswith('\x1b['):
                    nums = [int(n) if n.isdigit() else 0 for n in esc[2:-1].split(';')]
                    final = esc[-1]
                    if final in 'Hf':
                        self.y = min(49, (nums[0] or 1) - 1)
                        self.x = min(159, (nums[1] if len(nums) > 1 and nums[1] else 1) - 1)
                    elif final == 'J' and nums[0] in (2, 3): self.grid = [[' '] * 160 for _ in range(50)]
                    elif final == 'K':
                        start, end = (0, 160) if nums[0] == 2 else ((0, self.x + 1) if nums[0] == 1 else (self.x, 160))
                        self.grid[self.y][start:end] = [' '] * (end - start)
                    elif final == 'G': self.x = min(159, (nums[0] or 1) - 1)
                    elif final == 'A': self.y = max(0, self.y - (nums[0] or 1))
                    elif final == 'B': self.y = min(49, self.y + (nums[0] or 1))
                    elif final == 'C': self.x = min(159, self.x + (nums[0] or 1))
                    elif final == 'D': self.x = max(0, self.x - (nums[0] or 1))
                continue
            c, self.pending = self.pending[0], self.pending[1:]
            if c == '\r': self.x = 0
            elif c == '\n': self.y = min(49, self.y + 1)
            elif c == '\b': self.x = max(0, self.x - 1)
            elif c >= ' ' and not unicodedata.combining(c):
                self.grid[self.y][min(159, self.x)] = c
                self.x = min(159, self.x + (2 if unicodedata.east_asian_width(c) in ('W', 'F') else 1))

    def text(self):
        return '\n'.join(''.join(row).rstrip() for row in self.grid)


@unittest.skipUnless(os.environ.get('HERDR_RELOADER_ISOLATED') == '1' and HERDR and NODE and BASH, 'opt-in real Herdr, fake Pi')
class IsolatedHerdrTest(unittest.TestCase):
    def setUp(self):
        self.base = Path(tempfile.mkdtemp(dir='/tmp', prefix='pir-ui-'))
        print(f'\nIsolated evidence: {self.base}', flush=True)
        self.home = self.base / 'home'
        self.config = self.home / '.config/herdr'
        self.config.mkdir(parents=True)
        self.socket = str(self.config / 'herdr.sock')
        self.bin = self.base / 'bin'; self.bin.mkdir()
        self.env = {'HOME': str(self.home), 'PATH': f'{self.bin}:{os.environ["PATH"]}',
                    'TERM': 'xterm-256color', 'LANG': 'en_US.UTF-8', 'SHELL': BASH,
                    'HERDR_DISABLE_SOUND': '1', 'HERDR_SOCKET_PATH': self.socket, 'HERDR_BIN_PATH': HERDR}
        self.client_pid = self.master = None
        self.server_log = open(self.base / 'server.log', 'wb')
        self.raw = open(self.base / 'client.ansi', 'wb')
        self.screen = Screen()
        self.observed = set()
        self.server = None
        self.addCleanup(self.stop)
        command = shlex.quote(str(BINARY)) + ' status'
        (self.config / 'config.toml').write_text(f'''onboarding = false
[terminal]
default_shell = {json.dumps(BASH)}
shell_mode = "non_login"
[update]
version_check = false
manifest_check = false
[ui]
tab_bar_right = [{{ type = "command", command = {json.dumps(command)}, interval_seconds = 1, timeout_seconds = 1 }}]
[ui.sidebar.agents]
rows = [["workspace", "state_text"], ["agent"]]
''')
        # This fake process handles only /quit and lifecycle reports on OUR socket.
        fake = self.base / 'fake-pi.js'
        fake.write_text('''const fs = require('fs'), cp = require('child_process');
const root = require('path').dirname(process.env.HOME);
if (!process.env.HERDR_SOCKET_PATH.startsWith(root + '/')) throw Error('unsafe test socket');
const session = process.argv[process.argv.indexOf('--session') + 1];
const busy = process.argv.includes('--busy');
process.title = 'pi';
fs.appendFileSync(root + '/starts.jsonl', JSON.stringify({pane:process.env.HERDR_PANE_ID, session, pid:process.pid})+'\\n');
console.log('FAKE_PI_READY ' + process.env.HERDR_PANE_ID);
process.stdin.setRawMode(true);
let input = '';
process.stdin.on('data', data => {
  input += data.toString();
  if (input.includes('/quit\\r') || input.includes('/quit\\n')) process.exit(0);
});
function report(state) {
  const args = ['pane',state === 'startup' ? 'report-agent-session' : 'report-agent',process.env.HERDR_PANE_ID,
    '--source','herdr:pi','--agent','pi','--seq',String(Date.now()),'--agent-session-path',session];
  args.push(...(state === 'startup' ? ['--session-start-source','startup'] : ['--state',state]));
  const result = cp.spawnSync(process.env.HERDR_BIN_PATH, args, {env:process.env});
  fs.appendFileSync(root + '/reports.jsonl', JSON.stringify({args, status:result.status, error:String(result.error), out:String(result.stdout), err:String(result.stderr)})+'\\n');
  if (result.status || result.error) { console.error(String(result.stderr), result.error); process.exit(2); }
}
setTimeout(() => report('startup'), 50);
setTimeout(() => report('working'), 150);
setTimeout(() => report(busy ? 'working' : 'idle'), 650);
''')
        wrapper = self.bin / 'pi'
        wrapper.write_text(f'#!{BASH}\nexec -a pi {shlex.quote(NODE)} {shlex.quote(str(fake))} "$@"\n')
        wrapper.chmod(0o700)
        self.server = subprocess.Popen([HERDR, 'server'], env=self.env, cwd=self.base,
                                       stdin=subprocess.DEVNULL, stdout=self.server_log, stderr=self.server_log)
        deadline = time.monotonic() + 15
        while self.cli('status', 'server', check=False).returncode:
            self.assertLess(time.monotonic(), deadline, 'isolated server did not start')
            time.sleep(.1)

    def cli(self, *args, check=True):
        self.assertTrue(self.env['HERDR_SOCKET_PATH'].startswith(str(self.base)))
        result = subprocess.run([HERDR, *args], env=self.env, capture_output=True, text=True, timeout=15)
        if check: self.assertEqual(result.returncode, 0, (args, result.stdout, result.stderr))
        return result

    def api(self, method, params=None):
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(5); connection.connect(self.socket)
            connection.sendall((json.dumps({'id': 'isolated', 'method': method, 'params': params or {}}) + '\n').encode())
            result = json.loads(connection.makefile('rb').readline())
        self.assertNotIn('error', result)
        return result['result']

    def pump(self, duration=.1):
        until = time.monotonic() + duration
        while time.monotonic() < until:
            if not select.select([self.master], [], [], max(0, until - time.monotonic()))[0]: continue
            try: data = os.read(self.master, 65536)
            except OSError: break
            if not data: break
            self.raw.write(data); self.raw.flush(); self.screen.feed(data)
            if b'\x1b[6n' in data: os.write(self.master, b'\x1b[1;1R')
            if b'\x1b[c' in data: os.write(self.master, b'\x1b[?1;2c')
            text = self.screen.text()
            for line in text.splitlines():
                if "DON'T TYPE" in line: self.observed.add(line.strip())

    def wait_screen(self, text, timeout=45):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.pump()
            if text in self.screen.text(): return
        (self.base / 'failure.txt').write_text(self.screen.text())
        self.fail(f'Missing {text!r}; evidence {self.base}')

    def stop(self):
        if self.client_pid:
            try:
                os.kill(self.client_pid, signal.SIGTERM)
                until = time.monotonic() + 2
                while time.monotonic() < until:
                    if os.waitpid(self.client_pid, os.WNOHANG)[0]: break
                    self.pump(.1)
                else:
                    os.kill(self.client_pid, signal.SIGKILL)  # Only our PTY child, never the user's client.
                    os.waitpid(self.client_pid, 0)
            except (ProcessLookupError, ChildProcessError): pass
        if self.master is not None: os.close(self.master)
        if self.server:
            self.cli('server', 'stop', check=False)
            try: self.server.wait(10)
            except subprocess.TimeoutExpired: self.server.kill(); self.server.wait()
        self.server_log.close(); self.raw.close()

    def test_popup_reset_progress_focus_and_restore(self):
        created = json.loads(self.cli('workspace', 'create', '--label', 'ORIGIN', '--cwd', str(self.base), '--focus').stdout)
        origin = created['result']['root_pane']['pane_id']
        self.cli('pane', 'run', origin, "printf 'ORIGINAL_PANE_CONTENT\\n'")
        targets = []
        for label in ('TARGET_A', 'TARGET_B', 'BUSY'):
            created = json.loads(self.cli('workspace', 'create', '--label', label, '--cwd', str(self.base), '--no-focus').stdout)
            pane = created['result']['root_pane']['pane_id']; targets.append(pane)
            session = self.base / (label + " 'session'.jsonl"); session.write_text('{}\n')
            self.cli('pane', 'run', pane, 'pi --session ' + shlex.quote(str(session)) + (' --busy' if label == 'BUSY' else ''))
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            agents = json.loads(self.cli('agent', 'list').stdout)['result']['agents']
            if len(agents) == 3 and all(a.get('agent_session') for a in agents) and sum(a['agent_status'] in ('idle', 'done') for a in agents) == 2: break
            time.sleep(.1)
        else:
            (self.base / 'agents.json').write_text(json.dumps(agents, indent=2))
            for target in targets:
                (self.base / (target.replace(':', '-') + '.txt')).write_text(self.cli('pane', 'read', target, '--source', 'recent', '--lines', '30').stdout)
            self.fail('Fake agents did not report readiness')
        self.client_pid, self.master = pty.fork()
        if self.client_pid == 0:
            os.chdir(self.base); os.execve(HERDR, [HERDR], self.env)
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack('HHHH', 50, 160, 0, 0))
        self.pump(2); os.write(self.master, b'\x1b[I')
        for target in targets[:2]:
            self.api('pane.focus', {'pane_id': target}); self.pump(1)
        self.api('pane.focus', {'pane_id': origin}); self.pump(1)
        self.assertIn('TARGET_A · idle', self.screen.text())
        self.assertIn('TARGET_B · idle', self.screen.text())
        # Copy the manifest so RELOADER_BINARY can target a debug/release/installed build.
        plugin = self.base / 'plugin'; plugin.mkdir()
        manifest = (ROOT / 'herdr-plugin.toml').read_text().replace('./target/release/herdr-pi-reloader', str(BINARY))
        (plugin / 'herdr-plugin.toml').write_text(manifest)
        self.cli('plugin', 'link', str(plugin))
        self.cli('plugin', 'action', 'invoke', 'herdr-pi-reloader.pi-reloader.open')
        self.wait_screen('Reset all Pi')
        os.write(self.master, b'j\r')  # Only the isolated popup receives these keys.
        self.wait_screen('Reset: 2')
        self.pump(1.2)
        text = self.screen.text(); (self.base / 'final.txt').write_text(text)
        (self.base / 'progress.txt').write_text('\n'.join(sorted(self.observed)))
        self.assertIn('Visited: 2', text); self.assertIn('Failed: 0', text)
        self.assertIn('Skipped (busy): 1', text)
        self.assertIn('ORIGINAL_PANE_CONTENT', text)
        self.assertIn('TARGET_A · idle', text); self.assertIn('TARGET_B · idle', text)
        self.assertTrue(any('1/2' in line for line in self.observed), self.observed)
        self.assertNotIn("DON'T TYPE", text)
        starts = [json.loads(line) for line in (self.base / 'starts.jsonl').read_text().splitlines()]
        for target in targets[:2]:
            runs = [item for item in starts if item['pane'] == target]
            self.assertEqual(len(runs), 2); self.assertEqual(runs[0]['session'], runs[1]['session'])
            self.assertNotEqual(runs[0]['pid'], runs[1]['pid'])
        self.assertEqual(sum(item['pane'] == targets[2] for item in starts), 1)
        self.assertFalse(Path(self.socket + '.pi-reloader-status').exists())


if __name__ == '__main__': unittest.main()
