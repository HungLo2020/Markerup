#!/usr/bin/python3
"""Exercise the real Linux scheduler/notify-rust transport on a private D-Bus.
Run after cargo build:
  dbus-run-session -- /usr/bin/python3 DevUtils/test_reminder_notifications.py
Requires python3-dbus and python3-gi. Uses only temporary data and never opens UI.
"""
import datetime
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import shutil
import tempfile
import threading
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

DBusGMainLoop(set_as_default=True)


class Notifications(dbus.service.Object):
    def __init__(self, bus):
        self.name = dbus.service.BusName("org.freedesktop.Notifications", bus)
        super().__init__(bus, "/org/freedesktop/Notifications")
        self.failed = 0
        self.fail = True
        self.received = []

    @dbus.service.method("org.freedesktop.Notifications", in_signature="susssasa{sv}i", out_signature="u")
    def Notify(self, app, replaces, icon, summary, body, actions, hints, timeout):
        if self.fail:
            self.failed += 1
            raise dbus.exceptions.DBusException("Intentional test failure", name="org.freedesktop.Notifications.Error")
        assert list(actions) == ["default", "Open note"], "Missing note navigation action"
        self.received.append((str(app), str(summary), str(body)))
        return len(self.received)

    @dbus.service.method("org.freedesktop.Notifications", out_signature="as")
    def GetCapabilities(self):
        return ["body", "actions"]

    @dbus.service.method("org.freedesktop.Notifications", out_signature="ssss")
    def GetServerInformation(self):
        return ("Markerup test server", "Markerup", "1", "1.2")


def wait_for(predicate, message, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def main():
    bus = dbus.SessionBus()
    server = Notifications(bus)
    loop = GLib.MainLoop()
    threading.Thread(target=loop.run, daemon=True).start()
    executable = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parents[1] / "target/debug/markerup"
    with tempfile.TemporaryDirectory(prefix="markerup-notifications-") as directory:
        root = Path(directory)
        workspace = root / "notes"
        workspace.mkdir()
        index = root / "data/markerup/reminders"
        index.mkdir(parents=True)
        stamp = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=2)).strftime("%Y-%m-%d %H:%M")
        source = f"- [ ] Call Mom @remind({stamp}; tz=UTC; id=call)\n"
        note = workspace / "note.md"
        note.write_text(source)
        store_path = index / "index.json"
        store_path.write_text(json.dumps({"version": 1, "generation": 0, "workspaces": {str(workspace): {"local_path": str(workspace), "notes": {}, "error": None, "scanned_at": 0}}, "cursors": {}, "notification_error": None}))
        env = {**os.environ, "XDG_DATA_HOME": str(root / "data"), "XDG_CONFIG_HOME": str(root / "config")}
        process = None

        def launch(binary=executable):
            return subprocess.Popen([str(binary), "--reminder-service"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

        def store():
            with (index / "index.lock").open("a+") as lock:
                fcntl.flock(lock, fcntl.LOCK_SH)
                return json.loads(store_path.read_text())

        try:
            process = launch()
            wait_for(lambda: server.failed and store()["notification_error"], "Delivery failure was not reported")
            assert not store()["cursors"], "Failed notification was incorrectly acknowledged"
            server.fail = False
            wait_for(lambda: len(server.received) == 1 and store()["cursors"], "No successful native notification after retry")
            assert server.received[0][0] == "Markerup"
            assert "Call Mom" in server.received[0][1]
            assert store()["notification_error"] is None
            assert note.read_text() == source, "Scheduler modified the note"
            process.terminate(); process.wait(timeout=5)
            process = launch()
            time.sleep(4.5)
            assert len(server.received) == 1, "Restart duplicated a delivered reminder"
            process.terminate(); process.wait(timeout=5)
            # Erase delivery history so a stale cache would notify immediately.
            cached = store()
            cached["cursors"] = {}
            store_path.write_text(json.dumps(cached))
            # A completed task disappears before any cached delivery is attempted.
            note.write_text(source.replace("[ ]", "[x]"))
            process = launch()
            wait_for(lambda: not store()["workspaces"][str(workspace)]["notes"]["note.md"]["parsed"]["reminders"], "Completed task was not removed")
            assert len(server.received) == 1
            process.terminate(); process.wait(timeout=5)
            # A new reminder is discovered from Markdown by the headless process.
            note.write_text(source.replace("id=call", "id=second").replace("Call Mom", "Second reminder"))
            process = launch()
            wait_for(lambda: len(server.received) == 2, "New saved Markdown reminder was not delivered")
            assert "Second reminder" in server.received[1][1]
            process.terminate(); process.wait(timeout=5)
            # Corruption must keep the worker alive and recover after repair.
            healthy = store_path.read_text()
            store_path.write_text("invalid JSON")
            process = launch()
            wait_for(lambda: (index / "service.error").exists(), "Index failure was not reported")
            assert process.poll() is None, "Service exited on recoverable index error"
            duplicate = launch()
            assert duplicate.wait(timeout=5) == 0, "Retrying worker lost ownership"
            store_path.write_text(healthy)
            wait_for(lambda: not (index / "service.error").exists(), "Service did not recover after index repair", timeout=30)
            assert process.poll() is None
            assert len(server.received) == 2, "Repair replayed notifications"
            # Competing starts must exit even while the leader is retrying errors.
            duplicate = launch()
            assert duplicate.wait(timeout=5) == 0
            assert process.poll() is None
            assert len(server.received) == 2
            assert not (root / "config/autostart/markerup-reminders.desktop").exists()
            assert not (root / "config/systemd/user/markerup-reminders.service").exists()
            process.terminate(); process.wait(timeout=5)
            legacy = root / "data/markerup-dev/reminders"
            legacy.mkdir(parents=True, exist_ok=True)
            prior = store()
            (legacy / "index.json").write_text(json.dumps(prior))
            prior["cursors"] = {}
            store_path.write_text(json.dumps(prior))
            # Simulate both generations of pre-migration workers. They release
            # their actual flock only after receiving the handoff request.
            old_code = """
import fcntl,sys,time
from pathlib import Path
p=Path(sys.argv[1]); f=(p/'service.lock').open('a+')
fcntl.flock(f,fcntl.LOCK_EX)
(p/'service.request').write_text('legacy')
print('ready',flush=True)
while (p/'service.request').read_text() == 'legacy': time.sleep(.05)
"""
            old_workers = [subprocess.Popen([sys.executable, "-c", old_code, str(path)], stdout=subprocess.PIPE, text=True) for path in (index, legacy)]
            try:
                for old in old_workers: assert old.stdout.readline().strip() == "ready"
                process = launch()
                for old in old_workers: assert old.wait(timeout=10) == 0
                wait_for(lambda: store().get("linux_legacy_import") is not None and store()["cursors"], "Legacy delivery history was not migrated")
                time.sleep(3)
                assert len(server.received) == 2, "Migration replayed an acknowledged reminder"
                # A second launch cannot deliver even with a different binary path.
                alternate = root / "alternate-markerup"
                try: os.link(executable, alternate)
                except OSError: shutil.copy2(executable, alternate)
                duplicate = launch(alternate)
                assert duplicate.wait(timeout=5) == 0
                assert process.poll() is None
                assert len(server.received) == 2
                with (legacy / "service.lock").open("a+") as lock:
                    try: fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    except BlockingIOError: pass
                    else: raise AssertionError("Legacy namespace was not held by the shared worker")
            finally:
                for old in old_workers:
                    if old.poll() is None: old.terminate(); old.wait(timeout=5)
            print("PASS: Markdown discovery → native D-Bus delivery; failure/retry; persistent deduplication; startup cancellation before cached delivery; new reminder discovery; native navigation action; index-error recovery; shared development/release state; duplicate starts; legacy handoff and cursor migration. No note contents changed by the scheduler.")
        finally:
            if process and process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
            loop.quit()


if __name__ == "__main__":
    main()
