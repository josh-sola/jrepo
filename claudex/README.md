# claudex

`claudex` runs Claude Code through a local LiteLLM gateway that uses your
ChatGPT login. It is a personal, experimental bridge between Claude Code and
OpenAI models.

You need macOS or Linux, `uv`, Claude Code, the Codex CLI, and a ChatGPT
account that LiteLLM accepts for its ChatGPT provider.

## Install

```sh
uv tool install "git+https://github.com/josh-sola/jrepo.git#subdirectory=claudex"
```

If the command is not available after installing, run `uv tool update-shell`
and start a new terminal.

## First use

```sh
claudex login
claudex doctor
claudex models
claudex --model sol -- --help
```

`login` opens the Codex browser sign-in and stores the resulting tokens in
Claudex's private LiteLLM cache. `doctor` checks the local setup without making
a model request. Each normal launch starts a temporary local gateway and
removes it when Claude Code exits.

## Usage

Run Claude Code as usual by putting its arguments after `claudex`:

```sh
claudex
claudex -p "Summarize this repository"
claudex --model luna -p "Summarize this repository"
claudex --model gpt-5.6-terra -- --resume
```

`claudex` reserves `--model` before `--` for its own session-only model
choice. Use `claudex run -- ...` when Claude Code must receive `--model` or
when a Claude argument looks like a Claudex command:

```sh
claudex run -- --model claude-choice login
claudex run -- --help
```

Run `claudex --help` or `claudex run --help` for command help. The `--`
separator is not passed to Claude Code.

## With wt

Use `wt` to launch Claudex in a prepared, colored worktree session:

```sh
wt launch "fix login" monorepo --claudex -- --model sol
```

That `--model` selects Claudex's OpenAI model. When the `--model` option is
for Claude Code instead, use a second separator after wt's outer separator.
wt removes the first one and forwards the second to Claudex:

```sh
wt launch "fix login" monorepo --claudex -- -- --model claude-choice
```

For a direct process-replacing launch in a selected tree, use:

```sh
wt claudex "fix login" -- --model sol
```

## Models

The built-in aliases and Claude Code role mappings are:

| Alias | OpenAI model | Claude role |
| --- | --- | --- |
| `sol` | `gpt-5.6-sol` | Opus |
| `terra` | `gpt-5.6-terra` | Sonnet |
| `luna` | `gpt-5.6-luna` | Haiku |
| `spark` | `gpt-5.3-codex-spark` | optional direct selection |

## Configuration

Optionally create `~/.config/claudex/config.toml` (or
`$XDG_CONFIG_HOME/claudex/config.toml`). Claudex never creates or overwrites
this file.

```toml
[models]
review = "gpt-5.6-terra"

[roles]
opus = "sol"
sonnet = "review"
haiku = "luna"
```

Model values may be an alias or an upstream model ID. You may override any
role; roles you omit keep their built-in mappings. `claudex models` shows the
resolved aliases and roles.
