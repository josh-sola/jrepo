from __future__ import annotations

import os
import platform
import shutil
import sys
from contextlib import contextmanager
from importlib.metadata import version
from typing import Annotated

import click
import typer

from .compat import compatibility_status
from .config import config_path, is_private, litellm_environment, load_settings, oauth_auth_file, private_file
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


@app.command("login", help="Sign in to ChatGPT with device authorization.")
def login_command() -> int:
    try:
        return login()
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


def login(env: dict[str, str] | None = None) -> int:
    print("Opening LiteLLM's ChatGPT device login. Complete it in your browser.")
    environment = litellm_environment({**os.environ, **(env or {})})
    with applied_environment(environment), private_umask():
        from litellm.llms.chatgpt.authenticator import Authenticator

        Authenticator().get_access_token()
    auth = oauth_auth_file(environment)
    if auth.exists():
        private_file(auth)
    return 0


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
