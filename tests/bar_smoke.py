"""Exercise the real persistent bar worker against fake local IPC; no GUI/audio.

Usage: python tests/bar_smoke.py /absolute/test-artifact-directory
"""
from pathlib import Path
import json
import os
import queue
import socket
import subprocess
import sys
import threading
import time

base = Path(sys.argv[1]).resolve()
base.mkdir(parents=True, exist_ok=True)
runtime = base / "runtime"
(runtime / "speaches-ai").mkdir(parents=True, exist_ok=True, mode=0o700)
fd = os.open(runtime, os.O_RDONLY | os.O_DIRECTORY)
socket_path = runtime / "speaches-ai/app.sock"
socket_path.unlink(missing_ok=True)
server = socket.socket(socket.AF_UNIX)
short_path = f"/proc/{os.getpid()}/fd/{fd}/speaches-ai/app.sock"
server.bind(short_path)
server.listen()
server.settimeout(0.2)
stop = threading.Event()
state = {"state": "Ready", "animate": True, "language": "en"}
commands = queue.Queue()


def serve():
    while not stop.is_set():
        try:
            connection, _ = server.accept()
        except socket.timeout:
            continue
        with connection:
            connection.settimeout(2)
            command = b""
            while data := connection.recv(64):
                command += data
            command = command.decode()
            commands.put(command)
            response = json.dumps(state) if command == "snapshot" else "OK"
            connection.sendall((response + "\n").encode())


thread = threading.Thread(target=serve, daemon=True)
thread.start()
env = dict(os.environ, XDG_RUNTIME_DIR=f"/proc/{os.getpid()}/fd/{fd}")
process = subprocess.Popen(["target/debug/speaches-ai", "bar-watch"], env=env,
                           stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, text=True)
frames = queue.Queue()


def read_frames():
    for line in process.stdout:
        frames.put(json.loads(line)["full_text"])


threading.Thread(target=read_frames, daemon=True).start()


def until(predicate):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            frame = frames.get(timeout=0.2)
        except queue.Empty:
            continue
        if predicate(frame):
            return frame
    raise AssertionError("Expected frame not received")


try:
    initial = until(lambda frame: "\uf130" in frame)
    assert "foreground=" in initial
    assert initial.endswith(" EN")
    time.sleep(0.3)
    assert frames.empty(), "Idle state should not redraw continuously"
    state["state"] = {"Recording": 75}
    first = until(lambda frame: "1:15" in frame)
    second = until(lambda frame: "1:15" in frame and frame != first)
    assert first != second, "Recording must pulse"
    state["state"] = "Transcribing"
    first = until(lambda frame: "DejaVu Sans Mono" in frame)
    until(lambda frame: "DejaVu Sans Mono" in frame and frame != first)
    state["animate"] = False
    until(lambda frame: "\uf1ce" in frame)
    time.sleep(0.3)
    assert frames.empty(), "Reduced-motion mode should stop animation"
    process.stdin.write('{"button":1,"x":123,"":"ignored"}\n')
    process.stdin.write('{"button":3}\n')
    process.stdin.write('{"button":2}\n')
    process.stdin.flush()
    deadline = time.monotonic() + 3
    seen = []
    while time.monotonic() < deadline:
        try:
            seen.append(commands.get(timeout=0.2))
        except queue.Empty:
            continue
        if {"show", "preferences", "language-next"} <= set(seen):
            break
    assert {"show", "preferences", "language-next"} <= set(seen), "Clicks must dispatch the expected commands"
    assert set(seen) <= {"snapshot", "show", "preferences", "language-next"}, "No unexpected actions"
    process.stdin.close()
    process.wait(timeout=3)
    assert process.returncode == 0, process.stderr.read()
    print("PASS: colored icons, recording pulse, transcription spinner, idle/reduced-motion quiet, language label, left/right/middle click IPC, EOF shutdown")
finally:
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=3)
    stop.set()
    thread.join(timeout=1)
    server.close()
    socket_path.unlink(missing_ok=True)
    os.close(fd)
