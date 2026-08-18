# claudex

`claudex` runs Claude Code against a private, local LiteLLM gateway that uses
your ChatGPT device login. It is an experimental bridge, not an official
Anthropic, OpenAI, or Claude Code integration. Upstream changes can break it,
and ChatGPT subscription access may vary by plan and region.

It supports macOS and Linux. You need `uv`, Claude Code, and a ChatGPT account
that LiteLLM accepts for its ChatGPT provider. It does not support Windows,
API-key billing, shared gateways, background daemons, or unattended use.

## Install

The usual coworker install does not need a clone. This is a normal, non-editable
`uv tool` install, so removing a checkout cannot break the installed tool.

```sh
uv tool install "git+https://github.com/josh-sola/jrepo.git#subdirectory=claudex"
```

For a private repository or an SSH-based Git setup:

```sh
uv tool install "git+ssh://git@github.com/josh-sola/jrepo.git#subdirectory=claudex"
```

To pin an approved release or commit, put the Git reference before the
subdirectory fragment:

```sh
uv tool install "git+https://github.com/josh-sola/jrepo.git@<tag-or-commit>#subdirectory=claudex"
```

If you already cloned this repository, the fallback installer does the same
non-editable install from that checkout:

```sh
./claudex/install.sh
```

Update with the same `uv tool install --reinstall` command that you used to
install. To remove only the tool, while retaining your login and configuration:

```sh
uv tool uninstall jrepo-claudex
```

The clone fallback also supports `./claudex/install.sh --uninstall`.

The installer never edits shell startup files. If `claudex` is not found after
installing, run `uv tool update-shell` and start a new terminal.

## First use

```sh
claudex login
claudex doctor
claudex models
claudex --model sol -- --help
```

`login` starts LiteLLM's ChatGPT device OAuth flow. `doctor` makes no model
request. Each normal invocation starts one temporary gateway on a random local
port, runs Claude Code, and removes that gateway when Claude exits.

Run Claude Code as usual by putting its arguments after `claudex`:

```sh
claudex
claudex --model luna -- -p "Summarize this repository"
claudex --model gpt-5.6-terra -- --resume
```

The default is `terra`. The built-in aliases and Claude Code role mappings are:

| Alias | OpenAI model | Claude role |
| --- | --- | --- |
| `sol` | `gpt-5.6-sol` | Opus |
| `terra` | `gpt-5.6-terra` | Sonnet and default |
| `luna` | `gpt-5.6-luna` | Haiku |
| `spark` | `gpt-5.3-codex-spark` | optional direct selection |

## Configuration

Optionally create `~/.config/claudex/config.toml` (or
`$XDG_CONFIG_HOME/claudex/config.toml`). `claudex` never creates or overwrites
this file.

```toml
default = "terra"

[models]
review = "gpt-5.6-terra"

[roles]
opus = "sol"
sonnet = "review"
haiku = "luna"
```

Model values may be an alias or an upstream model ID. You may override any
role; roles you omit keep their built-in mappings. `claudex models` shows the
resolved result.

## Security and troubleshooting

`claudex` uses private directories (`0700`) and private files (`0600`). It
keeps configuration in `~/.config/claudex` and LiteLLM's OAuth state below
`~/.local/state/claudex/chatgpt`; XDG equivalents are used when set. The
gateway YAML is temporary, the local bearer key only exists in child-process
memory, and neither is logged or stored in a command line.

Run `claudex doctor` first if launch fails. It checks the OS, Claude Code,
LiteLLM version, OAuth state permissions, and the LiteLLM compatibility patch.
If it reports an open permission, use the exact `chmod` command it prints. If
the OAuth flow succeeds but launch fails after an upgrade, reinstall this pinned
package version; the experimental provider and gateway protocol can change
without notice.

Claude Code is configured only through the process environment. `claudex` does
not modify `~/.claude/settings.json`, and removes an inherited
`ANTHROPIC_API_KEY` for its child process.
