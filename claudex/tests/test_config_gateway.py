from __future__ import annotations

import stat
import tempfile
import unittest
from pathlib import Path

import yaml

from claudex.config import config_path, is_private, litellm_environment, load_settings, oauth_auth_file, private_directory
from claudex.gateway import gateway_yaml, write_gateway_config
from claudex.models import ModelSettings


class ConfigAndGatewayTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.env = {"XDG_CONFIG_HOME": self.temporary.name + "/config", "XDG_STATE_HOME": self.temporary.name + "/state"}

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_loads_additive_config_and_resolves_roles(self) -> None:
        path = config_path(self.env)
        private_directory(path.parent)
        path.write_text('default = "review"\n[models]\nreview = "gpt-5.6-sol"\n[roles]\nsonnet = "review"\n')
        path.chmod(0o600)
        settings = load_settings(self.env)
        self.assertEqual(settings.resolve(), "gpt-5.6-sol")
        self.assertEqual(settings.role_model("sonnet"), "gpt-5.6-sol")
        self.assertEqual(settings.role_model("haiku"), "gpt-5.6-luna")

    def test_runtime_and_token_paths_are_private(self) -> None:
        values = litellm_environment(self.env)
        token = oauth_auth_file(self.env)
        self.assertEqual(token.parent, Path(values["CHATGPT_TOKEN_DIR"]))
        self.assertTrue(is_private(token.parent, 0o700))
        token.write_text("{}")
        token.chmod(0o600)
        self.assertTrue(is_private(token, 0o600))

    def test_gateway_yaml_has_plain_response_models_and_extra_id(self) -> None:
        output = gateway_yaml(ModelSettings(), extra_models=["gpt-custom"])
        self.assertIn("model_name: gpt-5.6-terra", output)
        self.assertIn("model_name: gpt-custom", output)
        self.assertIn("model: chatgpt/gpt-custom", output)
        parsed = yaml.safe_load(output)
        entry = next(item for item in parsed["model_list"] if item["model_name"] == "gpt-custom")
        self.assertEqual(entry["model_info"], {"mode": "responses"})
        self.assertEqual(entry["litellm_params"]["model"], "chatgpt/gpt-custom")
        self.assertEqual(entry["litellm_params"]["supports_system_message"], False)
        self.assertTrue(entry["litellm_params"]["drop_params"])
        self.assertEqual(parsed["general_settings"]["master_key"], "os.environ/CLAUDEX_LITELLM_MASTER_KEY")

    def test_gateway_rejects_unsafe_model_id(self) -> None:
        with self.assertRaisesRegex(ValueError, "invalid model ID"):
            gateway_yaml(ModelSettings(models={"bad": "good\\ninjected: true"}))

    def test_gateway_config_is_private(self) -> None:
        path = write_gateway_config(Path(self.temporary.name) / "runtime", ModelSettings())
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
