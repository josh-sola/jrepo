from __future__ import annotations

import re
import secrets
from pathlib import Path

from .models import ModelSettings


MODEL_ID = re.compile(r"^[A-Za-z0-9._/-]+$")


def gateway_yaml(
    settings: ModelSettings, master_key_env: str = "CLAUDEX_LITELLM_MASTER_KEY", extra_models: list[str] | None = None
) -> str:
    unique_models = list(dict.fromkeys([*settings.models.values(), *(extra_models or [])]))
    invalid = [model for model in unique_models if not MODEL_ID.fullmatch(model)]
    if invalid:
        raise ValueError(f"invalid model ID: {invalid[0]!r}")
    lines = ["model_list:"]
    for upstream in unique_models:
        lines.extend(
            [
                f"  - model_name: {upstream}",
                "    litellm_params:",
                f"      model: chatgpt/{upstream}",
                "      drop_params: true",
                "      supports_system_message: false",
                "    model_info:",
                "      mode: responses",
            ]
        )
    lines.extend(["general_settings:", f"  master_key: os.environ/{master_key_env}"])
    return "\n".join(lines) + "\n"


def write_gateway_config(directory: Path, settings: ModelSettings, extra_models: list[str] | None = None) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    directory.chmod(0o700)
    path = directory / "config.yaml"
    path.write_text(gateway_yaml(settings, extra_models=extra_models), encoding="utf-8")
    path.chmod(0o600)
    return path


def new_master_key() -> str:
    return f"sk-claudex-{secrets.token_urlsafe(32)}"
