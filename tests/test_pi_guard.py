"""Real Pi editor/API regression, opt-in; fresh HOME, no credentials/tools/network.

HERDR_RELOADER_PI_ISOLATED=1 python3 -m unittest discover -s tests -p test_pi_guard.py -v
Never attaches to Herdr or an existing Pi. Test-only observer reads our synthetic drafts.
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shutil
import socket
import struct
import subprocess
import tempfile
import termios
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
PI = shutil.which('pi')
OBSERVER = '''import {readFileSync, writeFileSync, appendFileSync} from 'node:fs';
export default function(pi) {
  let timer;
  pi.on('session_start', (_event, ctx) => {
    let applied = '';
    appendFileSync(process.env.PROBE_ROOT + '/events', 'start\\n');
    timer = setInterval(() => {
      try {
        const input = readFileSync(process.env.PROBE_ROOT + '/editor-input', 'utf8');
        if (input !== applied) { ctx.ui.setEditorText(JSON.parse(input).text); applied = input; }
        writeFileSync(process.env.PROBE_ROOT + '/editor-state', JSON.stringify({
          text: ctx.ui.getEditorText(), session: ctx.sessionManager.getSessionFile() ?? null,
        }));
      } catch {}
    }, 20);
  });
  pi.on('session_shutdown', () => { clearInterval(timer); });
  pi.on('input', () => { appendFileSync(process.env.PROBE_ROOT + '/events', 'INPUT\\n'); return {action:'handled'}; });
  pi.on('before_agent_start', () => { throw Error('Model requests forbidden in this test'); });
}
'''


@unittest.skipUnless(shutil.which('node'), 'Node 22.18+ required for guard unit check')
class GuardUnitTest(unittest.TestCase):
    def test_guard_rechecks_drafts_busy_queue_and_identity_before_native_action(self):
        script = r'''
const assert = require('node:assert/strict'), net = require('node:net');
const factory = require(process.argv[2]).default;
process.env.HERDR_SOCKET_PATH = '/isolated-unit-' + process.pid;
process.env.HERDR_PANE_ID = 'w1:p1';
const handlers = {}, commands = {};
let draft = '', busy = false, queued = false, lateDraft = false, lateBusy = false, available = true, shutdowns = 0;
const ctx = {mode:'tui', isIdle:()=>!busy, hasPendingMessages:()=>queued,
  sessionManager:{getSessionFile:()=>'/unit-session.jsonl'},
  ui:{getEditorText:()=>draft, notify:message=>{throw Error(message)}},
  shutdown:()=>{shutdowns++}, reload:async()=>{throw Error('Unexpected reload')},
};
factory({on:(name, fn)=>{handlers[name]=fn}, registerCommand:(name, c)=>{commands[name]=c},
  getCommands:()=>available ? Object.keys(commands).map(name=>({name, source:'extension'})) : [],
  sendUserMessage:(text, options)=>{
    assert.equal(options.expandPromptTemplates, true);
    if (lateDraft) draft = 'typed after the first check';
    if (lateBusy) busy = true;
    const [name, token] = text.slice(1).split(' ');
    commands[name].handler(token, ctx);
  },
});
function request(action, generation, overrides={}) {
  return new Promise((resolve, reject)=>{
    const client = net.connect(`/tmp/herdr-pi-reloader-${process.getuid()}/${process.pid}.sock`);
    let data='';
    client.on('error', reject); client.setTimeout(4000, ()=>client.destroy(Error('timeout')));
    client.on('data', chunk=>{data+=chunk});
    client.on('end', ()=>{try {resolve(JSON.parse(data))} catch (e) {reject(e)}});
    client.on('connect', ()=>client.end(JSON.stringify({protocol:1, pid:process.pid,
      pane_id:process.env.HERDR_PANE_ID, server:process.env.HERDR_SOCKET_PATH,
      session_path:'/unit-session.jsonl', action, generation, ...overrides})+'\n'));
  });
}
(async()=>{
  await handlers.session_start({}, ctx);
  try {
    const generation = (await request('probe')).generation;
    for (const action of ['reload', 'quit']) {
      for (const text of ['draft', 'line one\nline two', '  \n']) {
        draft=text; assert.equal((await request(action, generation)).status,'draft'); assert.equal(draft,text);
      }
      draft=''; busy=true; assert.equal((await request(action,generation)).status,'busy'); busy=false;
      queued=true; assert.equal((await request(action,generation)).status,'busy'); queued=false;
      lateDraft=true; assert.equal((await request(action,generation)).status,'draft'); lateDraft=false; draft='';
      lateBusy=true; assert.equal((await request(action,generation)).status,'busy'); lateBusy=false; busy=false;
      for (const override of [{protocol:2}, {pid:0}, {pane_id:'other'}, {server:'other'},
                              {session_path:'other'}, {generation:'old'}, {action:'unknown'}]) {
        assert.equal((await request(action,generation,override)).status,'error');
      }
      available=false; assert.equal((await request(action,generation)).status,'error'); available=true;
    }
    assert.equal(shutdowns,0);
    assert.deepEqual(handlers.input({text:'/herdr-pi-reloader-control fallback'}),{action:'handled'});
    assert.equal((await request('quit',generation)).status,'accepted');
    assert.equal(shutdowns,1);
    console.log('guard assertions passed');
  } finally {await handlers.session_shutdown()}
})().catch(error=>{console.error(error);process.exitCode=1});
'''
        result = subprocess.run([shutil.which('node'), '-', str(ROOT / 'extensions/herdr-pi-reloader.ts')],
                                input=script, text=True, capture_output=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('guard assertions passed', result.stdout)


@unittest.skipUnless(os.environ.get('HERDR_RELOADER_PI_ISOLATED') == '1' and PI, 'opt-in isolated real Pi')
class PiGuardTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(dir='/tmp', prefix='pir-real-pi-'))
        print(f'\nIsolated Pi evidence: {self.root}', flush=True)
        self.master, slave = pty.openpty()
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))
        self.output = bytearray()
        self.env = dict(HOME=str(self.root), PATH=os.environ['PATH'], TERM='xterm-256color',
                        PI_CODING_AGENT_DIR=str(self.root / '.pi/agent'), PI_OFFLINE='1', PI_TELEMETRY='0',
                        PI_SKIP_VERSION_CHECK='1', HERDR_SOCKET_PATH=str(self.root / 'isolated-herdr.sock'),
                        HERDR_PANE_ID='w1:p1', PROBE_ROOT=str(self.root))
        (self.root / 'observer.ts').write_text(OBSERVER)
        extension = self.root / '.pi/agent/extensions/herdr-pi-reloader.ts'
        extension.parent.mkdir(parents=True)
        extension.symlink_to(ROOT / 'extensions/herdr-pi-reloader.ts')
        (self.root / 'editor-input').write_text(json.dumps({'text': ''}))
        self.process = subprocess.Popen([
            PI, '--offline', '--no-session', '--no-tools', '--no-skills',
            '--no-prompt-templates', '--no-themes', '--no-context-files', '--no-approve',
            '-e', str(self.root / 'observer.ts'),
        ], cwd=self.root, env=self.env, stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
        os.close(slave)
        self.path = Path(f'/tmp/herdr-pi-reloader-{os.getuid()}/{self.process.pid}.sock')
        self.addCleanup(self.stop)
        self.wait(lambda: self.path.exists() and (self.root / 'editor-state').exists())
        self.assertEqual(self.request('probe')['status'], 'ready')

    def pump(self, duration=.05):
        until = time.monotonic() + duration
        while time.monotonic() < until:
            if not select.select([self.master], [], [], max(0, until - time.monotonic()))[0]: continue
            try: data = os.read(self.master, 65536)
            except OSError: break
            if not data: break
            self.output.extend(data)
            if b'\x1b[6n' in data: os.write(self.master, b'\x1b[1;1R')
            if b'\x1b[c' in data: os.write(self.master, b'\x1b[?1;2c')

    def wait(self, predicate, timeout=15):
        until = time.monotonic() + timeout
        while time.monotonic() < until:
            self.pump()
            # Snapshot first: exit between two polls must not become a false failure.
            exited = self.process.poll() is not None
            try:
                if predicate(): return
            except (FileNotFoundError, json.JSONDecodeError, ConnectionRefusedError): pass
            if exited: break
        self.fail(f'Isolated Pi condition timed out; {self.root}; output:\n{self.output[-5000:].decode(errors="replace")}')

    def state(self):
        return json.loads((self.root / 'editor-state').read_text())

    def request(self, action, **changes):
        request = dict(protocol=1, pid=self.process.pid, pane_id='w1:p1',
                       server=self.env['HERDR_SOCKET_PATH'], session_path=None, action=action)
        if action != 'probe': request['generation'] = self.request('probe')['generation']
        request.update(changes)
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(5); client.connect(str(self.path))
            client.sendall((json.dumps(request) + '\n').encode())
            return json.loads(client.makefile('rb').readline())

    def editor(self, text):
        (self.root / 'editor-input').write_text(json.dumps({'text': text, 'revision': time.monotonic()}))
        self.wait(lambda: self.state()['text'] == text)

    def test_drafts_cursor_whitespace_and_stale_identity_are_preserved(self):
        for text in ['unsent draft', 'first line\nsecond line 🍀', '  \n ']:
            self.editor(text)
            for action in ('reload', 'quit'):
                self.assertEqual(self.request(action)['status'], 'draft')
                self.assertEqual(self.state()['text'], text)
                self.assertIsNone(self.process.poll())
        self.editor('')
        pasted = 'unsent /quit\n/reload is still draft text\n🍀'
        os.write(self.master, b'\x1b[200~' + pasted.encode() + b'\x1b[201~')
        self.wait(lambda: self.state()['text'] == pasted)
        for action in ('reload', 'quit'):
            self.assertEqual(self.request(action)['status'], 'draft')
            self.assertEqual(self.state()['text'], pasted)
        self.editor('first line\nsecond line')
        os.write(self.master, b'\x1b[D'); self.pump(.15)
        self.assertEqual(self.request('quit')['status'], 'draft')
        os.write(self.master, b'X')
        self.wait(lambda: self.state()['text'] == 'first line\nsecond linXe')
        self.editor('')
        for changed in ({'session_path': '/wrong.jsonl'}, {'pid': self.process.pid + 1},
                        {'pane_id': 'w2:p1'}, {'server': '/not-this-server'}, {'generation': 'stale'}):
            self.assertEqual(self.request('quit', **changed)['status'], 'error')
            self.assertIsNone(self.process.poll())
        self.assertNotIn('INPUT', (self.root / 'events').read_text())

    def test_native_reload_and_shutdown_without_editor_submission(self):
        generation = self.request('probe')['generation']
        self.assertEqual(self.request('reload')['status'], 'accepted')
        self.wait(lambda: self.path.exists() and self.request('probe')['generation'] != generation)
        self.assertEqual(self.state()['text'], '')
        self.assertEqual(self.request('quit')['status'], 'accepted')
        self.wait(lambda: self.process.poll() is not None)
        self.assertEqual(self.process.returncode, 0)
        self.assertFalse(self.path.exists())
        self.assertEqual((self.root / 'events').read_text(), 'start\nstart\n')

    def stop(self):
        if self.process.poll() is None:
            self.process.terminate()
            try: self.process.wait(5)
            except subprocess.TimeoutExpired: self.process.kill(); self.process.wait(5)
        self.pump(.1)
        (self.root / 'terminal.ansi').write_bytes(self.output)
        os.close(self.master)
        # Only the socket for our terminated child; no user Pi endpoint is touched.
        self.path.unlink(missing_ok=True)


if __name__ == '__main__': unittest.main()
