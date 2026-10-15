"""Isolated real-window appearance and workspace smoke test; never captures audio.
Usage: xvfb-run -a python tests/theme_smoke.py /absolute/artifact-directory
Requires a built app, xdotool, and ImageMagick import/convert.
Coordinate fallbacks are checked against the 760x760 render. Keyboard commands,
radio Tab/Space activation, saved data, and rendered surface colors verify outcomes.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

base = Path(sys.argv[1]).resolve()
base.mkdir(parents=True, exist_ok=True)
for name in ("runtime", "config/speaches-ai"):
    (base / name).mkdir(parents=True, exist_ok=True, mode=0o700)
fd = os.open(base / "runtime", os.O_RDONLY | os.O_DIRECTORY)
env = dict(os.environ, XDG_RUNTIME_DIR=f"/proc/{os.getpid()}/fd/{fd}",
           XDG_CONFIG_HOME=str(base / "config"))
binary = str(Path("target/debug/speaches-ai").resolve())
settings_path = base / "config/speaches-ai/settings.json"
settings_path.write_text(json.dumps({"theme": "light", "base_url": "http://127.0.0.1:9"}))
process = None


def command(arg):
    return subprocess.check_output([binary, arg], env=env, text=True, timeout=3).strip()


def windows(pattern="^Speaches AI$"):
    result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", pattern],
                            env=env, capture_output=True, text=True)
    return result.stdout.split()


def wait(predicate, label):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AssertionError(f"App exited {process.returncode} waiting for {label}")
        if predicate():
            return
        time.sleep(0.1)
    raise AssertionError(f"Timed out: {label}")


def saved():
    return json.loads(settings_path.read_text())


def key(value):
    subprocess.run(["xdotool", "key", "--clearmodifiers", value], env=env, check=True)
    time.sleep(0.2)


def focus(window):
    subprocess.run(["xdotool", "windowfocus", "--sync", window], env=env, check=True)
    time.sleep(0.2)


def click(window, x, y):
    subprocess.run(["xdotool", "mousemove", "--window", window, str(x), str(y), "click", "1"],
                   env=env, check=True)
    time.sleep(0.2)


def capture(window, name):
    path = base / name
    subprocess.run(["import", "-window", window, str(path)], env=env, check=True)
    return path


def surface(window):
    path = capture(window, "surface.png")
    pixel = subprocess.check_output(["convert", str(path), "-crop", "1x1+2+2", "-depth", "8", "rgb:-"])
    return sum(pixel) / len(pixel)


def start(log):
    global process
    process = subprocess.Popen([binary], env=env, stdout=log, stderr=log)
    wait(windows, "main window")
    window = windows()[0]
    focus(window)
    # Audio device enumeration and the first software-rendered frame can be slow.
    time.sleep(2)
    return window


try:
    with (base / "application.log").open("w") as log:
        window = start(log)
        wait(lambda: surface(window) > 200, "saved Light appearance")
        capture(window, "dictation-light.png")
        key("ctrl+comma")
        capture(window, "settings-light.png")
        click(window, 180, 409)  # Backend URL, never the recording button.
        key("ctrl+a")
        subprocess.run(["xdotool", "type", "--clearmodifiers", "http://draft.example.invalid"],
                       env=env, check=True)
        click(window, 207, 235)  # Dark radio.
        wait(lambda: saved().get("theme") == "dark", "immediate theme persistence")
        assert saved()["base_url"] == "http://127.0.0.1:9", "Theme saved unrelated draft"
        assert "api_key" not in saved()
        wait(lambda: surface(window) < 40, "Dark rendered surface")
        # The resident system-appearance synchronization must not undo an override.
        time.sleep(0.5)
        assert surface(window) < 40
        capture(window, "settings-dark.png")
        key("ctrl+s")
        wait(lambda: saved()["base_url"] == "http://draft.example.invalid", "explicit draft save")
        assert saved()["theme"] == "dark", "Form overwrote newer immediate theme"
        key("ctrl+1")
        capture(window, "dictation-dark.png")
        key("ctrl+q")
        wait(lambda: not windows(), "hide")
        command("preferences")
        wait(lambda: windows("Quick preferences"), "quick preferences only")
        quick = windows("Quick preferences")[0]
        focus(quick)
        wait(lambda: surface(quick) < 40, "quick preferences inherits theme")
        capture(quick, "quick-dark.png")
        assert not windows(), "Quick preferences opened the main window"
        key("Escape")
        wait(lambda: not windows("Quick preferences"), "quick dismissal")
        command("show")
        wait(windows, "recreated main window")
        window = windows()[0]
        focus(window)
        wait(lambda: surface(window) < 40, "recreated main inherits theme")
        command("quit")
        process.wait(timeout=5)

        window = start(log)
        wait(lambda: surface(window) < 40, "Dark survives process restart")
        key("ctrl+2")
        settings_path.parent.chmod(0o500)
        try:
            click(window, 138, 235)  # Light cannot be saved.
            time.sleep(0.4)
            assert saved()["theme"] == "dark", "Failed theme save changed persisted preference"
            assert surface(window) < 40, "Failed theme save changed appearance"
            capture(window, "theme-save-failed.png")
        finally:
            settings_path.parent.chmod(0o700)
        click(window, 55, 235)  # System radio.
        wait(lambda: saved()["theme"] == "system", "System selection persists")
        # The pinned library exposes individual radios as Tab stops. Exercise
        # their real Space activation, rather than assuming arrow navigation.
        click(window, 180, 409)  # Backend URL follows the three theme radios.
        key("shift+Tab")  # Dark.
        key("shift+Tab")  # Light.
        capture(window, "keyboard-light-focus.png")
        key("space")
        wait(lambda: saved()["theme"] == "light", "keyboard Light selection")
        wait(lambda: surface(window) > 200, "Light after failure recovery")
        key("Tab")
        key("space")
        wait(lambda: saved()["theme"] == "dark", "keyboard Dark selection")
        wait(lambda: surface(window) < 40, "keyboard Dark appearance")
        click(window, 180, 409)
        key("ctrl+a")
        subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "30", "http://session-only.invalid"], env=env, check=True)
        time.sleep(1)
        settings_path.parent.chmod(0o500)
        try:
            key("ctrl+s")
            time.sleep(1)
            assert saved()["base_url"] == "http://draft.example.invalid"
            capture(window, "settings-save-failed.png")
        finally:
            settings_path.parent.chmod(0o700)
        # A session-only save must keep its warning and allow persistence retry
        # even though the applied form now matches the in-memory settings.
        click(window, 660, 691)
        wait(lambda: saved()["base_url"] == "http://session-only.invalid", "settings save retry")
        subprocess.run(["xdotool", "windowsize", window, "540", "540"], env=env, check=True)
        time.sleep(0.3)
        capture(window, "settings-minimum.png")
        subprocess.run(["xdotool", "mousemove", "--window", window, "500", "350", "click",
                        "--repeat", "40", "--delay", "10", "5"], env=env, check=True)
        time.sleep(0.3)
        capture(window, "settings-minimum-bottom.png")
        key("ctrl+1")
        capture(window, "dictation-minimum.png")
        subprocess.run(["xdotool", "windowsize", window, "1100", "900"], env=env, check=True)
        subprocess.run(["xdotool", "windowmove", window, "0", "0"], env=env, check=True)
        key("ctrl+2")
        subprocess.run(["xdotool", "mousemove", "--window", window, "1000", "350", "click",
                        "--repeat", "40", "--delay", "10", "4"], env=env, check=True)
        time.sleep(0.3)
        capture(window, "settings-wide.png")
        command("quit")
        process.wait(timeout=5)
        assert process.returncode == 0
        print("PASS: workspace shortcuts, immediate themes, draft isolation, explicit save, overrides, "
              "hidden/recreated/quick windows, restart persistence, failed-save rollback/recovery, "
              "keyboard radio navigation, minimum/wide screenshots")
finally:
    settings_path.parent.chmod(0o700)
    if process is not None and process.poll() is None:
        process.terminate()
        process.wait(timeout=5)
    os.close(fd)
