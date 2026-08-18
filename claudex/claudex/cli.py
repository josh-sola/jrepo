from __future__ import annotations

import json
import os
import platform
import shutil
import sys
import tempfile
from contextlib import contextmanager
from importlib.metadata import version
from pathlib import Path
from typing import Annotated

import click
import typer

from .compat import compatibility_status
from .config import (
    config_path,
    is_private,
    litellm_environment,
    load_settings,
    oauth_auth_file,
    private_directory,
    private_file,
    state_dir,
)
from .models import ModelSettings
from .supervisor import ProxySupervisor, run_child


app = typer.Typer(
    add_completion=False,
    help="Run Claude Code through a local ChatGPT-backed gateway.",
)
PASSTHROUGH = {"allow_extra_args": True, "ignore_unknown_options": True}
SUBCOMMANDS = {"run", "login", "models", "doctor"}


def main(argv: list[str] | None = None) -> int:
    arguments = _route_arguments(list(sys.argv[1:] if argv is None else argv))
    try:
        result = app(args=arguments, prog_name="claudex", standalone_mode=False)
        return int(result or 0)
    except click.exceptions.Exit as error:
        return error.exit_code
    except click.ClickException as error:
        error.show()
        return error.exit_code


def _route_arguments(arguments: list[str]) -> list[str]:
    if not arguments:
        return ["run"]
    if arguments[0] in SUBCOMMANDS or arguments[0] == "--help":
        return arguments
    return ["run", *arguments]


def _launch_or_exit(model: str | None, claude_args: list[str]) -> int:
    try:
        return launch(model, claude_args)
    except (ValueError, RuntimeError) as error:
        print(f"claudex: {error}", file=sys.stderr)
        raise typer.Exit(2) from error


@app.command(context_settings=PASSTHROUGH, help="Run Claude Code and pass through extra arguments.")
def run(
    context: typer.Context,
    model: Annotated[str | None, typer.Option("--model", help="claudex alias or upstream model ID")] = None,
) -> int:
    return _launch_or_exit(model, list(context.args))


@app.command("login", help="Sign in to ChatGPT in your browser.")
def login_command(
    device_code: Annotated[
        bool, typer.Option("--device-code", help="Use LiteLLM's device authorization flow instead of browser login.")
    ] = False,
) -> int:
    try:
        return login(device_code=device_code)
    except (ValueError, RuntimeError) as error:
        print(f"claudex: {error}", file=sys.stderr)
        raise typer.Exit(2) from error


@app.command(help="Show the configured model aliases and role mappings.")
def models() -> None:
    try:
        print_models(load_settings())
    except (ValueError, RuntimeError) as error:
        print(f"claudex: {error}", file=sys.stderr)
        raise typer.Exit(2) from error


@app.command("doctor", help="Check the local setup without making a model request.")
def doctor_command() -> int:
    try:
        return doctor()
    except (ValueError, RuntimeError) as error:
        print(f"claudex: {error}", file=sys.stderr)
        raise typer.Exit(2) from error


def launch(model: str | None, claude_args: list[str], env: dict[str, str] | None = None) -> int:
    if shutil.which("claude") is None:
        raise RuntimeError("Claude Code is not on PATH. Install it, then run 'claudex doctor'.")
    settings = load_settings(env)
    selected = settings.resolve(model)
    gateway_models = [selected, *(settings.role_model(role) for role in ("opus", "sonnet", "haiku"))]
    with ProxySupervisor(settings, env=env, extra_models=gateway_models) as proxy:
        claude_env = claude_environment(dict(env or os.environ), settings, selected, proxy.base_url, proxy.token)
        return run_child(["claude", *claude_args], claude_env)


def claude_environment(
    environment: dict[str, str], settings: ModelSettings, selected: str, base_url: str, token: str
) -> dict[str, str]:
    result = dict(environment)
    result.pop("ANTHROPIC_API_KEY", None)
    result.update(
        {
            "ANTHROPIC_BASE_URL": base_url,
            "ANTHROPIC_AUTH_TOKEN": token,
            "ANTHROPIC_MODEL": selected,
            "ANTHROPIC_DEFAULT_OPUS_MODEL": settings.role_model("opus"),
            "ANTHROPIC_DEFAULT_SONNET_MODEL": settings.role_model("sonnet"),
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": settings.role_model("haiku"),
            "CLAUDE_CODE_ATTRIBUTION_HEADER": "0",
            "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING": "1",
            "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS": "1",
        }
    )
    return result


def login(env: dict[str, str] | None = None, device_code: bool = False) -> int:
    if device_code:
        return device_code_login(env)
    return browser_login(env)


def browser_login(env: dict[str, str] | None = None) -> int:
    environment = {**os.environ, **(env or {})}
    if shutil.which("codex", path=environment.get("PATH")) is None:
        raise RuntimeError("Codex CLI is not on PATH. Install it, then run 'claudex login' again.")
    runtime = private_directory(state_dir(environment))
    print("Starting Codex browser login.")
    with tempfile.TemporaryDirectory(prefix="codex-login-", dir=runtime) as temporary:
        codex_home = Path(temporary)
        _write_private_text(codex_home / "config.toml", 'cli_auth_credentials_store = "file"\n')
        codex_environment = dict(environment)
        codex_environment["CODEX_HOME"] = str(codex_home)
        with private_umask():
            exit_code = run_child(["codex", "login"], codex_environment)
        if exit_code != 0:
            raise RuntimeError(f"Codex login exited with status {exit_code}. Complete the browser sign-in and try again.")
        record = _litellm_auth_record(_read_codex_tokens(codex_home / "auth.json"))
        _write_private_json(oauth_auth_file(environment), record)
    return 0


def device_code_login(env: dict[str, str] | None = None) -> int:
    print("Opening LiteLLM's ChatGPT device login. Complete it in your browser.")
    environment = litellm_environment({**os.environ, **(env or {})})
    with applied_environment(environment), private_umask():
        from litellm.llms.chatgpt.authenticator import Authenticator

        Authenticator().get_access_token()
    auth = oauth_auth_file(environment)
    if auth.exists():
        private_file(auth)
    return 0


def _read_codex_tokens(path: Path) -> dict[str, object]:
    try:
        with path.open(encoding="utf-8") as file:
            auth = json.load(file)
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError("Codex did not write valid login credentials. Try 'claudex login' again.") from error
    if not isinstance(auth, dict) or not isinstance(tokens := auth.get("tokens"), dict):
        raise RuntimeError("Codex did not write valid login credentials. Try 'claudex login' again.")
    return tokens


def _litellm_auth_record(tokens: dict[str, object]) -> dict[str, str]:
    record: dict[str, str] = {}
    for name in ("access_token", "refresh_token", "id_token"):
        value = tokens.get(name)
        if not isinstance(value, str) or not value:
            raise RuntimeError("Codex did not write valid login credentials. Try 'claudex login' again.")
        record[name] = value
    account_id = tokens.get("account_id")
    if isinstance(account_id, str) and account_id:
        record["account_id"] = account_id
    return record


def _write_private_text(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as file:
        file.write(content)


def _write_private_json(path: Path, value: dict[str, str]) -> None:
    private_directory(path.parent)
    descriptor, temporary = tempfile.mkstemp(prefix=".auth-", dir=path.parent)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as file:
            json.dump(value, file)
            file.flush()
            os.fsync(file.fileno())
        os.replace(temporary, path)
        private_file(path)
    except BaseException:
        try:
            os.close(descriptor)
        except OSError:
            pass
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass
        raise


@contextmanager
def applied_environment(values: dict[str, str]):
    previous = dict(os.environ)
    os.environ.clear()
    os.environ.update(values)
    try:
        yield
    finally:
        os.environ.clear()
        os.environ.update(previous)


@contextmanager
def private_umask():
    previous = os.umask(0o077)
    try:
        yield
    finally:
        os.umask(previous)


def print_models(settings: ModelSettings) -> None:
    print("Aliases:")
    for alias, upstream in settings.models.items():
        marker = " (default)" if settings.resolve() == upstream else ""
        print(f"  {alias:<8} {upstream}{marker}")
    print("Roles:")
    for role in ("opus", "sonnet", "haiku"):
        source = settings.roles[role]
        print(f"  {role:<8} {source} -> {settings.role_model(role)}")


def doctor(env: dict[str, str] | None = None) -> int:
    values = dict(env or os.environ)
    failures: list[str] = []
    system = platform.system()
    if system in {"Darwin", "Linux"}:
        print(f"OK   platform: {system}")
    else:
        failures.append(f"unsupported platform: {system}; claudex supports macOS and Linux")
    claude = shutil.which("claude")
    if claude:
        print(f"OK   claude: {claude}")
    else:
        failures.append("Claude Code is not on PATH")
    codex = shutil.which("codex", path=values.get("PATH"))
    if codex:
        print(f"OK   codex: {codex}")
    else:
        failures.append("Codex CLI is not on PATH; install it before running 'claudex login'")
    try:
        print(f"OK   LiteLLM: {version('litellm')}")
    except ImportError:
        failures.append("LiteLLM is not installed correctly; reinstall claudex with uv")
    available, patch_message = compatibility_status()
    if available:
        print(f"OK   patch: {patch_message}")
    else:
        failures.append(patch_message)
    path = config_path(values)
    if path.exists() and not is_private(path, 0o600):
        failures.append(f"config permissions are too open: {path}; run chmod 600 {path}")
    elif path.exists():
        print(f"OK   config permissions: {path}")
    else:
        print(f"OK   config: defaults in use ({path} does not exist)")
    auth = oauth_auth_file(values)
    if auth.exists() and not is_private(auth, 0o600):
        failures.append(f"OAuth token permissions are too open: {auth}; run chmod 600 {auth}")
    elif auth.exists():
        print(f"OK   ChatGPT login: {auth}")
    else:
        failures.append("not logged in; run 'claudex login'")
    if failures:
        for failure in failures:
            print(f"FAIL {failure}")
        return 1
    return 0
