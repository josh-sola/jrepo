from __future__ import annotations

import unittest

from claudex.compat import compatibility_status, install_compatibility_patch, map_system_message_content


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

    def test_patch_replaces_factory_and_main_reference(self) -> None:
        available, message = compatibility_status()
        self.assertTrue(available, message)
        install_compatibility_patch()
        from litellm import main
        from litellm.litellm_core_utils.prompt_templates import factory

        self.assertTrue(getattr(factory.map_system_message_pt, "__claudex_patch__", False))
        self.assertIs(main.map_system_message_pt, factory.map_system_message_pt)
