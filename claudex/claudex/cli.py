from __future__ import annotations

import os
import platform
import shutil
import sys
from contextlib import contextmanager
from importlib.metadata import version

from .compat import compatibility_status
from .config import config_path, is_private, litellm_environment, load_settings, oauth_auth_file, private_file
from .models import ModelSettings
from .supervisor import ProxySupervisor, run_child


def main(argv: list[str] | None = None) -> int:
    arguments = list(sys.argv[1:] if argv is None else argv)
    try:
        if arguments[:1] == ["login"]:
            if len(arguments) != 1:
                raise ValueError("usage: claudex login")
            return login()
        if arguments[:1] == ["models"]:
            if len(arguments) != 1:
                raise ValueError("usage: claudex models")
            print_models(load_settings())
            return 0
        if arguments[:1] == ["doctor"]:
            if len(arguments) != 1:
                raise ValueError("usage: claudex doctor")
            return doctor()
        model, claude_args = parse_launch_args(arguments)
        return launch(model, claude_args)
    except (ValueError, RuntimeError) as error:
        print(f"claudex: {error}", file=sys.stderr)
        return 2


def parse_launch_args(arguments: list[str]) -> tuple[str | None, list[str]]:
    model: str | None = None
    remaining: list[str] = []
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument == "--":
            remaining.extend(arguments[index + 1 :])
            break
        if argument == "--model":
            index += 1
            if index >= len(arguments):
                raise ValueError("--model needs an alias or upstream model ID")
            model = arguments[index]
        elif argument.startswith("--model="):
            model = argument.removeprefix("--model=")
            if not model:
                raise ValueError("--model needs an alias or upstream model ID")
        else:
            remaining.append(argument)
        index += 1
    return model, remaining


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
            "ANTHROPIC_MODEL": f"claudex/{selected}",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": f"claudex/{settings.role_model('opus')}",
            "ANTHROPIC_DEFAULT_SONNET_MODEL": f"claudex/{settings.role_model('sonnet')}",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": f"claudex/{settings.role_model('haiku')}",
            "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY": "1",
            "CLAUDE_CODE_ATTRIBUTION_HEADER": "0",
            "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING": "1",
            "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS": "1",
        }
    )
    return result


def login(env: dict[str, str] | None = None) -> int:
    print("Opening LiteLLM's ChatGPT device login. Complete it in your browser.")
    environment = litellm_environment(env)
    with applied_environment(environment), private_umask():
        from litellm.llms.chatgpt.authenticator import Authenticator

        Authenticator().get_access_token()
    auth = oauth_auth_file(env)
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
