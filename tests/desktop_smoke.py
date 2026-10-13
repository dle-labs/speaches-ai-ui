"""Optional isolated X11 smoke test. Run under xvfb-run; never captures audio.

Usage: xvfb-run -a python tests/desktop_smoke.py /absolute/test-artifact-directory
Requires a built target/debug/speaches-ai, Python, Xvfb, and xdotool.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

base = Path(sys.argv[1]).resolve()
base.mkdir(parents=True, exist_ok=True)
for name in ("runtime", "config"):
    (base / name).mkdir(exist_ok=True, mode=0o700)
# Keep the Unix socket path short even if the artifact directory has a long name.
runtime_fd = os.open(base / "runtime", os.O_RDONLY | os.O_DIRECTORY)
env = dict(os.environ, XDG_RUNTIME_DIR=f"/proc/{os.getpid()}/fd/{runtime_fd}",
           XDG_CONFIG_HOME=str(base / "config"))
binary = str(Path("target/debug/speaches-ai").resolve())


def command(arg):
    return subprocess.check_output([binary, arg], env=env, text=True, timeout=3).strip()


def windows():
    result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "^Speaches AI$"],
                            env=env, capture_output=True, text=True, check=False)
    return result.stdout.split()


def wait_for(process, predicate, label):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if predicate():
            return
        if process.poll() is not None:
            raise RuntimeError(f"App exited {process.returncode} while waiting for {label}")
        time.sleep(0.1)
    raise RuntimeError(f"Timed out: {label}")


def ctrl_q():
    subprocess.run(["xdotool", "windowfocus", "--sync", windows()[0]], env=env, check=True)
    subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+q"], env=env, check=True)


try:
    for quit_on_ctrl_q in (False, True):
        settings_dir = base / "config" / "speaches-ai"
        settings_dir.mkdir(exist_ok=True)
        (settings_dir / "settings.json").write_text(json.dumps({"quit_on_ctrl_q": quit_on_ctrl_q}))
        with (base / f"app-{quit_on_ctrl_q}.log").open("w") as log:
            process = subprocess.Popen([binary], env=env, stdout=log, stderr=log)
            try:
                wait_for(process, windows, "window")
                assert command("status") == "MIC"
                assert '\uf130' in command("bar") and 'foreground=' in command("bar")
                ctrl_q()
                if not quit_on_ctrl_q:
                    wait_for(process, lambda: not windows(), "hide")
                    assert command("status") == "MIC"
                    focus = subprocess.check_output(["xdotool", "getwindowfocus"], env=env, text=True)
                    subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+shift+x"], env=env, check=True)
                    time.sleep(0.2)
                    assert command("status") == "MIC"
                    assert not windows(), "Global cancel must not reopen the app"
                    assert subprocess.check_output(["xdotool", "getwindowfocus"], env=env, text=True) == focus
                    command("show")
                    wait_for(process, windows, "reopen")
                    command("show")
                    assert len(windows()) == 1
                    assert command("quit") == "OK"
                process.wait(timeout=5)
                assert process.returncode == 0
                assert command("status") == "OFF"
                assert '\uf131' in command("bar")
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
    print("PASS: window, Ctrl+Q hide/quit settings, hidden status/cancel without focus change, reopen, single instance, quit, OFF")
finally:
    os.close(runtime_fd)
