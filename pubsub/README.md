# pubsub

`pubsub` is a local message bus. Agents on one machine talk to each other
through named topics. It keeps everything in one SQLite file, so it needs no
server.

`pubsub guide` prints this file, and `pubsub --help` points agents to it.

## Commands

Create a topic. It prints a name such as `brave-otter-lantern` on stdout, and
the matching `listen` and `send` commands on stderr.

```sh
pubsub create
```

Send a message. The body comes from stdin, or from a file with `--file`. It
must be non-empty UTF-8. The command prints the message id.

```sh
pubsub send brave-otter-lantern --user alice <<'EOF'
Build is green.
Ready for review.
EOF
pubsub send brave-otter-lantern --user alice --file notes.md
```

Listen for messages. By default it prints only new ones.

```sh
pubsub listen brave-otter-lantern
pubsub listen brave-otter-lantern --from-start
pubsub listen brave-otter-lantern --user bob
```

With `--user`, `listen` hides that user's own messages. It also saves its
place, so the next `listen` with the same `--user` resumes where the last one
stopped.

Close a topic. This deletes its messages and ends every `listen` on it.
Closing a closed topic does nothing. A closed name is never used again.

```sh
pubsub close brave-otter-lantern
```

## Where data lives

`$PUBSUB_DB` sets the database path. Without it, `pubsub` uses
`~/.local/share/pubsub/pubsub.db` and creates the folder if needed.

## Output format

`listen` prints each message in a frame. The time is local time.

```
[#12 alice 14:03:07]
Build is green.
Ready for review.
[/#12]
```

When the topic closes, `listen` prints `[closed <topic>]` and exits with
status 0. A missing or closed topic at start is an error with status 1.

Errors print as `pubsub: <message>` on stderr.

## Listen from a Claude Code agent

Start a Monitor with these settings:

- command: `pubsub listen <topic> --user <your-name>`
- timeout_ms: `1800000` (the maximum)

When the Monitor expires, start it again with the same command. The saved
place means no message is missed.

A message is complete when its `[/#id]` line arrives. `[closed <topic>]` means
the topic is gone and the Monitor ends.

Send replies with `pubsub send <topic> --user <your-name>` and put the body on
stdin with a heredoc.
