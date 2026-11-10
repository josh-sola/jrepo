# codemode

`cm` lets you call MCP servers from a short JavaScript program. You send it
one program on stdin. It runs the program and prints one JSON result. Each
configured MCP server is a namespace named `tools.<server>`.

`cm guide` prints this file, and `cm --help` points agents to it.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/josh-sola/jrepo/main/codemode/install.sh | sh
```

This installs the newest release on macOS or Linux into `~/.local/bin/cm`. Put
`CM_VERSION=0.1.0` in front of `sh` to pick a version.

```sh
cm upgrade          # install the newest release
cm upgrade --check  # only report the installed and newest versions
cm daemon status    # show whether the background daemon is running
cm daemon stop      # stop the daemon and the servers it keeps warm
```

## Config

`cm` reads `$CM_CONFIG`. Without it, `cm` reads
`~/.config/codemode/servers.toml`. Each server is a `[servers.<name>]` table.
The name must not contain a dot.

```toml
# A local server. It needs "command". "args", "env" and "cwd" are optional.
[servers.fs]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "~/notes"]

# A remote server that uses OAuth. Run "cm auth linear" once to log in.
[servers.linear]
url = "https://mcp.linear.app/mcp"
auth = "oauth"

# A remote server with a token header.
[servers.gh]
url = "https://api.githubcopilot.com/mcp/"
headers = { Authorization = "Bearer ${GITHUB_TOKEN}" }
```

- `${VAR}` in `args`, `env` and `headers` expands from the caller's
  environment. An unset variable is a config error.
- A leading `~` in `args` and `cwd` means your home directory.
- `cwd` defaults to the folder that holds the config file.
- Servers start on first use and stay warm for 10 minutes after the last call.
  `cm` starts a small background process to hold them.
- A server sees only `PATH`, `HOME`, `XDG_*` and what `env` sets.

## Commands

Find out what exists before you write a program.

```sh
cm describe                    # one line per server: its name and tool count
cm describe fs                 # TypeScript signatures for one server
cm describe fs.read_text_file  # one tool, in full
```

`cm describe` prints lines such as `fs (16 tools)`. A big server can have
hundreds of tools, so do not list one in full. Use `search()` inside a program
to find the tools you need. See "Writing a program".

Run a program. It comes from stdin, or from a file with `--file`.

```sh
cm run <<'EOF'
const [files, info] = await Promise.all([
  tools.fs.list_directory({ path: "." }),
  tools.fs.get_file_info({ path: "README.md" }),
])
console.log("listed", files.content.split("\n").length)
return { files, info }
EOF
```

Log in to a server that uses OAuth. This opens a browser. `run` and
`describe` never open one. They fail and tell you to run `cm auth <server>`.

```sh
cm auth linear
```

## Writing a program

- Call `await tools.<server>.<tool>({ ...args })`. A tool named `get-sum`
  needs brackets: `tools.everything["get-sum"]({ a: 1, b: 2 })`.
- To find a tool in a large server, call `search({ query?, namespace?,
  limit?, offset? })`. `query` matches tool names and descriptions, and
  `namespace` limits the search to one server. It returns matching tools with
  their signatures. It counts as one tool call.

  ```js
  return await search({ query: "echo", namespace: "everything", limit: 5 })
  ```

  ```json
  {
    "items": [
      {
        "path": "tools.everything.echo",
        "description": "Echoes back the input string",
        "signature": "tools.everything.echo({\n  /** Message to echo */\n  message: string,\n}): Promise<string | null>"
      }
    ],
    "remaining": 0,
    "next": null
  }
  ```

  When `remaining` is above 0, call `search` again with `offset` set to
  `next.offset`.
- Use `return` to send back a value. It must be plain JSON. Without a
  `return`, the value is `null`.
- Use `console.log` for notes. They come back in `logs`.
- Start independent calls together with `Promise.all`.
- A tool returns its structured result when it has one. Otherwise it returns
  the text of the result as a string.
- If a tool reports an error, the call throws. Catch it with `try` and
  `catch` to carry on.
- The program runs in a confined interpreter that supports a JavaScript
  subset. It has no `require`, no `fetch` and no file access. Use tools for
  those.
- You may use TypeScript type annotations. They are stripped before the
  program runs. Enums and namespaces are not supported.
- Limits: 60 seconds (change with `--timeout <ms>`), 100 tool calls, and 1 MB
  of output.

## Output

`cm run` prints one JSON object on stdout.

```json
{ "ok": true, "value": { "files": ["..."] }, "logs": ["listed 3"] }
```

```json
{ "ok": false, "error": { "kind": "ToolFailure", "message": "..." } }
```

`error.kind` says what went wrong, for example `ParseError`, `UnknownTool`,
`ToolFailure`, `ExecutionFailure` or `TimeoutExceeded`. `error.suggestions`
may list similar tool names. `truncated` is `true` when the output hit the
size limit. `warnings` lists problems that did not stop the program.

If a server is down or needs a login, `cm run` still runs the program with
the other servers. The result then has `unavailable`, which maps each failed
server to its message. A program that calls a missing server fails with
`UnknownTool`, and `unavailable` says why. `cm describe` marks such servers
as unavailable.

Exit codes: 0 when `ok` is true, 1 when the program or a server failed, and 2
for a usage or config error. Errors outside the JSON result print as
`cm: <message>` on stderr.
