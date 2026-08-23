# @dshl/pipe

> The dshl pipe backend — an **optional** dsh plugin that connects to a
> running dshl launcher over the legacy control pipe (`DSHL_CONTROL_URL`)
> and provides a subset of the native capability set as the
> `dshlPipeBackend` Cordis service.

## Overview

`@dshl/pipe` is a [dsh](https://github.com/hibays/dsh-launcher) plugin. It
is the **legacy / remote fallback** backend for `@dshl/control`: when a
standalone launcher exe started the dsh process and exported the control
endpoint via `DSHL_CONTROL_URL`, this plugin connects to it and dispatches
supervisor, action, and status calls over the pipe.

It is a **seam, not a requirement**: if `DSHL_CONTROL_URL` is unset (or not
a valid `dshl://` endpoint) at load time, the plugin exports `backend =
null`, registers no Cordis service, and is fully inert. Consumers such as
`@dshl/control` pick it up opportunistically — they call
`ctx.get('dshlPipeBackend') ?? null` and fall through to `@dshl/native`
(or report "no backend available") when it is absent.

The pipe cannot drive OS surfaces in another process, so window / tray /
embedded-terminal controls are **native-only** and are never exposed here.

## Capability subset

What `buildActions` / `buildSupervisor` / `buildStatus` actually export:

| Surface | Member | Behavior over pipe |
| --- | --- | --- |
| `actions` | `openTerminal()` | Remote `open-terminal` request; resolves `true` |
| `actions` | `openPath()` | Always `false` (not supported remotely) |
| `actions` | `openUrl()` | Always `false` (not supported remotely) |
| `actions` | `ping()` | Remote `ping` request |
| `actions` | `platformInfo()` | Local info only: `{ os, arch, shell: 'bash' }` |
| `supervisor` | `shutdown()` | Remote `shutdown`; resolves `{ ok: true, native: false }` |
| `supervisor` | `restart()` | Remote `restart`; resolves `{ ok: true, native: false }` |
| `supervisor` | `launch()` | Never sent — resolves `{ ok: false, error: 'launch not supported over pipe' }` |
| `status()` | — | Sends `ping`; reports `backend: 'pipe'`, launcher `version`, `connected`, `pipeError`; `launched` / `kernelRunning` mirror connectivity; `windowVisible` / `trayVisible` are `null` (unknown from here) |

## Protocol

A tiny newline-delimited JSON protocol over a single loopback TCP socket:

```
→  {"type":"hello","token":"<per-launch token>"}
   (server sends nothing on success)
→  {"type":"request","id":1,"method":"ping","params":{}}
←  {"id":1,"result":{"pong":true,"version":"…"}}
←  {"id":1,"error":"…"}                (on failure)
```

- **Hello handshake.** On every connect the client sends one
  `{"type":"hello","token":…}` frame. There is **no success reply** — the
  socket simply becomes usable. On failure the server answers with a single
  `{"id":0,"error":"…"}` frame and closes the connection. The client ignores
  the id-0 frame itself (no matching pending request); the failure surfaces
  through the disconnect, which rejects in-flight requests.
- **Requests.** One `{"type":"request","id":N,"method":…,"params":{}}`
  object per line; responses carry the same numeric `id` and exactly one of
  `result` / `error`. Each request has a 15 s timeout; connecting has a 5 s
  timeout.
- **Reconnect.** One socket at a time. A dropped connection rejects
  in-flight requests and the next `request()` reconnects (the server
  re-authenticates each new socket with the hello handshake).

Server-side methods dispatched by the launcher (`src/control.rs`):
`ping`, `shutdown`, `switch-profile`, `open-terminal`, `restart`;
anything else is answered with an error.

## Configuration

### `DSHL_CONTROL_URL`

Set in the environment before the dsh process starts:

```bash
DSHL_CONTROL_URL=dshl://<token>@127.0.0.1:<port> dsh web
```

Format: `dshl://<token>@host:port` — the `<token>` is the per-launch random
bearer token the launcher generated for this session and checked on every
connection.

Because the raw URL embeds that secret, it is **never logged verbatim**
(logs land on disk):

- Endpoint parse failures throw reason-only errors describing the expected
  shape instead of echoing the offending string.
- The startup log line redacts the credential:
  `dshl://***@127.0.0.1:<port>`.

When `DSHL_CONTROL_URL` is unset or invalid, no client is constructed, the
plugin provides no service, and consumers see no `dshlPipeBackend`.

## License

MIT © hibays
