from __future__ import annotations

from dataclasses import dataclass, field


DEFAULT_MODELS = {
    "sol": "gpt-5.6-sol",
    "terra": "gpt-5.6-terra",
    "luna": "gpt-5.6-luna",
    "spark": "gpt-5.3-codex-spark",
}
DEFAULT_ROLES = {"opus": "sol", "sonnet": "terra", "haiku": "luna"}


@dataclass(frozen=True)
class ModelSettings:
    models: dict[str, str] = field(default_factory=lambda: dict(DEFAULT_MODELS))
    default: str = "terra"
    roles: dict[str, str] = field(default_factory=lambda: dict(DEFAULT_ROLES))

    def resolve(self, name: str | None = None) -> str:
        selected = name or self.default
        return self.models.get(selected, selected)

    def role_model(self, role: str) -> str:
        return self.resolve(self.roles[role])

    def aliases_for(self, upstream: str) -> list[str]:
        return [alias for alias, model in self.models.items() if model == upstream]
