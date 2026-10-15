"""Isolated real-GPUI preference controls; mock HTTP only, no audio recording.
Usage: xvfb-run -a python tests/preferences_smoke.py /absolute/artifact-directory
Requires xdotool, xprop, ImageMagick import; no desktop configuration is changed.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import json
import os
import subprocess
import sys
import threading
import time

base = Path(sys.argv[1]).resolve()
base.mkdir(parents=True, exist_ok=True)
for name in ("runtime", "config/speaches-ai"):
    (base / name).mkdir(parents=True, exist_ok=True, mode=0o700)
fd = os.open(base / "runtime", os.O_RDONLY | os.O_DIRECTORY)
env = dict(os.environ, XDG_RUNTIME_DIR=f"/proc/{os.getpid()}/fd/{fd}", XDG_CONFIG_HOME=str(base / "config"))
binary = str(Path("target/debug/speaches-ai").resolve())


class Backend(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({"data": [{"id": name, "task": "automatic-speech-recognition"}
                                   for name in ("test/model-one", "test/model-two")]}).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        raise AssertionError("No audio uploads are allowed")

    def log_message(self, *args):
        pass


server = ThreadingHTTPServer(("127.0.0.1", 0), Backend)
threading.Thread(target=server.serve_forever, daemon=True).start()
settings_path = base / "config/speaches-ai/settings.json"
settings_path.write_text(json.dumps({"base_url": f"http://127.0.0.1:{server.server_port}",
                                     "model": "test/model-one", "copy": False,
                                     "quick_languages": ["", "en", "de"]}))


def command(*args):
    return subprocess.check_output([binary, *args], env=env, text=True, timeout=3).strip()


def selection():
    return json.loads(command("selection"))


def windows(pattern):
    result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", pattern],
                            env=env, text=True, capture_output=True)
    return result.stdout.split()


def wait(predicate, label):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AssertionError(f"App exited {process.returncode} waiting for {label}")
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError(f"Timed out: {label}")


def key(value):
    subprocess.run(["xdotool", "key", "--clearmodifiers", value], env=env, check=True)
    time.sleep(0.15)


def type_text(value):
    subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "30", value], env=env, check=True)
    time.sleep(0.3)


def capture(name):
    ids = windows("Quick preferences")
    if ids:
        subprocess.run(["import", "-window", ids[0], str(base / name)], env=env, check=True)


with (base / "application.log").open("w") as log:
    process = subprocess.Popen([binary, "preferences"], env=env, stdout=log, stderr=log)
    try:
        wait(lambda: windows("Quick preferences"), "compact window")
        wait(lambda: selection().get("model") == "test/model-one", "preferences snapshot")
        assert not windows("^Speaches AI$"), "Preferences must not open the full app"
        window = windows("Quick preferences")[0]
        native_type = subprocess.check_output(["xprop", "-id", window, "_NET_WM_WINDOW_TYPE"], env=env, text=True)
        assert "_NET_WM_WINDOW_TYPE_DIALOG" in native_type
        subprocess.run(["xdotool", "windowfocus", "--sync", window], env=env, check=True)
        time.sleep(0.4)
        capture("initial.png")
        # First keyboard tab stop is the language Select, not a recording control.
        key("Tab")
        key("Down")
        type_text("Spanish")
        capture("language-search.png")
        key("Return")
        wait(lambda: selection().get("language") == "es", "language Select confirmation")
        assert command("bar").endswith(" ES")
        key("Tab")
        key("Down")
        type_text("model-two")
        capture("model-search.png")
        key("Return")
        wait(lambda: selection().get("model") == "test/model-two", "model Select confirmation")
        capture("selected.png")
        key("Escape")
        wait(lambda: not windows("Quick preferences"), "Escape dismiss")
        assert command("status") == "MIC"
        assert not windows("^Speaches AI$")
        key("ctrl+alt+shift+l")
        wait(lambda: selection().get("language") == "", "hidden global language cycle")
        assert not windows("Quick preferences") and not windows("^Speaches AI$")
        command("language-next")
        wait(lambda: selection().get("language") == "en", "IPC language cycle")
        command("language", "de")
        wait(lambda: selection().get("language") == "de", "direct language selection")
        assert command("bar").endswith(" DE")
        command("model", "missing-model")
        wait(lambda: selection().get("error") is not None, "invalid model rejection")
        assert selection()["model"] == "test/model-two"
        command("model", "test/model-one")
        wait(lambda: selection().get("model") == "test/model-one" and selection().get("error") is None,
             "model error recovery")
        saved = json.loads(settings_path.read_text())
        assert saved["language"] == "de" and saved["model"] == "test/model-one"
        assert saved["copy"] is False and "api_key" not in saved
        command("preferences")
        wait(lambda: windows("Quick preferences"), "reopen preferences")
        command("preferences")
        assert len(windows("Quick preferences")) == 1
        subprocess.run(["xdotool", "windowfocus", "--sync", windows("Quick preferences")[0]], env=env, check=True)
        time.sleep(0.2)
        command("show")
        wait(lambda: windows("^Speaches AI$"), "full app on explicit request")
        main_window = windows("^Speaches AI$")[0]
        subprocess.run(["xdotool", "windowfocus", "--sync", main_window], env=env, check=True)
        wait(lambda: not windows("Quick preferences"), "focus-loss dismissal")
        time.sleep(0.3)
        subprocess.run(["import", "-window", main_window, str(base / "main.png")], env=env, check=True)
        key("ctrl+2")  # Settings is a separate view; Ctrl+1 returns to Dictation.
        # Coordinate fallback checked against the actual 760x760 Xvfb settings
        # render. Other controls are exercised by keyboard. Never click Record.
        def click_at(x, y):
            subprocess.run(["xdotool", "mousemove", "--window", main_window, str(x), str(y), "click", "1"], env=env, check=True)
            time.sleep(0.15)

        def top_of_settings():
            subprocess.run(["xdotool", "mousemove", "--window", main_window, "730", "400",
                            "click", "--repeat", "25", "--delay", "10", "4"], env=env, check=True)
            time.sleep(0.2)

        def focused_text():
            key("ctrl+a")
            key("ctrl+c")
            return subprocess.check_output(["xclip", "-selection", "clipboard", "-out"], env=env, text=True, timeout=3)

        def focus_language():
            top_of_settings()
            click_at(180, 409)
            # API key, model, microphone, language. Refresh is disabled while the
            # connection has an unsaved draft, so it is not a Tab stop.
            for _ in range(4):
                key("Tab")

        click_at(180, 409)
        key("ctrl+a")
        type_text("http://unsaved.example.invalid")
        focus_language()
        key("ctrl+a")
        type_text("fr")
        assert focused_text() == "fr"
        key("ctrl+1")
        click_at(209, 130)  # Explicitly choosing already-active DE must replace FR.
        key("ctrl+2")
        focus_language()
        assert focused_text() == "de"
        top_of_settings()
        click_at(180, 409)
        assert focused_text() == "http://unsaved.example.invalid", "Unrelated draft was clobbered"
        command("language", "en")
        wait(lambda: selection().get("language") == "en", "external selection sync")
        focus_language()
        assert focused_text() == "en"
        saved = json.loads(settings_path.read_text())
        assert saved["base_url"] == f"http://127.0.0.1:{server.server_port}", "Quick choice saved unrelated draft"
        settings_path.parent.chmod(0o500)
        try:
            command("language", "fr")
            wait(lambda: selection().get("error") is not None, "selection persistence failure")
            assert selection()["language"] == "en"
            assert json.loads(settings_path.read_text())["language"] == "en"
            assert command("bar").endswith(" EN")
        finally:
            settings_path.parent.chmod(0o700)
        command("language", "fr")
        wait(lambda: selection().get("language") == "fr" and selection().get("error") is None,
             "selection persistence recovery")
        # Saving explicitly must preserve the unrelated draft AND the newest
        # immediately saved language, rather than writing stale form state.
        key("ctrl+s")
        wait(lambda: json.loads(settings_path.read_text())["base_url"] == "http://unsaved.example.invalid",
             "explicit settings save")
        assert json.loads(settings_path.read_text())["language"] == "fr"
        assert command("quit") == "OK"
        process.wait(timeout=5)
        assert process.returncode == 0
        print("PASS: compact dialog only, searchable language/model selection by keyboard, immediate persistence, language bar label, hidden global cycling, invalid-model recovery, single instance, focus-loss dismissal, quick-button/external sync without clobbering drafts, save-failure rollback/recovery")
    except Exception:
        capture("failure.png")
        raise
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
        server.shutdown()
        server.server_close()
        os.close(fd)
