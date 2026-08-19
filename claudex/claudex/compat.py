from __future__ import annotations

import importlib
import importlib.metadata
import inspect
import json
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
        streaming = importlib.import_module("litellm.responses.streaming_iterator")
    except ImportError as error:
        return False, f"LiteLLM is not importable: {error}"
    version = importlib.metadata.version("litellm")
    if version != EXPECTED_LITELLM_VERSION:
        return False, f"LiteLLM {version} is unsupported; expected {EXPECTED_LITELLM_VERSION}"
    system_message_target = getattr(factory, "map_system_message_pt", None)
    if not callable(system_message_target):
        return False, "LiteLLM has no map_system_message_pt compatibility target"
    system_message_patched = getattr(system_message_target, "__claudex_patch__", False)
    if not system_message_patched and list(inspect.signature(system_message_target).parameters) != ["messages"]:
        return False, "LiteLLM map_system_message_pt has an unexpected signature"

    stream_target = getattr(streaming, "BaseResponsesAPIStreamingIterator", None)
    process_chunk = getattr(stream_target, "_process_chunk", None)
    if not callable(process_chunk):
        return False, "LiteLLM has no Responses stream compatibility target"
    stream_patched = getattr(process_chunk, "__claudex_patch__", False)
    if not stream_patched and list(inspect.signature(process_chunk).parameters) != ["self", "chunk"]:
        return False, "LiteLLM Responses stream target has an unexpected signature"

    system_message_patch_needed = not system_message_patched and (
        "m[\"content\"] + \" \" + next_m[\"content\"]" in inspect.getsource(system_message_target)
    )
    stream_patch_needed = False
    if not stream_patched:
        stream_source = inspect.getsource(process_chunk)
        stream_has_known_source = all(
            marker in stream_source
            for marker in (
                "parsed_chunk: Final = json.loads(chunk)",
                "ResponsesAPIStreamEvents.OUTPUT_TEXT_DELTA",
                "ResponsesAPIStreamEvents.RESPONSE_COMPLETED",
                "return openai_responses_api_chunk",
            )
        )
        if not stream_has_known_source:
            return False, "LiteLLM Responses stream target has unexpected source"
        stream_patch_needed = True

    if system_message_patched and stream_patched:
        return True, "LiteLLM compatibility patch is installed"
    if not system_message_patch_needed and not stream_patch_needed:
        return True, "LiteLLM already has the required compatibility support"
    return True, "LiteLLM compatibility patch is available"


def install_compatibility_patch() -> None:
    available, message = compatibility_status()
    if not available:
        raise RuntimeError(message)
    factory = importlib.import_module("litellm.litellm_core_utils.prompt_templates.factory")
    original = factory.map_system_message_pt
    if not getattr(original, "__claudex_patch__", False) and "m[\"content\"] + \" \" + next_m[\"content\"]" in inspect.getsource(original):
        patched = _patched_map_system_message_pt(original)
        factory.map_system_message_pt = patched
        _replace_imported_references(original, patched)

    streaming = importlib.import_module("litellm.responses.streaming_iterator")
    stream_target = streaming.BaseResponsesAPIStreamingIterator
    process_chunk = stream_target._process_chunk
    if getattr(process_chunk, "__claudex_patch__", False):
        return
    stream_target._process_chunk = _patched_process_chunk(process_chunk)


def _patched_map_system_message_pt(original: Callable[..., Any]) -> Callable[..., Any]:
    def patched(*args: Any, **kwargs: Any) -> Any:
        messages = kwargs.get("messages", args[0] if args else None)
        if not isinstance(messages, list):
            return original(*args, **kwargs)
        return map_system_message_content(messages)

    setattr(patched, "__claudex_patch__", True)
    return patched


def _patched_process_chunk(original: Callable[..., Any]) -> Callable[..., Any]:
    def patched(iterator: Any, chunk: str) -> Any:
        try:
            parsed_chunk = json.loads(chunk)
        except (TypeError, json.JSONDecodeError):
            return original(iterator, chunk)
        if not isinstance(parsed_chunk, dict):
            return original(iterator, chunk)

        if parsed_chunk.get("type") == "response.completed":
            response = parsed_chunk.get("response")
            if isinstance(response, dict) and not response.get("output"):
                recovered_output = _streamed_output_items(iterator)
                if recovered_output:
                    parsed_chunk = dict(parsed_chunk)
                    response = dict(response)
                    response["output"] = recovered_output
                    parsed_chunk["response"] = response
                    chunk = json.dumps(parsed_chunk)
            try:
                return original(iterator, chunk)
            finally:
                _clear_streamed_output_items(iterator)

        _record_streamed_output_item(iterator, parsed_chunk)
        return original(iterator, chunk)

    setattr(patched, "__claudex_patch__", True)
    return patched


def _record_streamed_output_item(iterator: Any, parsed_chunk: dict[str, Any]) -> None:
    from litellm.types.llms.openai import ResponsesAPIStreamEvents

    state = getattr(iterator, "_claudex_streamed_output_items", None)
    if not isinstance(state, dict):
        state = {"added": {}, "done": {}, "indexes": {}}
        setattr(iterator, "_claudex_streamed_output_items", state)
    added_items = state["added"]
    done_items = state["done"]
    item_indexes = state["indexes"]
    event_type = parsed_chunk.get("type")

    if event_type == ResponsesAPIStreamEvents.OUTPUT_ITEM_DONE:
        item = parsed_chunk.get("item")
        output_index = _output_index(parsed_chunk, len(done_items))
        if isinstance(item, dict) and output_index is not None:
            done_items[output_index] = _copy_output_item(item)
        return

    if event_type == ResponsesAPIStreamEvents.OUTPUT_ITEM_ADDED:
        item = parsed_chunk.get("item")
        output_index = _output_index(parsed_chunk, len(added_items))
        if not isinstance(item, dict) or output_index is None:
            return
        added_items[output_index] = _copy_output_item(item)
        item_id = item.get("id")
        if isinstance(item_id, str):
            item_indexes[item_id] = output_index
        return

    output_index = _output_index(parsed_chunk, None)
    if output_index is None:
        item_id = parsed_chunk.get("item_id")
        if isinstance(item_id, str):
            output_index = item_indexes.get(item_id)
    if output_index is None or output_index not in added_items:
        return
    item = added_items[output_index]
    if event_type == ResponsesAPIStreamEvents.CONTENT_PART_ADDED:
        part = parsed_chunk.get("part")
        if isinstance(part, dict):
            _record_content_part(item, parsed_chunk, part)
    elif event_type == ResponsesAPIStreamEvents.OUTPUT_TEXT_DELTA:
        delta = parsed_chunk.get("delta")
        if isinstance(delta, str):
            _append_output_text_delta(item, parsed_chunk, delta)
    elif event_type == ResponsesAPIStreamEvents.FUNCTION_CALL_ARGUMENTS_DELTA:
        delta = parsed_chunk.get("delta")
        if isinstance(delta, str) and item.get("type") == "function_call":
            arguments = item.get("arguments")
            item["arguments"] = (arguments if isinstance(arguments, str) else "") + delta


def _streamed_output_items(iterator: Any) -> list[dict[str, Any]]:
    state = getattr(iterator, "_claudex_streamed_output_items", None)
    if not isinstance(state, dict):
        return []
    added_items = state.get("added")
    done_items = state.get("done")
    if not isinstance(added_items, dict) or not isinstance(done_items, dict):
        return []
    output_items = {**added_items, **done_items}
    return [item for _, item in sorted(output_items.items())]


def _clear_streamed_output_items(iterator: Any) -> None:
    if hasattr(iterator, "_claudex_streamed_output_items"):
        delattr(iterator, "_claudex_streamed_output_items")


def _copy_output_item(item: dict[str, Any]) -> dict[str, Any]:
    copied = dict(item)
    content = item.get("content")
    if isinstance(content, list):
        copied["content"] = [dict(part) if isinstance(part, dict) else part for part in content]
    return copied


def _output_index(parsed_chunk: dict[str, Any], default: int | None) -> int | None:
    raw_index = parsed_chunk.get("output_index", default)
    try:
        output_index = int(raw_index)
    except (TypeError, ValueError):
        return None
    return output_index if output_index >= 0 else None


def _content_index(parsed_chunk: dict[str, Any], default: int) -> int | None:
    raw_index = parsed_chunk.get("content_index", default)
    try:
        content_index = int(raw_index)
    except (TypeError, ValueError):
        return None
    return content_index if content_index >= 0 else None


def _record_content_part(item: dict[str, Any], parsed_chunk: dict[str, Any], part: dict[str, Any]) -> None:
    content = item.get("content")
    if not isinstance(content, list):
        return
    content_index = _content_index(parsed_chunk, len(content))
    if content_index is None:
        return
    while len(content) <= content_index:
        content.append({})
    content[content_index] = dict(part)


def _append_output_text_delta(item: dict[str, Any], parsed_chunk: dict[str, Any], delta: str) -> None:
    content = item.get("content")
    if not isinstance(content, list):
        return
    content_index = _content_index(parsed_chunk, len(content))
    if content_index is None:
        return
    while len(content) <= content_index:
        content.append({"type": "output_text", "text": "", "annotations": []})
    part = content[content_index]
    if not isinstance(part, dict):
        part = {"type": "output_text", "text": "", "annotations": []}
        content[content_index] = part
    existing_text = part.get("text")
    part["type"] = "output_text"
    part["text"] = (existing_text if isinstance(existing_text, str) else "") + delta
    part.setdefault("annotations", [])


def _replace_imported_references(original: Callable[..., Any], patched: Callable[..., Any]) -> None:
    for module_name, module in tuple(sys.modules.items()):
        if (module_name != "litellm" and not module_name.startswith("litellm.")) or module is None:
            continue
        for name, value in vars(module).items():
            if value is original:
                setattr(module, name, patched)
