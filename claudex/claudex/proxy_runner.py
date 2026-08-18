from __future__ import annotations

import argparse
import socket
from typing import Any

from .compat import install_compatibility_patch


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--config", required=True)
    parser.add_argument("--fd", required=True, type=int)
    arguments = parser.parse_args(argv)
    install_compatibility_patch()
    from litellm.proxy.proxy_cli import run_server

    _run_server(run_server, arguments.config, arguments.fd)
    return 0


def _run_server(run_server: Any, config: str, fd: int) -> None:
    import uvicorn

    port = _listener_port(fd)
    original_run = uvicorn.run

    def inherited_run(*args: Any, **kwargs: Any) -> Any:
        kwargs["fd"] = fd
        return original_run(*args, **kwargs)

    uvicorn.run = inherited_run
    try:
        run_server.main(args=["--config", config, "--host", "127.0.0.1", "--port", str(port)], prog_name="litellm")
    finally:
        uvicorn.run = original_run


def _listener_port(fd: int) -> int:
    duplicate = socket.fromfd(fd, socket.AF_INET, socket.SOCK_STREAM)
    try:
        return int(duplicate.getsockname()[1])
    finally:
        duplicate.close()


if __name__ == "__main__":
    raise SystemExit(main())
