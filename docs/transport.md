---
wiki: src/transport.rs
---

# transport.rs

MCP Streamable HTTP: session management, SSE and JSON responses, stateless mode,
a configurable bind address, authentication, and size/rate limits. P1.10 of
[capability coverage](capability-coverage.md).

The stdio transport in [mcp](mcp.md) is unchanged and still the default — one
agent, one process, no network. This is the shared variant, for when more than
one client needs the same indexed repository.

```bash
aag mcp --transport http                      # loopback, sessions, no auth
aag mcp --transport http --port 8787 \
        --bind 0.0.0.0 --api-key "$TOKEN"     # shared
aag mcp --transport http --stateless          # behind a load balancer
```

## Two rules that do not bend

A graph server reads anything in the repository it indexes. So:

- **Binding anywhere but loopback requires `--api-key`.** The server refuses to
  start otherwise, with a message naming the flag. An unauthenticated shared
  transport is not a deployment option, it is an accident.
- **Every request is bounded.** `--max-body` (1 MiB default) caps a body,
  `--rate-limit` (600/minute default) caps a client, and 256 caps concurrent
  sessions. A server a client can make allocate without limit is not shareable
  either.

A cross-origin `Origin` header is refused with 403 so a browser page cannot drive
the server, and any path other than `/mcp` is 404.

## Sessions

`POST /mcp` with no `Mcp-Session-Id` mints one and returns it in that header.
Later requests present it; an unknown or expired id is a 400 that says
`session not found` rather than silently starting a new session — a client that
lost its session should be told, not quietly given a different one.

`DELETE /mcp` with the header ends the session. Otherwise a session expires after
30 minutes of silence, which keeps the table bounded without a background task.

`--stateless` skips all of it: every request stands alone, no id is minted, and
nothing a client claims is checked. That is the mode for a load balancer that
will not pin a client to one process.

## JSON and SSE

The same JSON-RPC answer is framed by what the client asked for:

| Request | Response |
|---|---|
| `POST /mcp`, `Accept: application/json` | `application/json`, the response object |
| `POST /mcp`, `Accept: text/event-stream` | `text/event-stream`, one `event: message` |
| `POST /mcp` for a notification | 202, empty body |
| `GET /mcp`, `Accept: text/event-stream` | an open stream: notifications and keepalives |
| `GET /mcp` without that Accept | 405 — there is nothing to receive |

**The `GET` stream carries server-initiated notifications.** It opens with one
`notifications/ready`, and every time the index is rewritten — by the watcher,
by a reconcile on connect — each open stream gets:

```text
event: message
data: {"jsonrpc":"2.0","method":"notifications/resources/updated",
       "params":{"uri":"aag://graph","revision":7}}
```

That is the MCP resource-update notification for the one resource this server
publishes. `resources/list` names it, `resources/read` returns the current
counts and the same revision number, so a client woken by a notification can
confirm it read the revision it was told about. Between changes the stream
emits keepalive comments, and it is bounded: it closes after an hour of them so
a forgotten client cannot hold a thread.

The stdio transport has no such channel — there is no stream to push into — so
a stdio client still asks again. That is a property of the transport, not a
choice about the notification.

## Container deployment

The server is one static binary, one SQLite file, and no other services. What a
container needs is the index — so build it in, or mount it:

```dockerfile
FROM debian:stable-slim
COPY aag /usr/local/bin/aag
WORKDIR /repo
COPY . /repo
# Index at build time so the image starts answering immediately.
RUN aag bigbang --no-install --no-viz
EXPOSE 8787
# 0.0.0.0 inside the container is fine — the token is what protects it.
CMD ["aag", "mcp", "--transport", "http", "--bind", "0.0.0.0", "--port", "8787"]
```

Operational notes, each the consequence of something above:

- **Pass the token as `AAG_MCP_API_KEY`**, not as an argument: an argument is
  visible in the process list. The flag exists for interactive use.
- **`--stateless` behind more than one replica.** Sessions live in the process
  that minted them, so a load balancer that spreads requests will otherwise hand
  a client a 400.
- **Mount the repository read-write or accept a stale graph.** The watcher and
  the post-edit hook write to `.aag/`; with a read-only mount the server answers
  from the index it was built with, which is a fine deployment as long as it is
  the intent.
- **One repository per server.** The root is a process-level choice; serving
  several means several processes, which is also how groups stay independent (see
  [federation](federation.md)).
- **Publish the port to loopback on the host** (`-p 127.0.0.1:8787:8787`) unless
  the token is set and you intend the exposure.
