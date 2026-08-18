from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from claudex import cli
from claudex.models import ModelSettings


class CliTests(unittest.TestCase):
    def test_forwards_claude_args_after_separator(self) -> None:
        self.assertEqual(cli.parse_launch_args(["--model", "sol", "--", "-p", "hello"]), ("sol", ["-p", "hello"]))

    def test_claude_environment_uses_proxy_origin_and_role_models(self) -> None:
        result = cli.claude_environment({"ANTHROPIC_API_KEY": "old"}, ModelSettings(), "gpt-direct", "http://127.0.0.1:3210", "key")
        self.assertEqual(result["ANTHROPIC_BASE_URL"], "http://127.0.0.1:3210")
        self.assertNotIn("ANTHROPIC_API_KEY", result)
        self.assertEqual(result["ANTHROPIC_MODEL"], "claudex/gpt-direct")
        self.assertEqual(result["ANTHROPIC_DEFAULT_OPUS_MODEL"], "claudex/gpt-5.6-sol")

    def test_launch_forwards_direct_model_to_gateway(self) -> None:
        captured: dict[str, object] = {}

        class Supervisor:
            def __init__(self, _settings: ModelSettings, **kwargs: object) -> None:
                captured.update(kwargs)
                self.base_url = "http://127.0.0.1:1234"
                self.token = "test-key"

            def __enter__(self) -> "Supervisor":
                return self

            def __exit__(self, *_: object) -> None:
                pass

        with patch.object(cli.shutil, "which", return_value="/usr/bin/claude"), patch.object(cli, "ProxySupervisor", Supervisor), patch.object(
            cli, "run_child", return_value=23
        ) as run_child:
            self.assertEqual(cli.launch("gpt-direct", ["--resume"]), 23)
        self.assertIn("gpt-direct", captured["extra_models"])
        run_child.assert_called_once()

    def test_login_uses_isolated_chatgpt_token_dir_and_private_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            env = {"XDG_STATE_HOME": directory, "XDG_CONFIG_HOME": directory + "/config"}
            from litellm.llms.chatgpt.authenticator import Authenticator

            def get_access_token(_self: object) -> str:
                path = Path(os.environ["CHATGPT_TOKEN_DIR"]) / "auth.json"
                path.write_text("{}")
                path.chmod(0o644)
                return "token"

            with patch.object(Authenticator, "get_access_token", get_access_token):
                self.assertEqual(cli.login(env), 0)
            token = Path(directory) / "claudex" / "chatgpt" / "auth.json"
            self.assertEqual(token.stat().st_mode & 0o777, 0o600)
