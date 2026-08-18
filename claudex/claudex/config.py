from __future__ import annotations

import os
import stat
import tomllib
from pathlib import Path

from .models import DEFAULT_MODELS, DEFAULT_ROLES, ModelSettings


def config_dir(env: dict[str, str] | None = None) -> Path:
    values = env or os.environ
    return Path(values.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "claudex"


def state_dir(env: dict[str, str] | None = None) -> Path:
    values = env or os.environ
    return Path(values.get("XDG_STATE_HOME", Path.home() / ".local" / "state")) / "claudex"


def private_directory(path: Path) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    path.chmod(0o700)
    return path


def private_file(path: Path) -> None:
    path.chmod(0o600)


def is_private(path: Path, required: int) -> bool:
    try:
        return stat.S_IMODE(path.stat().st_mode) & 0o077 == 0 and stat.S_IMODE(path.stat().st_mode) & required == required
    except FileNotFoundError:
        return False


def config_path(env: dict[str, str] | None = None) -> Path:
    return config_dir(env) / "config.toml"


def load_settings(env: dict[str, str] | None = None) -> ModelSettings:
    path = config_path(env)
    if not path.exists():
        return ModelSettings()
    with path.open("rb") as file:
        raw = tomllib.load(file)
    models = dict(DEFAULT_MODELS)
    models.update(_string_table(raw.get("models", {}), "models"))
    default = raw.get("default", "terra")
    if not isinstance(default, str):
        raise ValueError("config key 'default' must be a string")
    roles = dict(DEFAULT_ROLES)
    roles.update(_string_table(raw.get("roles", {}), "roles"))
    for role in DEFAULT_ROLES:
        if role not in roles:
            raise ValueError(f"config roles must include '{role}'")
    return ModelSettings(models=models, default=default, roles=roles)


def _string_table(value: object, name: str) -> dict[str, str]:
    if not isinstance(value, dict) or not all(isinstance(key, str) and isinstance(item, str) for key, item in value.items()):
        raise ValueError(f"config key '{name}' must be a table of strings")
    return value


def litellm_environment(base: dict[str, str] | None = None) -> dict[str, str]:
    values = dict(base or os.environ)
    home = private_directory(state_dir(values) / "litellm-home")
    config_home = private_directory(home / ".config")
    values["HOME"] = str(home)
    values["XDG_CONFIG_HOME"] = str(config_home)
    values["XDG_DATA_HOME"] = str(private_directory(home / ".local" / "share"))
    values["CHATGPT_TOKEN_DIR"] = str(private_directory(state_dir(values) / "chatgpt"))
    return values


def oauth_auth_file(env: dict[str, str] | None = None) -> Path:
    values = litellm_environment(env)
    return Path(values["CHATGPT_TOKEN_DIR"]) / "auth.json"
