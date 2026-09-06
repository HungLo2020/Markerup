import io
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

from DevUtils import PublishLatestDebToMattOSRepo as publisher


class PublishLatestDebTests(unittest.TestCase):
    def run_publisher(self, *args):
        def fake_run(command, *, check):
            if command[1] == str(publisher.DEBIAN_WORKFLOW_SCRIPT):
                output_directory = Path(command[command.index("--output-dir") + 1])
                (output_directory / "markerup.deb").touch()

        with (
            patch.object(sys, "argv", [str(publisher.__file__), *args]),
            patch.object(publisher.subprocess, "run", side_effect=fake_run) as run,
            patch.object(publisher, "download_latest_manager", return_value="mock-sha") as download,
            patch("sys.stdout", new_callable=io.StringIO),
        ):
            self.assertEqual(publisher.main(), 0)

        self.assertEqual(run.call_count, 2)
        download.assert_called_once()
        return run.call_args_list, download.call_args.args[0]

    def test_upload_explicitly_selects_mattpackages(self):
        calls, manager_path = self.run_publisher()
        release_command = calls[0].args[0]
        package = Path(release_command[-1]) / "markerup.deb"

        self.assertNotIn("--repo", release_command)
        self.assertEqual(
            calls[1].args[0],
            [sys.executable, str(manager_path), "--repo", "mattpackages", "upload", str(package)],
        )
        self.assertEqual(calls[1].kwargs, {"check": True})

    def test_github_repo_is_forwarded_unchanged(self):
        github_repo = "example-owner/custom-markerup"
        calls, manager_path = self.run_publisher("--repo", github_repo)
        output_directory = calls[0].args[0][-1]

        self.assertEqual(
            calls[0].args[0],
            [
                sys.executable,
                str(publisher.DEBIAN_WORKFLOW_SCRIPT),
                "--repo",
                github_repo,
                "--download-only",
                "--output-dir",
                output_directory,
            ],
        )
        self.assertEqual(calls[0].kwargs, {"check": True})
        self.assertEqual(
            calls[1].args[0],
            [
                sys.executable,
                str(manager_path),
                "--repo",
                "mattpackages",
                "upload",
                str(Path(output_directory) / "markerup.deb"),
            ],
        )


if __name__ == "__main__":
    unittest.main()
