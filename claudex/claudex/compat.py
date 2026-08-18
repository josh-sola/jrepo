from __future__ import annotations

import importlib
import importlib.metadata
import inspect
import sys
from collections.abc import Callable
from typing import Any


EXPECTED_LITELLM_VERSION = "1.97.0"


def map_system_message_content(messages: list[dict[str, Any]]) -> list[dict[str, Any]]:
    result: list[dict[str, Any]] = []
    for index, message in enumerate(messages):
        if message["role"] != "system":
            result.append(message)
            continue
        if index < len(messages) - 1:
            next_message = messages[index + 1]
            if next_message["role"] in {"user", "assistant"}:
                if isinstance(message["content"], list) or isinstance(next_message["content"], list):
                    next_message["content"] = _content_as_blocks(message["content"]) + _content_as_blocks(next_message["content"])
                elif message["content"] is not None:
                    if next_message["content"] is None:
                        next_message["content"] = message["content"]
                    else:
                        next_message["content"] = message["content"] + " " + next_message["content"]
            elif next_message["role"] == "system":
                result.append({"role": "user", "content": message["content"]})
        else:
            result.append({"role": "user", "content": message["content"]})
    return result


def _content_as_blocks(content: Any) -> list[Any]:
    if isinstance(content, list):
        return content
    if content is None:
        return []
    return [{"type": "text", "text": content}]


def compatibility_status() -> tuple[bool, str]:
    try:
        factory = importlib.import_module("litellm.litellm_core_utils.prompt_templates.factory")
    except ImportError as error:
        return False, f"LiteLLM is not importable: {error}"
    version = importlib.metadata.version("litellm")
    if version != EXPECTED_LITELLM_VERSION:
        return False, f"LiteLLM {version} is unsupported; expected {EXPECTED_LITELLM_VERSION}"
    target = getattr(factory, "map_system_message_pt", None)
    if not callable(target):
        return False, "LiteLLM has no map_system_message_pt compatibility target"
    if getattr(target, "__claudex_patch__", False):
        return True, "LiteLLM compatibility patch is installed"
    if list(inspect.signature(target).parameters) != ["messages"]:
        return False, "LiteLLM map_system_message_pt has an unexpected signature"
    if "m[\"content\"] + \" \" + next_m[\"content\"]" not in inspect.getsource(target):
        return True, "LiteLLM already has content-block system-message support"
    return True, "LiteLLM compatibility patch is available"


def install_compatibility_patch() -> None:
    available, message = compatibility_status()
    if not available:
        raise RuntimeError(message)
    factory = importlib.import_module("litellm.litellm_core_utils.prompt_templates.factory")
    original = factory.map_system_message_pt
    if getattr(original, "__claudex_patch__", False):
        return
    if "m[\"content\"] + \" \" + next_m[\"content\"]" not in inspect.getsource(original):
        return
    patched = _patched_map_system_message_pt(original)
    factory.map_system_message_pt = patched
    _replace_imported_references(original, patched)


def _patched_map_system_message_pt(original: Callable[..., Any]) -> Callable[..., Any]:
    def patched(*args: Any, **kwargs: Any) -> Any:
        messages = kwargs.get("messages", args[0] if args else None)
        if not isinstance(messages, list):
            return original(*args, **kwargs)
        return map_system_message_content(messages)

    setattr(patched, "__claudex_patch__", True)
    return patched


def _replace_imported_references(original: Callable[..., Any], patched: Callable[..., Any]) -> None:
    for module_name, module in tuple(sys.modules.items()):
        if (module_name != "litellm" and not module_name.startswith("litellm.")) or module is None:
            continue
        for name, value in vars(module).items():
            if value is original:
                setattr(module, name, patched)
