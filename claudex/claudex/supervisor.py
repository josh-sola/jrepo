from __future__ import annotations

import os
import signal
import socket
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import Any
from urllib.error import URLError
from urllib.request import urlopen

from .config import litellm_environment, private_directory, state_dir
from .gateway import new_master_key, write_gateway_config
from .models import ModelSettings


def available_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


class ProxySupervisor:
    def __init__(
        self,
        settings: ModelSettings,
        env: dict[str, str] | None = None,
        popen: Callable[..., Any] = subprocess.Popen,
        ready: Callable[[str], bool] | None = None,
        wait_seconds: float = 10,
        extra_models: list[str] | None = None,
    ) -> None:
        self.settings = settings
        self.parent_env = dict(env or os.environ)
        self.popen = popen
        self.ready = ready or proxy_ready
        self.wait_seconds = wait_seconds
        self.extra_models = extra_models or []
        self.process: Any | None = None
        self.port: int | None = None
        self.master_key: str | None = None
        self._temporary: tempfile.TemporaryDirectory[str] | None = None
        self._log_file: Any | None = None
        self.log_path: Path | None = None

    def __enter__(self) -> "ProxySupervisor":
        self.port = available_port()
        self.master_key = new_master_key()
        runtime = private_directory(state_dir(self.parent_env) / "runtime")
        self._temporary = tempfile.TemporaryDirectory(prefix="proxy-", dir=runtime)
        temporary_path = Path(self._temporary.name)
        config = write_gateway_config(temporary_path, self.settings, self.extra_models)
        self.log_path = temporary_path / "proxy.log"
        self._log_file = _open_private_log(self.log_path)
        environment = litellm_environment(self.parent_env)
        environment["CLAUDEX_LITELLM_MASTER_KEY"] = self.master_key
        self.process = self.popen(
            _litellm_command(config, self.port),
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=self._log_file,
            stderr=subprocess.STDOUT,
            preexec_fn=_set_private_umask,
        )
        try:
            self._wait_until_ready()
        except BaseException:
            self.close()
            raise
        return self

    @property
    def base_url(self) -> str:
        if self.port is None:
            raise RuntimeError("proxy has not started")
        return f"http://127.0.0.1:{self.port}"

    @property
    def token(self) -> str:
        if self.master_key is None:
            raise RuntimeError("proxy has not started")
        return self.master_key

    def _wait_until_ready(self) -> None:
        deadline = time.monotonic() + self.wait_seconds
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(
                    f"LiteLLM proxy stopped before it became ready (exit {self.process.returncode}). {self._log_tail()}"
                )
            if self.ready(self.base_url):
                return
            time.sleep(0.1)
        raise RuntimeError(f"LiteLLM proxy did not become ready within 10 seconds. {self._log_tail()}")

    def close(self) -> None:
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=3)
        self.process = None
        if self._log_file is not None:
            self._log_file.close()
            self._log_file = None
        if self._temporary is not None:
            self._temporary.cleanup()
            self._temporary = None

    def __exit__(self, *_: object) -> None:
        self.close()

    def _log_tail(self) -> str:
        if self._log_file is not None:
            self._log_file.flush()
        if self.log_path is None or not self.log_path.exists():
            return "No proxy log was written."
        lines = self.log_path.read_text(encoding="utf-8", errors="replace").splitlines()[-12:]
        text = "\n".join(lines).replace(self.master_key or "", "[redacted]")
        return f"Proxy log tail:\n{text}" if text else "Proxy log was empty."


def _litellm_command(config: Path, port: int) -> list[str]:
    return [sys.executable, "-m", "claudex.proxy_runner", "--config", str(config), "--host", "127.0.0.1", "--port", str(port)]


def _set_private_umask() -> None:
    os.umask(0o077)


def _open_private_log(path: Path) -> Any:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    return os.fdopen(descriptor, "w", encoding="utf-8", buffering=1)


def proxy_ready(base_url: str) -> bool:
    try:
        with urlopen(f"{base_url}/health/liveliness", timeout=0.5) as response:
            return 200 <= response.status < 300
    except (OSError, URLError):
        return False


def run_child(command: Sequence[str], env: dict[str, str]) -> int:
    child = subprocess.Popen(command, env=env)
    old_handlers: dict[int, Any] = {}

    def forward(signum: int, _frame: object) -> None:
        if child.poll() is None:
            child.send_signal(signum)

    for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        old_handlers[signum] = signal.signal(signum, forward)
    try:
        return child.wait()
    finally:
        for signum, handler in old_handlers.items():
            signal.signal(signum, handler)
