#!/usr/bin/env python3
"""Exercise the SMB backend using a disposable, loopback-only Samba share.

Requires smbd (Ubuntu: samba). Runs unprivileged, uses no saved credentials,
and deletes the share and server state when finished. Override SMBD to use
an extracted Samba binary without installing or starting a system service.
"""

import os
from pathlib import Path
import pwd
import shutil
import signal
import socket
import subprocess
import tempfile
import time


def main():
    smbd = os.environ.get("SMBD") or shutil.which("smbd")
    if not smbd:
        raise SystemExit("smbd is required (install samba or set SMBD)")
    if os.getuid() == 0:
        raise SystemExit("Run this test as an ordinary user, without sudo")
    with tempfile.TemporaryDirectory(prefix="markerup-smb-") as directory:
        root = Path(directory)
        for name in ("share", "state", "cache", "private", "lock", "pid", "rpc"):
            (root / name).mkdir()
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        config = root / "smb.conf"
        config.write_text(f"""[global]
server role = standalone server
interfaces = 127.0.0.1
bind interfaces only = yes
smb ports = {port}
server min protocol = SMB2
map to guest = Bad User
guest account = {pwd.getpwuid(os.getuid()).pw_name}
load printers = no
disable spoolss = yes
printing = bsd
printcap name = /dev/null
state directory = {root}/state
cache directory = {root}/cache
private dir = {root}/private
lock directory = {root}/lock
pid directory = {root}/pid
ncalrpc dir = {root}/rpc
log file = {root}/smbd.log
[test]
path = {root}/share
guest ok = yes
guest only = yes
read only = no
""")
        with (root / "server-output.log").open("w+") as output:
            server = subprocess.Popen(
                [
                    smbd, "--foreground", "--no-process-group",
                    "--configfile", str(config), "--log-basename", str(root),
                    "--debug-stdout", "--debuglevel=3",
                ],
                stdout=output,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            try:
                deadline = time.monotonic() + 15
                while True:
                    if server.poll() is not None:
                        raise RuntimeError(
                            f"temporary Samba server exited before startup ({server.returncode})"
                        )
                    try:
                        with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                            break
                    except OSError:
                        if time.monotonic() >= deadline:
                            raise RuntimeError("temporary Samba server did not start") from None
                        time.sleep(0.1)
                env = dict(
                    os.environ,
                    MARKERUP_SMB_SERVER=f"127.0.0.1:{port}",
                    MARKERUP_SMB_SHARE="test",
                    MARKERUP_SMB_USERNAME="",
                    MARKERUP_SMB_PASSWORD="",
                    MARKERUP_SMB_REMOTE_PATH="",
                )
                subprocess.run(
                    [
                        "cargo", "test", "--locked", "--lib", "real_smb_round_trip",
                        "--", "--ignored", "--nocapture",
                    ],
                    cwd=Path(__file__).resolve().parent.parent,
                    env=env,
                    check=True,
                    timeout=300,
                )
            except BaseException:
                output.flush()
                output.seek(0)
                print(output.read())
                for log in sorted(set(root.glob("*.log")) | set(root.glob("log.*"))):
                    if log.name == "server-output.log":
                        continue
                    print(f"--- {log.name} ---")
                    print(log.read_text(errors="replace"))
                raise
            finally:
                # Include smbd's connection workers, even if its parent exited.
                try:
                    os.killpg(server.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(server.pid, signal.SIGKILL)
                    server.wait()


if __name__ == "__main__":
    main()
