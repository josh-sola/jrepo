from __future__ import annotations

import argparse

from .compat import install_compatibility_patch


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--config", required=True)
    parser.add_argument("--host", required=True)
    parser.add_argument("--port", required=True)
    arguments = parser.parse_args(argv)
    install_compatibility_patch()
    from litellm.proxy.proxy_cli import run_server

    run_server.main(
        args=["--config", arguments.config, "--host", arguments.host, "--port", arguments.port],
        prog_name="litellm",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
