from __future__ import annotations

import json
import os
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path
from unittest.mock import patch

from typer.testing import CliRunner

from claudex import cli
from claudex.models import ModelSettings


class CliTests(unittest.TestCase):
    def setUp(self) -> None:
        self.runner = CliRunner()

    def test_default_launch_forwards_unknown_options_without_separator(self) -> None:
        with patch.object(cli, "launch", return_value=0) as launch:
            self.assertEqual(cli.main(["--model", "sol", "-p", "hello", "--verbose"]), 0)
        launch.assert_called_once_with("sol", ["-p", "hello", "--verbose"])

    def test_default_launch_forwards_positional_first_arguments(self) -> None:
        with patch.object(cli, "launch", return_value=0) as launch:
            self.assertEqual(cli.main(["summarize this repo", "--verbose"]), 0)
        launch.assert_called_once_with(None, ["summarize this repo", "--verbose"])

    def test_default_launch_forwards_separator_arguments_in_order(self) -> None:
        with patch.object(cli, "launch", return_value=0) as launch:
            self.assertEqual(cli.main(["--model=gpt-direct", "--", "-p", "hello", "--verbose"]), 0)
        launch.assert_called_once_with("gpt-direct", ["-p", "hello", "--verbose"])

    def test_wt_launch_arguments_keep_the_claudex_model_and_claude_decoration(self) -> None:
        with patch.object(cli, "launch", return_value=0) as launch:
            self.assertEqual(cli.main(["-n", "tree label", "--model", "sol", "/color violet"]), 0)
        launch.assert_called_once_with("sol", ["-n", "tree label", "/color violet"])

    def test_wt_launch_second_separator_preserves_a_claude_model_option(self) -> None:
        with patch.object(cli, "launch", return_value=0) as launch:
            self.assertEqual(
                cli.main(["-n", "tree label", "--", "--model", "claude-choice", "/color violet"]),
                0,
            )
        launch.assert_called_once_with(
            None,
            ["-n", "tree label", "--model", "claude-choice", "/color violet"],
        )

    def test_separator_starts_an_implicit_run(self) -> None:
        with patch.object(cli, "launch", return_value=0) as launch:
            self.assertEqual(cli.main(["--", "--model", "claude-choice"]), 0)
        launch.assert_called_once_with(None, ["--model", "claude-choice"])

    def test_default_launch_without_arguments(self) -> None:
        with patch.object(cli, "launch", return_value=0) as launch:
            self.assertEqual(cli.main([]), 0)
        launch.assert_called_once_with(None, [])

    def test_explicit_run_preserves_colliding_claude_arguments(self) -> None:
        with patch.object(cli, "launch", return_value=0) as launch:
            result = self.runner.invoke(cli.app, ["run", "--", "--model", "claude-choice", "login"])
        self.assertEqual(result.exit_code, 0, result.output)
        launch.assert_called_once_with(None, ["--model", "claude-choice", "login"])

    def test_explicit_run_reserves_its_model_option(self) -> None:
        with patch.object(cli, "launch", return_value=0) as launch:
            result = self.runner.invoke(cli.app, ["run", "--model", "gpt-direct", "-p", "hello"])
        self.assertEqual(result.exit_code, 0, result.output)
        launch.assert_called_once_with("gpt-direct", ["-p", "hello"])

    def test_login_subcommand_uses_typer_command(self) -> None:
        with patch.object(cli, "login", return_value=0) as login:
            result = self.runner.invoke(cli.app, ["login"])
        self.assertEqual(result.exit_code, 0, result.output)
        login.assert_called_once_with(device_code=False)

    def test_login_device_code_flag_uses_explicit_legacy_route(self) -> None:
        with patch.object(cli, "login", return_value=0) as login:
            result = self.runner.invoke(cli.app, ["login", "--device-code"])
        self.assertEqual(result.exit_code, 0, result.output)
        login.assert_called_once_with(device_code=True)

    def test_models_subcommand_uses_typer_command(self) -> None:
        settings = ModelSettings()
        with patch.object(cli, "load_settings", return_value=settings), patch.object(cli, "print_models") as print_models:
            result = self.runner.invoke(cli.app, ["models"])
        self.assertEqual(result.exit_code, 0, result.output)
        print_models.assert_called_once_with(settings)

    def test_models_output_has_no_default_marker(self) -> None:
        output = StringIO()
        with redirect_stdout(output):
            cli.print_models(ModelSettings())
        self.assertNotIn("default", output.getvalue())

    def test_doctor_subcommand_uses_typer_command(self) -> None:
        with patch.object(cli, "doctor", return_value=0) as doctor:
            result = self.runner.invoke(cli.app, ["doctor"])
        self.assertEqual(result.exit_code, 0, result.output)
        doctor.assert_called_once_with()

    def test_help_is_available_for_app_and_run(self) -> None:
        run_help = self.runner.invoke(cli.app, ["run", "--help"])
        self.assertEqual(run_help.exit_code, 0, run_help.output)
        self.assertIn("--model", run_help.output)

        output = StringIO()
        with patch.object(cli, "launch") as launch, redirect_stdout(output):
            self.assertEqual(cli.main(["--help"]), 0)
        launch.assert_not_called()
        self.assertIn("login", output.getvalue())

    def test_main_returns_child_exit_status(self) -> None:
        with patch.object(cli, "launch", return_value=23) as launch:
            self.assertEqual(cli.main(["-p", "hello"]), 23)
        launch.assert_called_once_with(None, ["-p", "hello"])

    def test_main_returns_subcommand_exit_status(self) -> None:
        with patch.object(cli, "doctor", return_value=1) as doctor:
            self.assertEqual(cli.main(["doctor"]), 1)
        doctor.assert_called_once_with()

    def test_runtime_error_keeps_concise_error_format(self) -> None:
        output = StringIO()
        with patch.object(cli, "launch", side_effect=RuntimeError("Claude Code is not on PATH")):
            with redirect_stderr(output):
                self.assertEqual(cli.main(["-p", "hello"]), 2)
        self.assertEqual(output.getvalue(), "claudex: Claude Code is not on PATH\n")

    def test_claude_environment_uses_proxy_origin_role_models_and_explicit_override(self) -> None:
        settings = ModelSettings(
            models={"sol": "gpt-sol", "terra": "gpt-terra", "luna": "gpt-luna", "review": "gpt-review"},
            roles={"opus": "sol", "sonnet": "review", "haiku": "luna"},
        )
        result = cli.claude_environment({"ANTHROPIC_API_KEY": "old"}, settings, "gpt-direct", "http://127.0.0.1:3210", "key")
        self.assertEqual(result["ANTHROPIC_BASE_URL"], "http://127.0.0.1:3210")
        self.assertNotIn("ANTHROPIC_API_KEY", result)
        self.assertEqual(result["ANTHROPIC_MODEL"], "gpt-direct")
        self.assertEqual(result["ANTHROPIC_DEFAULT_OPUS_MODEL"], "gpt-sol")
        self.assertEqual(result["ANTHROPIC_DEFAULT_SONNET_MODEL"], "gpt-review")
        self.assertEqual(result["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "gpt-luna")
        self.assertNotIn("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY", result)

    def test_claude_environment_implicit_launch_omits_model(self) -> None:
        result = cli.claude_environment({}, ModelSettings(), None, "http://127.0.0.1:3210", "key")
        self.assertNotIn("ANTHROPIC_MODEL", result)

    def test_claude_environment_implicit_launch_preserves_inherited_model(self) -> None:
        result = cli.claude_environment(
            {"ANTHROPIC_MODEL": "caller-choice"}, ModelSettings(), None, "http://127.0.0.1:3210", "key"
        )
        self.assertEqual(result["ANTHROPIC_MODEL"], "caller-choice")

    def test_launch_implicit_model_uses_role_models_without_setting_anthropic_model(self) -> None:
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

        settings = ModelSettings(
            models={"sol": "gpt-sol", "terra": "gpt-terra", "luna": "gpt-luna"},
            roles={"opus": "sol", "sonnet": "terra", "haiku": "luna"},
        )
        with patch.object(cli.shutil, "which", return_value="/usr/bin/claude"), patch.object(
            cli, "load_settings", return_value=settings
        ), patch.object(cli, "ProxySupervisor", Supervisor), patch.object(cli, "run_child", return_value=23) as run_child:
            self.assertEqual(cli.launch(None, ["--resume"], env={}), 23)
        self.assertEqual(captured["extra_models"], ["gpt-sol", "gpt-terra", "gpt-luna"])
        self.assertNotIn("ANTHROPIC_MODEL", run_child.call_args.args[1])

    def test_launch_resolves_explicit_alias_and_supplies_role_models_to_gateway(self) -> None:
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

        settings = ModelSettings(
            models={"sol": "gpt-sol", "terra": "gpt-terra", "luna": "gpt-luna"},
            roles={"opus": "sol", "sonnet": "terra", "haiku": "luna"},
        )
        with patch.object(cli.shutil, "which", return_value="/usr/bin/claude"), patch.object(
            cli, "load_settings", return_value=settings
        ), patch.object(cli, "ProxySupervisor", Supervisor), patch.object(cli, "run_child", return_value=23) as run_child:
            self.assertEqual(cli.launch("sol", ["--resume"]), 23)
        self.assertEqual(captured["extra_models"], ["gpt-sol", "gpt-terra", "gpt-luna"])
        self.assertEqual(run_child.call_args.args[1]["ANTHROPIC_MODEL"], "gpt-sol")

    def test_launch_forwards_explicit_direct_model_to_gateway(self) -> None:
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
        self.assertEqual(run_child.call_args.args[1]["ANTHROPIC_MODEL"], "gpt-direct")

    def test_browser_login_uses_temporary_codex_home_and_converts_tokens(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            normal_home = Path(directory) / "normal-home"
            normal_auth = normal_home / ".codex" / "auth.json"
            normal_auth.parent.mkdir(parents=True)
            normal_auth.write_text('{"tokens": {"access_token": "do-not-use"}}')
            env = {
                "HOME": str(normal_home),
                "XDG_STATE_HOME": directory,
                "XDG_CONFIG_HOME": directory + "/config",
                "HTTPS_PROXY": "override-proxy",
            }
            observed: dict[str, str] = {}

            def complete_login(command: object, environment: dict[str, str]) -> int:
                self.assertEqual(command, ["codex", "login"])
                observed.update({name: environment[name] for name in ("PATH", "SSL_CERT_FILE", "BROWSER", "HTTPS_PROXY", "CODEX_HOME")})
                codex_home = Path(environment["CODEX_HOME"])
                self.assertTrue(codex_home.is_relative_to(Path(directory) / "claudex"))
                self.assertEqual((codex_home / "config.toml").read_text(), 'cli_auth_credentials_store = "file"\n')
                (codex_home / "auth.json").write_text(
                    json.dumps(
                        {
                            "tokens": {
                                "access_token": "access",
                                "refresh_token": "refresh",
                                "id_token": "id",
                                "account_id": "account",
                            }
                        }
                    )
                )
                return 0

            with patch.dict(os.environ, {"SSL_CERT_FILE": "inherited-cert", "BROWSER": "inherited-browser", "HTTPS_PROXY": "inherited-proxy"}):
                with patch.object(cli.shutil, "which", return_value="/usr/bin/codex"), patch.object(
                    cli, "run_child", side_effect=complete_login
                ):
                    self.assertEqual(cli.login(env), 0)
            token = Path(directory) / "claudex" / "chatgpt" / "auth.json"
            self.assertEqual(token.stat().st_mode & 0o777, 0o600)
            self.assertEqual(json.loads(token.read_text()), {
                "access_token": "access",
                "refresh_token": "refresh",
                "id_token": "id",
                "account_id": "account",
            })
            self.assertFalse(Path(observed["CODEX_HOME"]).exists())
            self.assertEqual(normal_auth.read_text(), '{"tokens": {"access_token": "do-not-use"}}')
            self.assertEqual(observed["PATH"], os.environ["PATH"])
            self.assertEqual(observed["SSL_CERT_FILE"], "inherited-cert")
            self.assertEqual(observed["BROWSER"], "inherited-browser")
            self.assertEqual(observed["HTTPS_PROXY"], "override-proxy")

    def test_device_code_route_stays_available(self) -> None:
        with patch.object(cli, "browser_login") as browser, patch.object(cli, "device_code_login", return_value=0) as device:
            self.assertEqual(cli.login(device_code=True), 0)
        browser.assert_not_called()
        device.assert_called_once_with(None)

    def test_device_code_login_uses_isolated_chatgpt_token_dir_and_private_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            env = {"XDG_STATE_HOME": directory, "XDG_CONFIG_HOME": directory + "/config", "HTTPS_PROXY": "override-proxy"}
            from litellm.llms.chatgpt.authenticator import Authenticator

            observed: dict[str, str] = {}

            def get_access_token(_self: object) -> str:
                observed.update({name: os.environ[name] for name in ("PATH", "SSL_CERT_FILE", "BROWSER", "HTTPS_PROXY", "CHATGPT_TOKEN_DIR")})
                path = Path(os.environ["CHATGPT_TOKEN_DIR"]) / "auth.json"
                path.write_text("{}")
                path.chmod(0o644)
                return "token"

            with patch.dict(os.environ, {"SSL_CERT_FILE": "inherited-cert", "BROWSER": "inherited-browser", "HTTPS_PROXY": "inherited-proxy"}):
                before = dict(os.environ)
                with patch.object(Authenticator, "get_access_token", get_access_token):
                    self.assertEqual(cli.login(env, device_code=True), 0)
                self.assertEqual(dict(os.environ), before)
            token = Path(directory) / "claudex" / "chatgpt" / "auth.json"
            self.assertEqual(token.stat().st_mode & 0o777, 0o600)
            self.assertEqual(observed["PATH"], before["PATH"])
            self.assertEqual(observed["SSL_CERT_FILE"], "inherited-cert")
            self.assertEqual(observed["BROWSER"], "inherited-browser")
            self.assertEqual(observed["HTTPS_PROXY"], "override-proxy")

    def test_browser_login_reports_missing_codex(self) -> None:
        with patch.object(cli.shutil, "which", return_value=None):
            with self.assertRaisesRegex(RuntimeError, "Codex CLI is not on PATH"):
                cli.browser_login({"PATH": ""})

    def test_browser_login_reports_child_failure_and_cleans_up(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            observed: dict[str, Path] = {}

            def fail_login(_command: object, environment: dict[str, str]) -> int:
                observed["home"] = Path(environment["CODEX_HOME"])
                return 17

            with patch.object(cli.shutil, "which", return_value="/usr/bin/codex"), patch.object(
                cli, "run_child", side_effect=fail_login
            ):
                with self.assertRaisesRegex(RuntimeError, "exited with status 17"):
                    cli.browser_login({"XDG_STATE_HOME": directory})
            self.assertFalse(observed["home"].exists())

    def test_browser_login_rejects_malformed_codex_auth(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            def malformed_login(_command: object, environment: dict[str, str]) -> int:
                Path(environment["CODEX_HOME"], "auth.json").write_text('{"tokens": {"access_token": "access"}}')
                return 0

            with patch.object(cli.shutil, "which", return_value="/usr/bin/codex"), patch.object(
                cli, "run_child", side_effect=malformed_login
            ):
                with self.assertRaisesRegex(RuntimeError, "did not write valid login credentials"):
                    cli.browser_login({"XDG_STATE_HOME": directory})

    def test_doctor_checks_for_codex_cli(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            def which(command: str, path: str | None = None) -> str | None:
                return "/usr/bin/" + command if command in {"claude", "codex"} else None

            output = StringIO()
            with patch.object(cli.platform, "system", return_value="Darwin"), patch.object(cli.shutil, "which", side_effect=which), patch.object(
                cli, "version", return_value="test"
            ), patch.object(cli, "compatibility_status", return_value=(True, "available")), redirect_stdout(output):
                self.assertEqual(cli.doctor({"XDG_CONFIG_HOME": directory + "/config", "XDG_STATE_HOME": directory + "/state"}), 1)
            self.assertIn("OK   codex: /usr/bin/codex", output.getvalue())
