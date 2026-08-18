from __future__ import annotations

import importlib
import unittest


class LiteLLMProxyImportTests(unittest.TestCase):
    def test_proxy_server_imports_with_pinned_fastapi(self) -> None:
        importlib.import_module("litellm.proxy.proxy_server")
