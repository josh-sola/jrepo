from __future__ import annotations

import json
import sys
import types
import unittest

from claudex.compat import _patched_process_chunk, _replace_imported_references, compatibility_status, install_compatibility_patch, map_system_message_content


class CompatibilityTests(unittest.TestCase):
    def test_string_content_keeps_litellm_behavior(self) -> None:
        messages = [{"role": "system", "content": "You help."}, {"role": "user", "content": "Hi"}]
        self.assertEqual(map_system_message_content(messages), [{"role": "user", "content": "You help. Hi"}])

    def test_block_content_preserves_non_text_blocks(self) -> None:
        image = {"type": "image", "source": {"type": "base64", "data": "abc"}}
        messages = [
            {"role": "system", "content": [{"type": "text", "text": "Look."}, image]},
            {"role": "user", "content": [{"type": "text", "text": "Okay"}]},
        ]
        result = map_system_message_content(messages)
        self.assertEqual(result[0]["content"][1], image)
        self.assertEqual(len(result[0]["content"]), 3)

    def test_mixed_content_becomes_text_block_and_list(self) -> None:
        messages = [{"role": "system", "content": "Rules"}, {"role": "user", "content": [{"type": "text", "text": "Go"}]}]
        self.assertEqual(
            map_system_message_content(messages)[0]["content"],
            [{"type": "text", "text": "Rules"}, {"type": "text", "text": "Go"}],
        )

    def test_tool_call_only_assistant_keeps_fields_when_system_content_is_merged(self) -> None:
        tool_call = {"id": "call_1", "type": "function", "function": {"name": "lookup", "arguments": "{}"}}
        messages = [
            {"role": "system", "content": "Use tools."},
            {"role": "assistant", "content": None, "tool_calls": [tool_call]},
        ]
        result = map_system_message_content(messages)
        self.assertEqual(result, [{"role": "assistant", "content": "Use tools.", "tool_calls": [tool_call]}])

    def test_block_system_content_does_not_create_a_none_text_block(self) -> None:
        messages = [
            {"role": "system", "content": [{"type": "text", "text": "Use tools."}]},
            {"role": "assistant", "content": None, "tool_calls": [{"id": "call_1"}]},
        ]
        result = map_system_message_content(messages)
        self.assertEqual(result[0]["content"], [{"type": "text", "text": "Use tools."}])

    def test_replace_imported_references_updates_loaded_litellm_module(self) -> None:
        module = types.ModuleType("litellm.synthetic_reference")

        def original() -> None:
            pass

        def patched() -> None:
            pass

        module.reference = original
        sys.modules[module.__name__] = module
        try:
            _replace_imported_references(original, patched)
            self.assertIs(module.reference, patched)
        finally:
            sys.modules.pop(module.__name__, None)

    def test_patch_replaces_factory_and_main_reference(self) -> None:
        available, message = compatibility_status()
        self.assertTrue(available, message)
        install_compatibility_patch()
        from litellm import main
        from litellm.litellm_core_utils.prompt_templates import factory

        self.assertTrue(getattr(factory.map_system_message_pt, "__claudex_patch__", False))
        self.assertIs(main.map_system_message_pt, factory.map_system_message_pt)

    def test_sse_text_deltas_recover_empty_completed_response(self) -> None:
        chunks, iterator = self._process_stream(
            self._sse(
                {"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": []}},
                {"type": "response.content_part.added", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}},
                {"type": "response.output_text.delta", "output_index": 0, "content_index": 0, "delta": "Allow"},
                {"type": "response.output_text.delta", "output_index": 0, "content_index": 0, "delta": " this"},
                self._completed_response(output=[]),
            )
        )
        self.assertEqual(chunks[-1]["response"]["output"][0]["content"][0]["text"], "Allow this")
        self.assertFalse(hasattr(iterator, "_claudex_streamed_output_items"))

    def test_sse_done_item_takes_precedence_over_added_deltas(self) -> None:
        chunks, _ = self._process_stream(
            self._sse(
                {"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": []}},
                {"type": "response.content_part.added", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}},
                {"type": "response.output_text.delta", "output_index": 0, "content_index": 0, "delta": "partial"},
                {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "final", "annotations": []}]}},
                self._completed_response(output=[]),
            )
        )
        self.assertEqual(chunks[-1]["response"]["output"][0]["content"][0]["text"], "final")

    def test_sse_function_call_deltas_recover_empty_completed_response(self) -> None:
        chunks, _ = self._process_stream(
            self._sse(
                {"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "approve", "arguments": "", "status": "in_progress"}},
                {"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "{\"allow\":"},
                {"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "true}"},
                self._completed_response(output=[]),
            )
        )
        self.assertEqual(chunks[-1]["response"]["output"][0]["arguments"], '{"allow":true}')

    def test_non_empty_completed_response_is_unchanged(self) -> None:
        completed_item = {"type": "message", "id": "msg_completed", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "completed", "annotations": []}]}
        chunks, _ = self._process_stream(
            self._sse(
                {"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": []}},
                {"type": "response.output_text.delta", "output_index": 0, "content_index": 0, "delta": "ignored"},
                self._completed_response(output=[completed_item]),
            )
        )
        self.assertEqual(chunks[-1]["response"]["output"], [completed_item])

    def test_malformed_stream_chunk_passes_through_once(self) -> None:
        iterator = types.SimpleNamespace()
        seen: list[str] = []

        def original(_iterator, chunk):
            seen.append(chunk)
            return "upstream"

        patched = _patched_process_chunk(original)
        self.assertEqual(patched(iterator, "not json"), "upstream")
        self.assertEqual(seen, ["not json"])

    def test_patch_installation_is_idempotent_and_keeps_target_identity(self) -> None:
        from litellm.responses import streaming_iterator

        install_compatibility_patch()
        target = streaming_iterator.BaseResponsesAPIStreamingIterator
        process_chunk = target._process_chunk
        install_compatibility_patch()
        self.assertTrue(getattr(process_chunk, "__claudex_patch__", False))
        self.assertIs(process_chunk, target._process_chunk)

    @staticmethod
    def _process_stream(raw_sse: str):
        iterator = types.SimpleNamespace()
        chunks: list[dict[str, object]] = []

        def original(_iterator, chunk):
            parsed_chunk = json.loads(chunk)
            chunks.append(parsed_chunk)
            return parsed_chunk

        patched = _patched_process_chunk(original)
        for chunk in raw_sse.splitlines():
            patched(iterator, chunk.removeprefix("data: "))
        return chunks, iterator

    @staticmethod
    def _completed_response(*, output: list[dict[str, object]]) -> dict[str, object]:
        return {
            "type": "response.completed",
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 0,
                "model": "gpt-5.6-terra",
                "status": "completed",
                "output": output,
            },
        }

    @staticmethod
    def _sse(*events: dict[str, object]) -> str:
        return "\n".join(f"data: {json.dumps(event)}" for event in events)
