"""Live System appearance on an isolated D-Bus + X11 session; no audio capture.
Usage: dbus-run-session -- xvfb-run -a python tests/system_theme_smoke.py /absolute/artifact-dir
Requires dbus-python, PyGObject, xdotool, ImageMagick, and the built app.
The fake portal refuses an already-owned name, so it cannot replace a real portal.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
assert not bus.name_has_owner("org.freedesktop.portal.Desktop"), "Use a fresh dbus-run-session"
name = dbus.service.BusName("org.freedesktop.portal.Desktop", bus, do_not_queue=True)


class Portal(dbus.service.Object):
    scheme = 1  # Prefer dark.

    @dbus.service.method("org.freedesktop.portal.Settings", in_signature="ss", out_signature="v")
    def Read(self, namespace, key):
        if (namespace, key) == ("org.freedesktop.appearance", "color-scheme"):
            return dbus.UInt32(self.scheme, variant_level=1)
        raise dbus.exceptions.DBusException("Unknown setting", name="org.freedesktop.portal.Error.NotFound")

    @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
    def Get(self, interface, key):
        if interface == "org.freedesktop.portal.Settings" and key == "version":
            return dbus.UInt32(2, variant_level=1)
        raise dbus.exceptions.DBusException("Unknown property")

    @dbus.service.signal("org.freedesktop.portal.Settings", signature="ssv")
    def SettingChanged(self, namespace, key, value):
        pass

    def change(self, scheme):
        self.scheme = scheme
        self.SettingChanged("org.freedesktop.appearance", "color-scheme", dbus.UInt32(scheme))
        return GLib.SOURCE_REMOVE


portal = Portal(bus, "/org/freedesktop/portal/desktop")
loop = GLib.MainLoop()
threading.Thread(target=loop.run, daemon=True).start()
base = Path(sys.argv[1]).resolve()
base.mkdir(parents=True, exist_ok=True)
for directory in ("runtime", "config/speaches-ai"):
    (base / directory).mkdir(parents=True, exist_ok=True, mode=0o700)
fd = os.open(base / "runtime", os.O_RDONLY | os.O_DIRECTORY)
env = dict(os.environ, XDG_RUNTIME_DIR=f"/proc/{os.getpid()}/fd/{fd}", XDG_CONFIG_HOME=str(base / "config"))
binary = str(Path("target/debug/speaches-ai").resolve())
settings = base / "config/speaches-ai/settings.json"
settings.write_text('{"theme":"system"}')


def command(arg):
    return subprocess.check_output([binary, arg], env=env, text=True, timeout=3).strip()


def windows(pattern="^Speaches AI$"):
    return subprocess.run(["xdotool", "search", "--onlyvisible", "--name", pattern],
                          env=env, capture_output=True, text=True).stdout.split()


def wait(predicate, label):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        assert process.poll() is None, f"App exited while waiting for {label}"
        if predicate():
            return
        time.sleep(0.1)
    raise AssertionError(f"Timed out: {label}")


def surface(window):
    path = base / "surface.png"
    subprocess.run(["import", "-window", window, str(path)], env=env, check=True)
    pixel = subprocess.check_output(["convert", str(path), "-crop", "1x1+2+2", "-depth", "8", "rgb:-"])
    return sum(pixel) / len(pixel)


def key(window, value):
    subprocess.run(["xdotool", "windowfocus", "--sync", window], env=env, check=True)
    subprocess.run(["xdotool", "key", "--clearmodifiers", value], env=env, check=True)
    time.sleep(0.2)


def click(window, x, y):
    subprocess.run(["xdotool", "mousemove", "--window", window, str(x), str(y), "click", "1"],
                   env=env, check=True)
    time.sleep(0.3)


with (base / "application.log").open("w") as log:
    process = subprocess.Popen([binary], env=env, stdout=log, stderr=log)
    try:
        wait(windows, "main window")
        window = windows()[0]
        time.sleep(2)
        wait(lambda: surface(window) < 40, "portal startup Dark")
        GLib.idle_add(portal.change, 2)
        wait(lambda: surface(window) > 200, "live portal Light")
        GLib.idle_add(portal.change, 1)
        wait(lambda: surface(window) < 40, "live portal Dark")
        key(window, "ctrl+q")
        wait(lambda: not windows(), "hidden main")
        GLib.idle_add(portal.change, 2)
        time.sleep(0.4)
        command("preferences")
        wait(lambda: windows("Quick preferences"), "quick-only window")
        quick = windows("Quick preferences")[0]
        wait(lambda: surface(quick) > 200, "hidden session follows Light")
        GLib.idle_add(portal.change, 1)
        wait(lambda: surface(quick) < 40, "quick-only live Dark")
        key(quick, "Escape")
        wait(lambda: not windows("Quick preferences"), "quick closed")
        command("show")
        wait(windows, "recreated main")
        window = windows()[0]
        time.sleep(1)
        key(window, "ctrl+2")
        click(window, 207, 235)  # Fixed Dark.
        assert json.loads(settings.read_text())["theme"] == "dark"
        GLib.idle_add(portal.change, 2)
        time.sleep(0.5)
        assert surface(window) < 40, "System overrode fixed Dark"
        click(window, 55, 235)  # Return to System, using latest Light preference.
        wait(lambda: surface(window) > 200, "System resumes latest portal preference")
        GLib.idle_add(portal.change, 1)
        wait(lambda: surface(window) < 40, "System returns to Dark")
        GLib.idle_add(portal.change, 0)
        wait(lambda: surface(window) > 200, "no preference falls back to Light")
        command("quit")
        process.wait(timeout=5)
        assert process.returncode == 0
        print("PASS: portal startup/live updates, hidden and quick-only System changes, "
              "fixed override isolation, resume System, no-preference Light fallback")
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
        loop.quit()
        os.close(fd)
