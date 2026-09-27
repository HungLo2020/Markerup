#!/usr/bin/python3
"""Test the extracted Debian hooks and service setup on a private D-Bus.
Usage: dbus-run-session -- /usr/bin/python3 DevUtils/test_reminder_package.py package.deb
No package is installed; no real user session or startup entry is changed.
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
from test_reminder_notifications import wait_for


def main():
    package = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="markerup-deb-test-") as directory:
        root = Path(directory)
        payload, control = root / "payload", root / "control"
        subprocess.run(["dpkg-deb", "--extract", str(package), str(payload)], check=True)
        subprocess.run(["dpkg-deb", "--control", str(package), str(control)], check=True)
        binary = payload / "usr/bin/markerup"
        helper = payload / "usr/lib/markerup/reminder-sessions"
        assert helper.stat().st_mode & 0o111
        assert "--reminder-setup" in (payload / "etc/xdg/autostart/markerup-reminder-bootstrap.desktop").read_text()
        assert (payload / "usr/share/markerup/reminders/service-version").read_text().strip() == "1"
        user_home = root / "home"
        runtime = root / "runtime/42424"
        runtime.mkdir(parents=True)
        socket_file = socket.socket(socket.AF_UNIX)
        socket_file.bind(str(runtime / "bus"))
        fake_bin = root / "bin"
        fake_bin.mkdir()
        # Stub only OS user/session management. The package's real executable
        # handles setup, handoff, migration, retries, and removal.
        stubs = {
            "getent": f'#!/bin/sh\nprintf "%s\\n" "testuser:x:42424:42424::{user_home}:/bin/sh"\n',
            "runuser": '#!/bin/sh\n[ "$1" = -u ] && [ "$2" = testuser ] && [ "$3" = -- ] || exit 9\nshift 3\nexec "$@"\n',
            "systemctl": r'''#!/usr/bin/python3
import os,sys,subprocess,signal
from pathlib import Path
if os.environ.get("TEST_SYSTEMD") != "1": sys.exit(1)
args=sys.argv[1:]; pidfile=Path(os.environ["TEST_PIDFILE"])
if "stop" in args or "restart" in args:
    if pidfile.exists():
        try: os.kill(int(pidfile.read_text()),signal.SIGTERM)
        except ProcessLookupError: pass
        pidfile.unlink(missing_ok=True)
if "restart" in args:
    child=subprocess.Popen([os.environ["TEST_BINARY"],"--reminder-service"],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
    pidfile.write_text(str(child.pid))
''',
        }
        for name, text in stubs.items():
            path = fake_bin / name
            path.write_text(text)
            path.chmod(0o755)
        # Redirect only fixed installation/session paths into the extracted tree.
        rewritten = helper.read_text().replace("/run/user/", str(root / "runtime") + "/").replace("/usr/bin/markerup", str(binary))
        # Keep the real private notification bus; the socket above represents
        # an active login for the session enumeration branch.
        rewritten = rewritten.replace('DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus"', 'DBUS_SESSION_BUS_ADDRESS="$TEST_BUS"')
        sandbox_helper = root / "sessions"
        sandbox_helper.write_text(rewritten)
        sandbox_helper.chmod(0o755)
        env = {**os.environ, "PATH": str(fake_bin) + ":" + os.environ["PATH"], "TEST_BUS": os.environ["DBUS_SESSION_BUS_ADDRESS"], "TEST_BINARY": str(binary), "TEST_PIDFILE": str(root / "systemd.pid")}
        for name in ("postinst", "prerm"):
            path = root / name
            path.write_text((control / name).read_text().replace("/usr/lib/markerup/reminder-sessions", str(sandbox_helper)))
            path.chmod(0o755)
        index = user_home / ".local/share/markerup/reminders"
        config = user_home / ".config/autostart/markerup-reminders.desktop"

        def owner():
            return json.loads((index / "service.owner.json").read_text())

        try:
            subprocess.run([str(root / "postinst"), "configure"], env=env, check=True, timeout=60)
            wait_for(lambda: (index / "service.heartbeat").exists(), "Package setup did not start a worker")
            assert str(binary) in config.read_text()
            assert owner()["executable"] == str(binary)
            # Simulate an upgrade: a second configure must stop/start safely.
            subprocess.run([str(root / "postinst"), "configure", "0.4.3"], env=env, check=True, timeout=60)
            wait_for(lambda: (index / "service.heartbeat").exists(), "Upgrade did not leave a worker")
            subprocess.run([str(root / "prerm"), "upgrade", "0.4.5"], env=env, check=True)
            assert config.exists(), "prerm upgrade removed startup prematurely"
            subprocess.run([str(root / "prerm"), "remove"], env=env, check=True, timeout=60)
            assert not config.exists(), "Removal left a stale startup entry"
            import fcntl
            with (index / "leader.lock").open("a+") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            # Exercise managed startup as well as the fallback above.
            env["TEST_SYSTEMD"] = "1"
            subprocess.run([str(root / "postinst"), "configure"], env=env, check=True, timeout=60)
            unit = user_home / ".config/systemd/user/markerup-reminders.service"
            assert str(binary) in unit.read_text()
            assert "Restart=on-failure" in unit.read_text()
            assert not config.exists()
            first_pid = (root / "systemd.pid").read_text()
            subprocess.run([str(root / "postinst"), "configure", "0.4.3"], env=env, check=True, timeout=60)
            assert (root / "systemd.pid").read_text() != first_pid, "Upgrade did not restart the managed worker"
            subprocess.run([str(root / "prerm"), "remove"], env=env, check=True, timeout=60)
            assert not unit.exists()
            print("PASS: packaged install/upgrade hooks, per-user setup, login bootstrap, handoff, and removal; no real installation performed.")
        finally:
            subprocess.run([str(root / "prerm"), "remove"], env=env, timeout=60, check=False)
            socket_file.close()


if __name__ == "__main__":
    main()
