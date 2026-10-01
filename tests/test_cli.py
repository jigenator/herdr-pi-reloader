"""Regression checks against a fake Herdr executable; never contact a live session.

Run: cargo build --locked && python3 -m unittest discover -s tests -v
Set RELOADER_BINARY to check a release/installed binary instead of target/debug.
"""

import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("RELOADER_BINARY", ROOT / "target/debug/herdr-pi-reloader")).resolve()
FAKE_HERDR = r'''
import json, os, sys
from pathlib import Path
root = Path(os.environ["FIXTURE_DIR"])
args = sys.argv[1:]
with (root / "calls.jsonl").open("a") as log:
    log.write(json.dumps(args) + "\n")
fixture = json.loads((root / "fixture.json").read_text())
if args == ["agent", "list"]:
    result = {"agents": fixture["agents"]}
elif args[:3] == ["pane", "process-info", "--pane"]:
    if fixture.get("process_error"):
        sys.exit("mock process-info failure")
    process = {"name": "zsh", "argv0": "zsh"} if (root / "quit").exists() else fixture["process"]
    result = {"process_info": {"foreground_processes": [process]}}
elif args[:2] == ["pane", "run"]:
    # Record commands, NEVER execute them or forward them to Herdr.
    if args[3] == "/quit":
        (root / "quit").touch()
    result = {"type": "ok"}
else:
    sys.exit("unexpected fake Herdr call: " + repr(args))
print(json.dumps({"result": result}))
'''


class CliTests(unittest.TestCase):
    def run_cli(self, command, process, status="idle", session=True, process_error=False):
        with tempfile.TemporaryDirectory(prefix="pi-reloader-test-") as directory:
            root = Path(directory)
            session_path = str(root / "session with 'quotes' and $(no-evaluation).jsonl")
            agent = {"agent": "pi", "agent_status": status, "pane_id": "w1:p1"}
            if session:
                agent["agent_session"] = {"agent": "pi", "kind": "path", "value": session_path}
            (root / "fixture.json").write_text(json.dumps({
                "agents": [agent], "process": process, "process_error": process_error,
            }))
            herdr = root / "fake-herdr"
            herdr.write_text(f"#!{sys.executable}\n" + FAKE_HERDR)
            herdr.chmod(0o700)
            result = subprocess.run(
                [str(BINARY), command],
                env={"HOME": directory, "PATH": os.defpath, "HERDR_BIN_PATH": str(herdr), "FIXTURE_DIR": directory},
                cwd=directory, capture_output=True, text=True, timeout=10,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            calls = [json.loads(line) for line in (root / "calls.jsonl").read_text().splitlines()]
            return result.stdout, calls, session_path

    def test_current_and_legacy_pi_identity(self):
        for process in (
            {"name": "node", "argv0": "pi"},
            {"name": "node", "argv0": "/opt/bin/pi"},
            {"name": "node", "argv": ["/opt/bin/pi", "--session", "file"]},
            {"name": "node", "argv0": None, "argv": ["pi"]},
            {"name": "pi"},
        ):
            with self.subTest(process=process):
                output, calls, _ = self.run_cli("reload", process)
                self.assertIn("reloaded: 1", output)
                self.assertEqual([c for c in calls if c[:2] == ["pane", "run"]], [["pane", "run", "w1:p1", "/reload"]])

    def test_reset_checks_exit_before_resuming_exact_session(self):
        output, calls, session = self.run_cli("reset", {"name": "node", "argv0": "pi"})
        self.assertIn("reset: 1", output)
        quit_index = calls.index(["pane", "run", "w1:p1", "/quit"])
        self.assertEqual(calls[quit_index + 1], ["pane", "process-info", "--pane", "w1:p1"])
        self.assertEqual(len([c for c in calls if c[:2] == ["pane", "run"]]), 2)
        self.assertEqual(shlex.split(calls[-1][3]), ["pi", "--session", session])

    def test_non_pi_processes_are_skipped_and_counted(self):
        for process in (
            {"name": "node", "argv0": "node"},
            {"name": "node", "argv0": "not-pi"},
            {"name": "node", "argv": ["node", "pi"]},
            {"name": "node", "argv0": None},
            {},
        ):
            for command in ("reload", "reset"):
                with self.subTest(process=process, command=command):
                    output, calls, _ = self.run_cli(command, process)
                    self.assertIn("skipped_non_pi: 1", output)
                    self.assertFalse(any(c[:2] == ["pane", "run"] for c in calls))

    def test_working_agents_and_missing_reset_sessions_are_untouched(self):
        for command in ("reload", "reset"):
            output, calls, _ = self.run_cli(command, {"name": "node", "argv0": "pi"}, status="working")
            self.assertIn("skipped_unsafe_status: 1", output)
            self.assertFalse(any(c[:2] == ["pane", "run"] for c in calls))
        output, calls, _ = self.run_cli("reset", {"name": "node", "argv0": "pi"}, session=False)
        self.assertIn("failed: 1", output)
        self.assertFalse(any(c[:2] == ["pane", "run"] for c in calls))

    def test_process_inspection_failure_never_runs_commands(self):
        for command in ("reload", "reset"):
            output, calls, _ = self.run_cli(command, {}, process_error=True)
            self.assertIn("failed: 1", output)
            self.assertIn("mock process-info failure", output)
            self.assertFalse(any(c[:2] == ["pane", "run"] for c in calls))


if __name__ == "__main__":
    unittest.main()
