from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from claudex.models import ModelSettings
from claudex.supervisor import ProxySupervisor, run_child


class FakeProcess:
    def __init__(self, returncode: int | None = None) -> None:
        self.returncode = returncode
        self.terminated = False

    def poll(self) -> int | None:
        return self.returncode

    def terminate(self) -> None:
        self.terminated = True
        self.returncode = 0

    def kill(self) -> None:
        self.returncode = -9

    def wait(self, timeout: float | None = None) -> int:
        if self.returncode is None:
            self.returncode = 0
        return self.returncode


class SupervisorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.env = {"XDG_STATE_HOME": self.temporary.name, "XDG_CONFIG_HOME": self.temporary.name + "/config"}

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_starts_private_proxy_and_cleans_it_up(self) -> None:
        captured: dict[str, object] = {}
        process = FakeProcess()

        def popen(command: list[str], **kwargs: object) -> FakeProcess:
            captured["command"] = command
            captured["config"] = Path(command[command.index("--config") + 1]).read_text()
            captured["env"] = kwargs["env"]
            return process

        with ProxySupervisor(ModelSettings(), self.env, popen=popen, ready=lambda _: True, extra_models=["gpt-direct"]) as proxy:
            self.assertIn("gpt-direct", captured["config"])
            self.assertNotIn(proxy.token, " ".join(captured["command"]))
            self.assertEqual(proxy.base_url.split(":")[0], "http")
        self.assertTrue(process.terminated)

    def test_reports_safe_proxy_log_on_startup_failure(self) -> None:
        def popen(_command: list[str], **kwargs: object) -> FakeProcess:
            kwargs["stdout"].write(f"invalid config; secret {kwargs['env']['CLAUDEX_LITELLM_MASTER_KEY']}\\n")
            return FakeProcess(returncode=2)

        supervisor = ProxySupervisor(ModelSettings(), self.env, popen=popen, ready=lambda _: False)
        with self.assertRaisesRegex(RuntimeError, "Proxy log tail") as error:
            supervisor.__enter__()
        self.assertIn("[redacted]", str(error.exception))

    def test_forwards_child_exit_status(self) -> None:
        process = FakeProcess(returncode=17)
        with patch("claudex.supervisor.subprocess.Popen", return_value=process):
            self.assertEqual(run_child(["claude", "--resume"], {}), 17)
