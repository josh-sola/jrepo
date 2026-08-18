from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path


class InstallerTests(unittest.TestCase):
    def test_uninstall_reports_a_no_op_when_tool_is_absent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake_uv = root / "uv"
            fake_uv.write_text(
                "#!/usr/bin/env bash\n"
                "if [[ \"$1 $2\" == \"tool list\" && \"${FAKE_UV_STATE:-}\" == \"present\" ]]; then\n"
                "  echo 'jrepo-claudex v0.1.0'\n"
                "elif [[ \"$1 $2\" == \"tool uninstall\" ]]; then\n"
                "  echo \"$*\" >> \"$FAKE_UV_LOG\"\n"
                "fi\n"
            )
            fake_uv.chmod(0o755)
            log = root / "uv.log"
            environment = {**os.environ, "PATH": f"{root}:{os.environ['PATH']}", "FAKE_UV_LOG": str(log)}
            installer = Path(__file__).parents[1] / "install.sh"
            result = subprocess.run(["bash", str(installer), "--uninstall"], env=environment, check=False, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("claudex is not installed", result.stdout)
            self.assertNotIn("Removed the claudex tool", result.stdout)
            self.assertFalse(log.exists())
