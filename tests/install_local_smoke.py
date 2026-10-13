#!/usr/bin/env python3
"""Isolated installer tests. Usage: python3 tests/install_local_smoke.py SCRATCH_ROOT.

All executable/session tools are mocks. The fixture copies the installer and redirects
only its fixed stock-config source to a fixture directory; no real HOME or i3 is used.
Run as an ordinary user, just like the production installer.
"""

import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]

APP = r'''
import json, os, pathlib, sys
version = VERSION
base = pathlib.Path(os.environ["MOCK_ROOT"])
command = sys.argv[1]
with (base / "events").open("a") as f:
    f.write(json.dumps([version, command]) + "\n")
state = base / "running"
if command == "status":
    print("IDLE" if state.exists() else "OFF")
elif command == "quit":
    state.unlink(missing_ok=True)
elif command == "show":
    if version == "new" and os.environ.get("FAIL_START"):
        sys.exit(1)
    state.write_text(version)
'''

TOOL = r'''
import json, os, pathlib, shutil, sys, time
base = pathlib.Path(os.environ["MOCK_ROOT"])
tool = pathlib.Path(sys.argv[0]).name
with (base / "events").open("a") as f:
    f.write(json.dumps([tool] + sys.argv[1:]) + "\n")
if tool == "cargo":
    if os.environ.get("FAIL_BUILD"):
        sys.exit(1)
    target = pathlib.Path(sys.argv[sys.argv.index("--target-dir") + 1]) / "debug"
    target.mkdir(parents=True, exist_ok=True)
    shutil.copy2(base / "new-app", target / "speaches-ai")
elif tool == "desktop-file-validate":
    text = pathlib.Path(sys.argv[1]).read_text()
    assert "Exec=\"/" in text and "@EXEC@" not in text
    if os.environ.get("FAIL_VALIDATE"):
        sys.exit(1)
elif tool == "update-desktop-database":
    if os.environ.get("FAIL_CACHE"):
        sys.exit(1)
elif tool == "sleep":
    time.sleep(0.005)
'''


def executable(path, body):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!{sys.executable}\n" + body)
    path.chmod(0o755)


class InstallerSmoke(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="installer-", dir=SCRATCH)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / 'home with spaces %name'
        self.home.mkdir()
        self.repo = self.root / "repo"
        self.script = self.repo / "scripts/install-local.sh"
        self.script.parent.mkdir(parents=True)
        self.stock = self.root / "stock"
        self.stock.mkdir()
        (self.stock / "10_stock").write_text("stock block\n")
        script = (REPO / "scripts/install-local.sh").read_text()
        self.script.write_text(script.replace("stock_dir=/usr/share/i3xrocks/conf.d", f'stock_dir="{self.stock}"'))
        for relative in ("integration/i3xrocks/80_speaches", "integration/desktop/speaches-ai.desktop"):
            destination = self.repo / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(REPO / relative, destination)
        self.tools = self.root / "tools"
        for tool in ("cargo", "i3-msg", "desktop-file-validate", "update-desktop-database", "sleep"):
            executable(self.tools / tool, TOOL)
        executable(self.root / "new-app", APP.replace("VERSION", repr("new")))
        self.env = {
            **os.environ,
            "HOME": str(self.home),
            "XDG_DATA_HOME": str(self.home / "data space"),
            "XDG_STATE_HOME": str(self.home / "state"),
            "XDG_CONFIG_HOME": str(self.home / "settings"),
            "XDG_RUNTIME_DIR": str(self.root / "runtime"),
            "DISPLAY": ":mock",
            "MOCK_ROOT": str(self.root),
            "PATH": str(self.tools) + os.pathsep + os.environ["PATH"],
        }
        Path(self.env["XDG_RUNTIME_DIR"]).mkdir()
        self.binary = self.home / ".local/bin/speaches-ai"
        self.bar = self.home / ".config/regolith3/i3xrocks/conf.d"
        self.block = self.bar / "80_speaches"
        self.desktop = Path(self.env["XDG_DATA_HOME"]) / "applications/speaches-ai.desktop"
        self.state = Path(self.env["XDG_STATE_HOME"]) / "speaches-ai"
        self.settings = Path(self.env["XDG_CONFIG_HOME"]) / "speaches-ai/settings.json"
        self.settings.parent.mkdir(parents=True)
        self.settings.write_text('{"do_not_replace": true}\n')

    def run_install(self, *, yes=True, answer=None, **extra):
        return subprocess.run(
            ["bash", str(self.script)] + (["--yes"] if yes else []),
            env={**self.env, **extra}, input=answer, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20,
        )

    def events(self):
        path = self.root / "events"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def seed_update(self):
        executable(self.binary, APP.replace("VERSION", repr("old")))
        self.bar.mkdir(parents=True)
        self.block.write_text("previous owned block\n")
        (self.bar / "20_custom").write_text("user custom block\n")
        self.desktop.parent.mkdir(parents=True)
        self.desktop.write_text("previous desktop\n")
        (self.root / "running").write_text("old")
        return [p.read_bytes() for p in (self.binary, self.block, self.desktop)]

    def assert_settings(self):
        self.assertEqual(self.settings.read_text(), '{"do_not_replace": true}\n')

    def test_first_install_and_repeat(self):
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Install complete", result.stdout)
        self.assertEqual((self.bar / "10_stock").read_text(), "stock block\n")
        self.assertEqual(self.block.read_bytes(), (REPO / "integration/i3xrocks/80_speaches").read_bytes())
        self.assertEqual((self.root / "running").read_text(), "new")
        text = self.desktop.read_text()
        self.assertIn('Exec="' + str(self.binary).replace("%", "%%") + '" show\n', text)
        for line in ("Name=Speaches AI", "Icon=audio-input-microphone", "Terminal=false", "Type=Application", "Categories=AudioVideo;Audio;Utility;", "StartupWMClass=speaches-ai"):
            self.assertIn(line + "\n", text)
        validator = shutil.which("desktop-file-validate")
        if validator:
            validated = subprocess.run([validator, str(self.desktop)], capture_output=True, text=True)
            self.assertEqual(validated.returncode, 0, validated.stdout + validated.stderr)
        self.assertEqual(stat.S_IMODE((self.state / "application.log").stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(self.state.stat().st_mode), 0o700)
        self.assert_settings()
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Update complete", result.stdout)
        self.assertEqual(sorted(p.name for p in self.bar.iterdir()), ["10_stock", "80_speaches"])
        self.assertEqual([p.name for p in self.desktop.parent.iterdir()], ["speaches-ai.desktop"])
        self.assertEqual(sorted(p.name for p in self.state.iterdir()), ["application.log", "rollback-latest"])
        self.assertEqual(sorted(p.name for p in (self.state / "rollback-latest").iterdir()), ["80_speaches", "binary", "speaches-ai.desktop"])

    def test_update_preserves_settings_and_backs_up_before_quit(self):
        previous = self.seed_update()
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.bar / "20_custom").read_text(), "user custom block\n")
        self.assert_settings()
        self.assertEqual([p.read_bytes() for p in [self.state / "rollback-latest" / name for name in ("binary", "80_speaches", "speaches-ai.desktop")]], previous)
        events = self.events()
        quit_index = events.index(["old", "quit"])
        self.assertLess(next(i for i, e in enumerate(events) if e[0] == "cargo"), quit_index)
        self.assertLess(next(i for i, e in enumerate(events) if e[0] == "desktop-file-validate"), quit_index)
        self.assertLess(events.index(["new", "show"]), events.index(["i3-msg", "restart"]))

    def test_failed_start_rolls_back_and_relaunches_previous(self):
        previous = self.seed_update()
        result = self.run_install(FAIL_START="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("restoring the previous", result.stderr)
        self.assertEqual([p.read_bytes() for p in (self.binary, self.block, self.desktop)], previous)
        self.assertEqual((self.root / "running").read_text(), "old")
        self.assertIn(["old", "show"], self.events())
        self.assertNotIn(["i3-msg", "restart"], self.events())
        self.assertEqual((self.bar / "20_custom").read_text(), "user custom block\n")
        self.assert_settings()

    def test_decline(self):
        previous = self.seed_update()
        result = self.run_install(yes=False, answer="n\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Cancelled", result.stdout)
        self.assertFalse(any(e[0] == "cargo" or e[-1] == "quit" for e in self.events()))
        self.assertEqual([p.read_bytes() for p in (self.binary, self.block, self.desktop)], previous)
        self.assertFalse(self.state.exists())

    def test_build_failure_leaves_running_install_untouched(self):
        previous = self.seed_update()
        result = self.run_install(FAIL_BUILD="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(e[-1] in ("quit", "restart") for e in self.events()))
        self.assertEqual([p.read_bytes() for p in (self.binary, self.block, self.desktop)], previous)
        self.assertEqual((self.root / "running").read_text(), "old")
        self.assertFalse(self.state.exists())

    def test_validation_failure_does_not_stop_app(self):
        previous = self.seed_update()
        result = self.run_install(FAIL_VALIDATE="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(e[-1] in ("quit", "restart") for e in self.events()))
        self.assertEqual([p.read_bytes() for p in (self.binary, self.block, self.desktop)], previous)
        self.assertEqual(sorted(p.name for p in self.desktop.parent.iterdir()), ["speaches-ai.desktop"])

    def test_desktop_exec_special_characters(self):
        old_home = self.home
        self.home = self.root / 'home % with "quote" $dollar `tick` \\slash'
        old_home.rename(self.home)
        for key in ("HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CONFIG_HOME"):
            self.env[key] = self.env[key].replace(str(old_home), str(self.home))
        for attr in ("binary", "bar", "block", "desktop", "state", "settings"):
            setattr(self, attr, Path(str(getattr(self, attr)).replace(str(old_home), str(self.home))))
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        escaped = ""
        for character in str(self.binary):
            if character == "%":
                escaped += "%%"
            elif character == "\\":
                escaped += "\\" * 4
            elif character in '\"$`':
                escaped += "\\" * 2 + character
            else:
                escaped += character
        self.assertIn('Exec="' + escaped + '" show\n', self.desktop.read_text())
        validator = shutil.which("desktop-file-validate")
        if validator:
            validated = subprocess.run([validator, str(self.desktop)], capture_output=True, text=True)
            self.assertEqual(validated.returncode, 0, validated.stdout + validated.stderr)
        self.assert_settings()

    def test_default_xdg_data_directory(self):
        self.env.pop("XDG_DATA_HOME")
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.home / ".local/share/applications/speaches-ai.desktop").is_file())

    def test_cache_failure_is_nonfatal(self):
        result = self.run_install(FAIL_CACHE="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Warning: desktop cache", result.stderr)
        self.assertIn(["i3-msg", "restart"], self.events())

    def test_failed_first_start_removes_owned_files(self):
        result = self.run_install(FAIL_START="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.binary.exists())
        self.assertFalse(self.block.exists())
        self.assertFalse(self.desktop.exists())
        self.assertNotIn(["i3-msg", "restart"], self.events())
        self.assert_settings()


if __name__ == "__main__":
    if len(sys.argv) != 2 or not Path(sys.argv[1]).is_dir():
        sys.exit("Usage: python3 tests/install_local_smoke.py EXISTING_SCRATCH_ROOT")
    if os.geteuid() == 0:
        sys.exit("Run installer smoke tests as an ordinary user, not root.")
    SCRATCH = Path(sys.argv.pop()).resolve()
    unittest.main(verbosity=2)
