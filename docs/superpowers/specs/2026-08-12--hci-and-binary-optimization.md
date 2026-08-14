# Harness Control Interface (HCI) + Health Agent + Binary Optimization

**Date:** 2026-08-12
**Status:** Draft (post-brainstorming, pre-review)
**Scope:** Two related concerns bundled into one spec because the second (binary
size / RAM) is a property the first depends on, and neither is meaningful
without the other.

---

## 0. Why this spec exists

The product is a local-first, BYOK, multi-agent workspace. Two questions are
open simultaneously:

1. **How does an external agent (e.g. a Mavis-style outer loop) connect to a
   running `harness.exe` and drive it as a real user would?** Today the only
   way to test the desktop app is to launch it, click around, and hope. That
   is not a feedback loop we can build a self-improving product on.
2. **How do we shrink the shipped binary to ~5MB and reduce idle RAM as much
   as the platform allows?** Other Tauri v2 apps demonstrate that 4–5MB
   single-file executables are reachable. We have not done the work.

Both answers are small in absolute terms (the HCI server is ~150KB, the
frontend-embedding pass is build-system work) but they each unblock
something larger. HCI unblocks automated UI-level regression testing and
the in-process Health Agent. Binary optimization unblocks the "feels like
nothing is running" UX that local-first apps live or die on.

This spec designs HCI and Health Agent in detail, then proposes a concrete
path to the 5MB target with realistic expectations on RAM.

---

## 1. Architecture overview

```
┌──────────────────────────────────────────────────────┐
│  harness.exe (single process)                        │
│                                                      │
│  ┌──────────────┐    ┌──────────────────────────┐    │
│  │  WebView     │    │  HCI Server (new)        │    │
│  │  (React UI)  │    │  - TcpListener (opt-in)  │    │
│  │              │◄──►│  - Auth gate             │    │
│  │  existing    │    │  - Method router         │    │
│  │  IPC bridge  │    │  - Event broadcaster     │    │
│  └──────┬───────┘    └──────────┬───────────────┘    │
│         │                       │                    │
│         └───────────┬───────────┘                    │
│                     │                                │
│              ┌──────▼──────┐                         │
│              │  Shared     │                         │
│              │  State      │ ← existing Zustand +   │
│              │             │   Tauri State          │
│              └──────┬──────┘                         │
│                     │                                │
│         ┌───────────┼────────────┐                   │
│         │           │            │                   │
│   ┌─────▼────┐ ┌────▼─────┐ ┌────▼──────────┐        │
│   │Orchestr. │ │  Coder / │ │ Health Agent  │ (new) │
│   │          │ │Architect /│ │ - periodic    │        │
│   │          │ │Researcher │ │ - ephemeral   │        │
│   └──────────┘ └──────────┘ │ - user-model  │        │
│                             └───────────────┘        │
└──────────────────────────────────────────────────────┘
        ▲                       ▲
        │ TCP                   │ TCP
        │                       │
┌───────┴────────┐       ┌──────┴─────────┐
│  Outer agent   │       │  CI / scripts  │
│  (Mavis, etc.) │       │  (any client)  │
└────────────────┘       └────────────────┘
```

**One protocol, three clients.** The HCI server speaks the same wire format
to all of them. The Health Agent and any future in-process clients skip the
TCP listener and auth gate but hit the same method router and shared state.

---

## 2. Harness Control Interface (HCI)

### 2.1 Transport and lifecycle

- **Opt-in.** HCI is disabled unless `HARNESS_CONTROL=1` is in the
  environment OR a Settings toggle is on. Default: OFF in release builds,
  ON in dev builds.
- **Bind.** When enabled, on startup the server binds `TcpListener` to
  `127.0.0.1:0` (OS picks a free port). Single listener, not a pool.
- **Connection-info file.** The server writes a JSON file describing how to
  reach it. Two locations, picked in order:
  1. The directory containing the exe (preferred, survives restart of the
     terminal that launched the exe)
  2. `std::env::temp_dir()` if the exe directory is not writable (e.g.
     installed under `Program Files`)
  - Filename: `harness.control.json`
  - Shape:
    ```json
    {
      "version": 1,
      "pid": 12345,
      "addr": "127.0.0.1",
      "port": 51423,
      "boot_token": "base64-256-bit-random",
      "started_at": "2026-08-12T10:36:46Z",
      "build": { "git_sha": "abc1234", "channel": "release" }
    }
    ```
  - **Atomic write** via `tempfile::NamedTempFile::persist` — a half-written
    file would be unrecoverable.
  - **Cleanup.** On graceful shutdown, delete the file. On crash, leave it
    and let the next launch overwrite. Clients must validate `pid` is alive
    before trusting the file.
- **One-shot auth token.** The `boot_token` is consumed on the first
  successful auth. Subsequent connections require either:
  - A `session_token` issued by the server after the first auth, OR
  - A fresh boot_token from a re-read of the connection-info file (which
    the server will have rotated, see below)
- **Token rotation.** Every 5 minutes, OR after the first auth of a
  `boot_token`, the server writes a new connection-info file with a fresh
  `boot_token` and invalidates all existing `session_token`s. Rationale: a
  long-lived shell script that walks away leaves a stale token in its
  memory; rotation is the cheap defense.
- **Graceful shutdown** deletes the file. `system.shutdown` method (write
  tier) does this on the user's behalf.

### 2.2 Wire format

JSON-RPC 2.0 with the following pragmatic relaxations:

- Clients may omit `jsonrpc: "2.0"` for ergonomics (the server adds it on
  response).
- Newline-delimited framing (one JSON object per line), no batch.
- Notifications (no `id`) are allowed from server to client for pushed
  events; client-to-server notifications are not used.
- Standard error codes: `-32700` (parse), `-32600` (invalid request),
  `-32601` (method not found), `-32602` (invalid params), `-32603`
  (internal). The server adds two custom codes:
  - `-32001` write-tier disabled (call requires the write env var)
  - `-32002` auth required (call before `auth` method)
- A typed error envelope includes a `data` field with a structured payload
  when useful (e.g. validation errors point at the offending field).

### 2.3 Method taxonomy

Read-tier ships first; write-tier methods are gated by `HARNESS_CONTROL_WRITE=1`.
All methods are synchronous (request → response) unless explicitly noted as
event-driven (subscribe).

**`system.*`**

| Method | Tier | Notes |
|---|---|---|
| `system.ping` | read | Returns `{pong, uptime_ms}`. |
| `system.info` | read | Version, git SHA, channel, platform, webview kind, uptime. |
| `system.shutdown` | write | Graceful shutdown; deletes connection-info file. |
| `system.reload_webview` | write | Reloads the React UI without restarting Rust. |

**`workspace.*`**

| Method | Tier | Notes |
|---|---|---|
| `workspace.list` | read | Tree of active workspace. `depth` param (default 4, max 8). |
| `workspace.read` | read | File contents. Refuses files >5MB (returns -32603 with reason). |
| `workspace.stat` | read | File metadata. |
| `workspace.search` | read | ripgrep wrapper. `pattern`, `glob`, `case`, `max_results` (default 200). |
| `workspace.open` | write | Set active workspace path. Validates the path is a directory. |
| `workspace.write` | write | File write. Same path validation as the existing IPC bridge. |

**`session.*`**

| Method | Tier | Notes |
|---|---|---|
| `session.list` | read | All sessions with metadata. |
| `session.get` | read | Full session transcript. |
| `session.create` | write | New session, returns `session_id`. |
| `session.send` | write | Inject user message, returns `task_id`. |
| `session.abort` | write | Stop in-flight task. |

**`agent.*`**

| Method | Tier | Notes |
|---|---|---|
| `agent.list` | read | Active agents with state, current task, last activity, iter count, error count, queue depth. |
| `agent.get` | read | Detailed agent state including tool call history, token use, message queue snapshot. |
| `agent.dispatch` | write | Kick a goal into the Orchestrator without a user chat. |
| `agent.stop` | write | Halt a specific agent. |
| `agent.inject` | write | Push a system message into an agent's context. Used by Health Agent. |

**`canvas.*`**

| Method | Tier | Notes |
|---|---|---|
| `canvas.snapshot` | read | JSON dump of all current surfaces (chat, file tree, sub-tiles, ActiveJobBar). Returns a `snapshot_id`. |
| `canvas.diff` | read | Diff between two snapshot ids. Returns the structural delta. |
| `canvas.screenshot` | write | Base64 PNG of the WebView. Heavy; write tier because it can be slow. |
| `canvas.subscribe` | read | Server pushes `events.canvas.changed` when the canvas state mutates. |

**`events.*`**

| Method | Tier | Notes |
|---|---|---|
| `events.subscribe` | read | Register interest in `agent.*`, `session.*`, `canvas.*`, `system.*` events. Server emits `{"method": "events.<category>.<event>", "params": {...}}` notifications. |

### 2.4 Security model

- **Localhost only.** Bind `127.0.0.1`, not `0.0.0.0`. Reject peer IPs that
  aren't loopback at the kernel level (already enforced by the bind).
- **One-shot token, rotating.** The `boot_token` is consumed on first use.
  Session tokens are rotated every 5 minutes. This blocks a second process
  from reading the file and racing.
- **No general command execution.** There is no `system.exec` method. All
  capabilities are typed methods with bounded payloads. The closest thing
  to "run a command" is the existing `cli.exec` Tauri command, which is
  exposed as `workspace.exec` here with the same argument-validation as
  the IPC bridge already enforces.
- **File system scoping.** All `workspace.*` methods are scoped to the
  active workspace. Cross-workspace access requires `workspace.open` first.
- **Rate limiting.** Token bucket per connection: 100 calls/sec, burst 200.
  Beyond that, the server closes the connection. This stops a buggy client
  from wedging the Rust side.
- **Audit log.** Every write-tier call is logged with `{timestamp, method,
  params_hash, peer_pid_if_available}`. The log lives in the existing
  Tauri logging path; no separate file.

### 2.5 Failure modes (explicit)

| Failure | Server response |
|---|---|
| TCP listener bind fails | Log a warning, do NOT write the connection-info file, continue without HCI. The UI is unaffected. |
| Connection-info file write fails | Log a warning, HCI is enabled but unreachable by file-based clients. Direct-port clients can still connect if they know the port (they won't, normally). |
| Client sends malformed JSON | Standard `-32700` parse error. |
| Client calls method before auth | `-32002`. |
| Client calls write method without write tier | `-32001`. |
| Method handler panics | Caught, logged, returned as `-32603` with a UUID for log correlation. The connection stays open. |
| Server crashes | Process dies, file lingers, next launch overwrites. |

---

## 3. Health Agent v0

### 3.1 What it is

A periodic, ephemeral, in-process meta-agent. It runs every N seconds,
observes the active session, optionally messages or stops sub-agents, and
terminates. It is **not** a long-running daemon, **not** a chatbot, and
**does not** talk to the user directly.

### 3.2 Lifecycle

```
[Settings tick fires every T seconds]
    │
    ▼
[Capture SessionSnapshot] ──────► existing HCI read methods
    │
    ▼
[Spawn Health Agent task with timeout D]
    │
    │  Context = { snapshot, allowed methods, model id, history (last 3 verdicts) }
    │
    ▼
[Agent runs an inner loop]
    │
    │  on each iter: decide → call allowed method → observe → repeat
    │
    ▼
[Return Verdict ∈ {ok, remediated, escalate, error}]
    │
    ▼
[Log structured alert, write to session log if remediated/escalate]
    │
    ▼
[Terminate]
```

- **T (tick interval):** configurable in Settings, 10s–5min, default 30s.
- **D (per-tick max duration):** configurable, 10s–60s, default 20s. Hard
  timeout — if the agent exceeds D, the task is cancelled and a
  `timeout` verdict is logged.
- **Model:** user-selectable in Settings, separate from the chat model.
  Defaults to the same model as the chat if not set. A small fast model
  (Haiku-class) is recommended for cost.
- **History:** the agent gets the last 3 verdicts in its context, so it
  can see "I already tried stopping this agent last tick, don't retry".

### 3.3 What it can do (v0 surface)

**Read methods (always available):**
- `agent.list`, `agent.get` — observe any sub-agent
- `session.get` — full transcript of the current session
- `events.subscribe` — tail real-time events for the duration of the tick

**Write methods (require `HARNESS_CONTROL_WRITE=1`):**
- `agent.inject` — push a corrective message into a specific sub-agent
- `agent.stop` — halt a stuck agent
- `session.send` — send a message on behalf of the user (used for the
  "the user should know about this" escalation)

**Methods explicitly NOT exposed to Health Agent in v0:**
- `agent.dispatch` — re-dispatch to a different agent (deferred, see
  open questions)
- `workspace.*` — out of scope for a meta-agent
- `system.*` — out of scope

### 3.4 Detection heuristics (v0)

These are the triggers that cause the Health Agent to be spawned:

1. **Stuck iteration.** A single agent has been on the same task for >4
   consecutive iterations with no progress (no new file writes, no new
   tool results, no reduction in error count).
2. **Error spike.** An agent has produced >3 errors in the last 30s.
3. **Queue depth.** An agent's message queue is >50 messages deep.
4. **Resource pressure.** RSS > 250MB OR webview memory > 200MB (probed
   on Windows via `psapi`, on Unix via `/proc/self/status`).
5. **Idle orchestrator.** The orchestrator has no active sub-tasks for
   >5min but the session is "active" (user sent a message in the last
   10min). This catches a silently dead loop.

The Health Agent receives the trigger in its context and uses its own
judgment about what to do. The heuristics are signals, not decisions.

### 3.5 Verdict semantics

| Verdict | Meaning | UI effect |
|---|---|---|
| `ok` | Nothing wrong, or no action needed. | None. |
| `remediated` | Agent acted; situation resolved. | Subtle "Health" indicator on the ActiveJobBar with a hover popover. |
| `escalate` | Agent could not resolve; user should know. | Same as `remediated` but the popover copy says "Health Agent couldn't auto-resolve". One entry in the session log. |
| `error` | The Health Agent itself errored. | Logged; surfaces in the next `system.info` if user queries. |
| `timeout` | The per-tick duration was exceeded. | Logged as `error` for now; a future version may auto-tune D. |

### 3.6 Settings UI

A new section in `Settings.tsx` (only visible when the build has HCI
enabled, which is dev builds by default):

- **Enable Health Agent** (toggle, default ON in dev, OFF in release)
- **Model** (dropdown of model entries, defaults to chat model)
- **Tick interval** (slider 10s–5min, default 30s)
- **Per-tick max duration** (slider 10s–60s, default 20s)
- **Detection thresholds** (collapsible advanced; default values for
  each of the 5 heuristics, "reset to defaults" button)

The setting is per-workspace (stored in the workspace state once that
exists, falls back to a global config in `localStorage` for v0).

---

## 4. Binary size & RAM optimization

### 4.1 Where the size is today

Hypothesized audit (to be verified in the implementation pass):

| Component | Estimated size | Notes |
|---|---|---|
| Tauri runtime | ~1.5MB | Hard to reduce; baseline. |
| Rust agent loop + IPC | ~0.5–1.0MB | The multi-agent graph and shared state. |
| Embedded webview shell (skeleton HTML) | ~2KB | Trivial. |
| Frontend assets (loaded externally today) | ~0.5–1.0MB | React, the components, the glass CSS. |
| **Total exe as-is** | **~2.5–3.5MB** | The exe itself is small. The dist/ folder is what we ship alongside. |

**The actual user-facing "size"** is `harness.exe` + `dist/` + `WebView2
bootstrapper` (if not pre-installed on Windows). On a fresh Windows
machine without WebView2 runtime, that bootstrapper is ~100MB. The
"portable" install is therefore dominated by the webview runtime, not
our code.

### 4.2 5MB target — what it actually requires

The 4–5MB Tauri exes people cite (e.g. certain AI IDE terminals)
typically have:

1. **Embedded frontend assets.** The `dist/` HTML/JS/CSS is baked into
   the exe via `include_bytes!` and served from memory by Tauri's
   custom protocol. No external `dist/` folder. Saves a few hundred KB
   of file system overhead AND makes the exe self-contained.
2. **Release-profile optimization.** `lto = "fat"`, `codegen-units = 1`,
   `strip = "symbols"`, `opt-level = "z"` (or `"s"` for size). Standard
   Rust release tricks.
3. **Compile-time feature pruning.** Disable any Tauri features we
   don't use (e.g. tray icon if unused, deep-link if unused,
   updater if unused). The default `tauri = { version = "2", features
   = ["wry"] }` is already lean, but worth checking.
4. **WebView2 runtime check.** On Windows, we should detect WebView2
   at startup and prompt to install only if missing, instead of
   shipping a 100MB bootstrapper. Users on Windows 10+ usually have it
   pre-installed.
5. **No dev-mode bloat in release.** `tauri::Builder` should not
   register the devtools IPC in release builds. Trivial but easy to
   forget.

**Estimated size after these passes: 2.5–4MB exe, no external
dist/, no bundled webview runtime.** That's our 5MB target met.

### 4.3 RAM optimization — be realistic

The WebView2 process on Windows (essentially a Chromium renderer) has
an irreducible baseline of ~60–80MB resident at idle for a React SPA.
A Tauri app with one WebView typically shows:

- **Idle (no agents, just the UI):** ~80–120MB total process RSS
  (Rust exe ~10–20MB, WebView2 ~60–100MB)
- **Active (multi-agent loop, canvas open):** ~150–250MB
- **Heavy (5+ concurrent agents, large workspace):** ~300–500MB

**We cannot get this to 5MB.** The webview itself is a Chromium-class
process. If we wanted 5MB RAM, we'd need to drop Tauri's webview model
and use a smaller rendering surface (Slint, egui, Druid), but that
sacrifices the React UI that is the product's whole point.

**What we CAN do to reduce RAM:**

1. **Lazy-load the WebView.** Tauri's `WebviewWindow` can be created on
   demand, not at app startup. If the user only uses CLI commands for
   a session, the WebView process never spawns. Saves ~60MB.
2. **One WebView, not many.** Tauri supports multiple windows; we
   don't need them. The Dynamic Canvas uses iframes or in-DOM panels,
   not separate WebViews. Confirmed: today we have one WebView.
3. **Trim React bundle.** The frontend is the bulk of the
   WebView's working set. Tree-shake Lucide icons (we import the full
   set today), use `React.lazy` for the Settings modal and other
   non-default surfaces, drop unused tailwind classes. Estimated
   reduction: 10–20% of the working set, ~10–20MB.
4. **WebView2 disk cache.** The webview maintains a disk cache. We
   should set its quota explicitly (e.g. 10MB) so it doesn't bloat
   over time. Minor; not a huge lever.
5. **Trim the agent loop's memory footprint.** The orchestrator keeps
   the full message history in memory. For a long session, this can
   grow to MBs. Compression to summary blobs after N messages is a
   future optimization, not v0.
6. **Profile on Windows with `psapi` and on macOS with `vmmap`.** The
   health agent can include a memory probe in its `system.info`
   response so we can see real numbers, not guesses.

**Realistic RAM target:** ~80MB idle (with the WebView pre-loaded,
which is the default for UX reasons), ~60MB if we add the lazy-load
option. A future v1 can hit 40–50MB idle by using a Tauri alternative
or a non-webview UI, but that's a different product.

### 4.4 What goes in this spec vs. deferred

| In scope for this spec (v0 HCI + size) | Deferred to a follow-up spec |
|---|---|
| HCI server, auth, method router, read tier | Write tier env var (design only; implement in v0.5) |
| Health Agent lifecycle + heuristics | Health Agent v1 with re-dispatch |
| Embed `dist/` into exe | Lazy WebView creation |
| Release-profile cargo config | Frontend bundle tree-shaking (needs a frontend audit) |
| WebView2 detection on Windows | WebView2 cache quota tuning |
| Cargo feature pruning | Compression of long session history |
| Memory probe in `system.info` | Platform-specific UI for memory display |

---

## 5. Testing strategy

### 5.1 HCI unit tests (Rust)

- **Auth flow.** First connection consumes `boot_token`; second
  connection without `auth` is rejected.
- **Token rotation.** A connection authenticated at T0 loses its
  session_token at T+5min; subsequent calls get `-32002`.
- **Method routing.** Each method dispatches to the right handler.
- **Write-tier gating.** A write method returns `-32001` when
  `HARNESS_CONTROL_WRITE` is not set.
- **Rate limit.** 200 calls in a second from one connection → the
  201st is rejected and the connection is closed.
- **Connection-info file.** Atomic write; deleted on shutdown;
  overwritten on rotation.
- **Panic isolation.** A handler that panics returns `-32603` and the
  connection survives.

### 5.2 HCI integration tests (Python client)

- Spawn `harness.exe` in headless mode (a test-only `--no-window` flag
  is added in this work), wait for the connection-info file, connect,
  exercise the full read tier.
- Verify that `canvas.snapshot` matches the in-process state after a
  scripted sequence of WebView events (driven via the existing dev
  test harness).

### 5.3 Health Agent tests

- **Trigger detection.** A scripted agent loop with 5 stuck iterations
  causes the Health Agent to be spawned within T+1 seconds.
- **Verdict propagation.** An `escalate` verdict appears in the
  session log within D+1 seconds.
- **Timeout enforcement.** An agent that does not return a verdict
  within D seconds is cancelled, and the timeout verdict is logged.
- **History passes through.** The 4th tick after 3 prior `remediated`
  verdicts includes those verdicts in the agent's context.

### 5.4 Binary / RAM tests (CI, not unit)

- **Size budget.** CI fails if `harness.exe` after a clean release
  build exceeds 5MB. (We will probably relax this to a warning at
  first and tighten as we go.)
- **RAM budget.** CI runs a headless harness, captures RSS at 5s
  intervals, fails if idle RSS > 120MB. (Same caveat.)
- **Smoke test.** The CI-built exe launches, opens the connection-info
  file, accepts one read call, exits cleanly.

---

## 6. Migration & rollout

- **Phase 1 (this spec, behind a feature flag).** HCI and Health Agent
  compile in but are OFF by default. Opt-in via env var and Settings
  toggle. Binary optimization passes land in this phase too.
- **Phase 2 (next minor).** HCI is ON by default in dev builds, OFF in
  release. Health Agent is ON in dev. Write tier still requires the env
  var.
- **Phase 3 (after we've shaken it out).** Decide based on telemetry
  from the outer agent loop whether to ship HCI on by default in
  release builds. Probably not — the existing IPC bridge is what users
  care about.

The Health Agent and HCI do not break any existing surface. They are
additive.

---

## 7. Open questions

1. **Re-dispatch in Health Agent v1.** We deferred `agent.dispatch` to
   v1 because re-dispatching an in-flight task cleanly is non-trivial
   (what happens to the in-flight one? cancellation? drain? abandon?).
   Defer the design discussion until we've seen the read+remediate loop
   work in practice for a sprint.
2. **WebView lazy-load.** Tempting for RAM, but breaks the "open the
   exe, see the UI instantly" expectation. We could expose it as a
   `--lazy` flag for headless/automated use. Defer to follow-up spec.
3. **Per-connection rate limit vs. global.** v0 is per-connection. A
   global limit (e.g. 1000 calls/sec across all connections) protects
   against many connections at once. Defer until we see a need.
4. **Cross-platform TcpListener behavior.** macOS and Linux work as
   expected. Windows Defender sometimes flags localhost-bind executables
   that are unsigned. We should sign the binary for distribution
   regardless; not in this spec.

---

## 8. Out of scope

- **A formal RPC schema language (Protobuf, Cap'n Proto).** JSON is
  fine for the v0 surface. If we outgrow it, we'll add it then.
- **Multi-process Harness (worker subprocesses).** The current
  single-process model is intentional and simple.
- **A Tauri alternative (e.g. a webview-less UI).** Tauri is the right
  choice for the React UI; we'd lose too much to switch.
- **Telemetry / crash reporting.** Explicitly off per `PRODUCT.md`.
- **An agent that talks to the user directly.** That's the chat
  surface, not Health.
