---
name: publish-mattos-deb
description: Publish an updated Markerup Debian package to the MattOS repository when the user explicitly asks to update the program there.
---

# Publish Markerup to MattOS

Use this skill only when the user explicitly asks to update or publish Markerup in the MattOS repository. Do not run the publisher for questions, audits, or build-only requests.

From the Markerup repository root:

1. Check the working tree. Do not discard or silently include unrelated uncommitted changes; if any are present, stop and report them.
2. In `Cargo.toml`, increment only the package patch version by one, preserving the major and minor numbers. For example, `0.1.1` becomes `0.1.2`, and `1.3.9` becomes `1.3.10`.
3. Make no other source or configuration changes. Do not create a commit.
4. Run the publisher from the repository root:

   ```bash
   python3 DevUtils/PublishLatestDebToMattOSRepo.py
   ```

5. Report whether the command succeeded and the package version. If it fails, report the error without making additional changes or committing.
