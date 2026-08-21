# jpi

jpi is a Pi package for personal extensions and skills in this repository.

## Install from Git

Install the current default branch:

```sh
pi install git:github.com/josh-sola/jrepo
```

To keep the installation on a specific release or revision, append a tag or
commit:

```sh
pi install git:github.com/josh-sola/jrepo@<tag-or-commit>
```

Pi treats the repository root as the package root. As a result, Git
installation clones the full monorepo even though the Pi resources are under
`jpi/`.

## Develop locally

From the repository root, load the checkout for one Pi session:

```sh
pi -e .
```

To keep the checkout installed, pass its absolute path to `pi install` instead.
Run the focused check from the repository root:

```sh
just jpi-check
```

Start Pi and run `/jpi`. An info notification that says `jpi is loaded.`
confirms that the extension loaded.

## Status footer extension

`jpi/extensions/jpi-status/` replaces Pi's built-in footer in TUI sessions.
Its layout is configured through
`${PI_CODING_AGENT_DIR:-~/.pi/agent}/status-line.json`. This is the default:

```json
{
  "format": [
    [
      "@jpi/model",
      "@jpi/context",
      "@jpi/repository",
      "@jpi/worktree",
      "@jpi/branch",
      "@jpi/pull-request",
      "@jpi/stack"
    ],
    ["@jpi/slot"]
  ],
  "disabledStatuses": []
}
```

Each inner `format` array is one line. The footer omits unavailable components
and lines that become empty. It joins rendered components with ` · `. The local
component IDs are:

| ID | Content |
| --- | --- |
| `@jpi/model` | Active model name |
| `@jpi/context` | Context-window percentage |
| `@jpi/repository` | Repository name |
| `@jpi/worktree` | Linked `wt` tree name |
| `@jpi/branch` | Shortened branch name |
| `@jpi/pull-request` | Graphite pull request and draft state |
| `@jpi/stack` | Graphite stack position |
| `@jpi/slot` | Published extension statuses, sorted by status ID |

Any other `format` string is an exact, case-sensitive extension `setStatus()`
ID, such as `auto-review`. A missing extension status is omitted until its
extension publishes it. The `@jpi/` namespace is reserved; unknown IDs in that
namespace make the config invalid.

`@jpi/slot` includes every published extension status except IDs listed in
`disabledStatuses`. An explicit extension ID still renders when it is disabled
from the slot. The slot also includes explicitly placed statuses, so the same
status can appear more than once. This filtering only changes the custom footer;
it does not disable an extension.

Both config fields are optional. A missing file or field uses its default
without warning. Invalid JSON or values produce a warning and restore the full
default config.

The extension refreshes Git and `wt` metadata at startup, after branch changes,
and every 10 seconds. Missing Git, `wt`, Graphite, or GitHub-origin data removes
only the affected components. It does not show token totals, cost, thinking
level, or provider rate limits.

### Commands

- `/jpi-status status` reports whether this footer component is active.
- `/jpi-status refresh` requests an immediate repository metadata refresh.
- `/jpi-status reload` reloads `status-line.json` and rerenders the footer.

Pi has one custom-footer slot. Another extension that calls `setFooter()` later
replaces this footer. Existing extension statuses remain registered, but the
replacement footer decides whether to display them.

## Auto-review extension

`jpi/extensions/auto-review/` adds an auto-review gate for tool calls.
It reads only the user-level reviewer config at
`${PI_CODING_AGENT_DIR:-~/.pi/agent}/review.json`. It ignores project-local
config on purpose.

### Setup

1. Copy `jpi/review.example.json` to `~/.pi/agent/review.json` or to the
   directory named by `PI_CODING_AGENT_DIR`.
2. Set `model` to the reviewer model you want Pi to call.
3. Keep `allow.tools` and `allow.bash` small. `allow.bash` entries are regular
   expression sources that must match the full command. Anchor them, for
   example `^npm test$`.
4. Start Pi with this package loaded, then run `/auto-review status`.

The config shape is:

```json
{
  "model": "provider/model-id",
  "enabled": true,
  "allow": {
    "tools": ["read", "find"],
    "bash": ["^npm test$"]
  },
  "policy": ["extra trusted reviewer instruction"],
  "timeoutMs": 10000
}
```

`enabled` defaults to `true`. `policy` appends trusted environment rules to the
bundled reviewer policy; it does not replace that policy. `/auto-review on` and
`/auto-review off` change only the current session. They do not write to disk.

### Commands

- `/auto-review status` shows the current state.
- `/auto-review on` forces review on for the current session.
- `/auto-review off` disables review for the current session.
- `/auto-review reload` reloads `review.json` from disk.

### Behavior and limits

- Every non-allowlisted tool call is reviewed before execution.
- The bundled policy keeps the detailed Guardian risk taxonomy for data egress,
  credential use, security weakening, destructive actions, authorization, and
  low-risk exceptions. Its stable prefix uses short-lived prompt caching.
- Exact tool names in `allow.tools` skip review.
- `allow.bash` applies only to the full bash command string. Partial matches do
  not skip review.
- After a call passes, its arguments are frozen so a later extension cannot
  change the action after review. Argument-rewriting extensions must run first.
- If config, reviewer model, or reviewer auth is missing or invalid while the
  gate is enabled, the extension fails closed for non-allowlisted calls and
  tells Pi how to reload or disable the gate.
- An explicit denial blocks the tool call and tells the main model not to work
  around the denial. It may only try a materially safer alternative or ask the
  user. Three denials without an approved call in between stop the run to
  prevent retry loops. Reviewer failures do not reset the denial count.
- Reviewer timeouts, errors, or malformed output also block the call, but they
  are treated as review failures rather than unsafe judgments. The model gets
  one retry. A second consecutive review failure stops the run and directs it
  to ask the user.
- A stop takes effect when every tool call in the current batch is blocked. If
  a parallel batch mixes an allowed call with the tripping denial, the allowed
  call still runs and the stop lands on the next batch; the open circuit blocks
  every call after the trip either way.

### Trust and data exposure

The reviewer model receives a compact bundle of recent user text, the current
working directory, the exact tool name, and bounded JSON arguments. It does not
receive hidden assistant reasoning or prior review decisions as precedent.

This extension is not a sandbox. It is a policy gate that runs with the same
local privileges as the rest of Pi. This first version reviews proposed actions
but does not scan tool results for prompt injection. If a tool call is
allowlisted, it skips the review model entirely. That is convenient, but it also
removes this extra check. Use allowlists only for commands and tools you trust.

### Design context

This extension borrows the general shape from Anthropic's Claude Code auto-mode
documentation and OpenAI's Codex auto-review documentation. Its bundled policy
is a near-complete adaptation of the public Codex Guardian policy. It removes
only rules that assume the reviewer can run its own read-only tools:

- https://www.anthropic.com/engineering/claude-code-auto-mode
- https://code.claude.com/docs/en/auto-mode-config
- https://learn.chatgpt.com/docs/sandboxing/auto-review
- https://github.com/openai/codex/blob/main/codex-rs/core/src/guardian/policy.md

## Resources

- Extensions: `jpi/extensions/`
- Skills: `jpi/skills/`
- Tests: `jpi/tests/`

Keep each skill in its own directory with a `SKILL.md` file. Do not place a
Markdown file directly in `jpi/skills/`, because Pi treats top-level Markdown
files there as skills.
