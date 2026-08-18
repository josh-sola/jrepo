from __future__ import annotations

import socket
import unittest
from unittest.mock import patch

import uvicorn

from claudex.proxy_runner import _run_server


class ProxyRunnerTests(unittest.TestCase):
    def test_run_server_passes_inherited_fd_to_uvicorn(self) -> None:
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        fd = listener.fileno()
        captured: dict[str, object] = {}

        class RunServer:
            def main(self, **kwargs: object) -> None:
                captured["cli"] = kwargs
                uvicorn.run("app", host="127.0.0.1", port=0)

        def uvicorn_run(*_args: object, **kwargs: object) -> None:
            captured["uvicorn"] = kwargs

        try:
            with patch.object(uvicorn, "run", uvicorn_run):
                _run_server(RunServer(), "config.yaml", fd)
        finally:
            listener.close()
        self.assertEqual(captured["uvicorn"]["fd"], fd)
        self.assertEqual(captured["cli"]["args"][:2], ["--config", "config.yaml"])
