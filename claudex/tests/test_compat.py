from __future__ import annotations

import sys
import types
import unittest

from claudex.compat import _replace_imported_references, compatibility_status, install_compatibility_patch, map_system_message_content


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
