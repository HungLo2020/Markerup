import io
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from DevUtils import PublishLatestDebToMattOSRepo as publisher


class PublishLatestDebTests(unittest.TestCase):
    def run_publisher(self, package_directory, *args):
        (package_directory / "stale.deb").write_bytes(b"old package")

        def fake_run(command, **kwargs):
            if command[:2] == ["cargo", "tauri"]:
                (package_directory / "markerup.deb").touch()

        with (
            patch.object(sys, "argv", [str(publisher.__file__), *args]),
            patch.object(publisher, "DEBIAN_PACKAGE_DIRECTORY", package_directory),
            patch.object(publisher.subprocess, "run", side_effect=fake_run) as run,
            patch.object(publisher, "download_latest_manager", return_value="mock-sha") as download,
            patch("sys.stdout", new_callable=io.StringIO),
        ):
            self.assertEqual(publisher.main(), 0)

        return run.call_args_list, download

    def test_builds_local_package_then_uploads_to_mattpackages(self):
        with tempfile.TemporaryDirectory() as temporary:
            package_directory = Path(temporary)
            calls, download = self.run_publisher(package_directory)
        package = package_directory / "markerup.deb"
        manager_path = calls[1].args[0][1]

        self.assertEqual(
            calls[0].args[0],
            ["cargo", "tauri", "build", "--bundles", "deb", "--", "--locked"],
        )
        self.assertEqual(calls[0].kwargs, {"cwd": publisher.REPOSITORY_ROOT, "check": True})
        self.assertEqual(
            calls[1].args[0],
            [
                sys.executable,
                str(manager_path),
                "--repo",
                "mattpackages",
                "upload",
                str(package),
            ],
        )
        self.assertEqual(calls[1].kwargs, {"check": True})
        download.assert_called_once()

    def test_build_only_does_not_download_manager_or_upload(self):
        with tempfile.TemporaryDirectory() as temporary:
            package_directory = Path(temporary)
            calls, download = self.run_publisher(package_directory, "--build-only")

        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0].args[0][:3], ["cargo", "tauri", "build"])
        download.assert_not_called()


if __name__ == "__main__":
    unittest.main()
