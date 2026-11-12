# opurom-core

Round 2/3 **kernel slice** of the OPUROM kernel. This is one independent Rust crate and one
independent local git repo; it does not import or link any other OPUROM project. It implements
the state/event part of [IR v0.1](../opurom-spec/IR_SCHEMA.md) plus the R2 additions from
[IR_R2_ADDENDUM.md](../opurom-spec/IR_R2_ADDENDUM.md), the process-isolated plugin host
from [PLUGIN_PROTOCOL.md](../opurom-spec/PLUGIN_PROTOCOL.md), and the R3 deterministic
cooperative scheduler from [R3_ADDENDUM.md](../opurom-spec/R3_ADDENDUM.md). It is **not** the
whole framework: no renderer, compiler, target emitters, platform adapters, GUI or build
pipeline live here.

## Build and verify

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cd /Users/mac/work/opurom/opurom-core
cargo fmt --all
cargo test
```

Verified on Rust 1.99.1: **75 integration tests passed (20 from round 1, 38 from round 2,
17 new), 0 failed, 0 ignored; 0 doc tests; zero compiler warnings**. Dependencies are
`serde_json` and `serde_core` (no derive, no network fetch needed beyond the crates.io index).

## API

```rust
use opurom_core::{Event, Runtime, Value};

let json = std::fs::read_to_string("app.opuir.json")?;
let mut runtime = Runtime::from_ir_json(&json)?;      // validates, never panics on bad IR
assert_eq!(runtime.state("count"), Some(&Value::Int(0)));
assert_eq!(runtime.version(), 0);

let patches = runtime.dispatch(Event::Tap { node_id: 4 })?;  // +1
assert_eq!(patches[0].value, Value::Int(1));
assert_eq!(runtime.version(), 1);
let _ = runtime.snapshot();                          // source-ordered Vec<Patch>
# Ok::<(), Box<dyn std::error::Error>>(())
```

| Item | Notes |
| --- | --- |
| `Ir::parse(&str) -> Result<Ir, CoreError>` | Validates `ir == "0.1"`, state types (`int`/`bool`/`string` with matching `init`), unique state names, unique `u32` node ids, and action legality. `add` needs an int target + integer value, `toggle` needs a bool target and no value, `set` needs a type-matching value. A `set` without value is legal only on `input` events with a string target. `props` and `theme` are preserved but not interpreted. |
| `Node.events` | `HashMap<String, Vec<Action>>`: an event maps to an **ordered action list**. A v0.1 single action object is stored as a one-element list, so every round 1 IR file keeps working. |
| `Node.when` | `Option<String>`: the optional conditional node field is **preserved**; the kernel does not interpret it (hiding subtrees is a renderer concern). |
| `Action` | `{ op, target, value: Option<Value>, value_from: Option<String> }`. `value` and `value_from` are mutually exclusive. |
| `Runtime::from_ir_json(&str)` | Builds the state map and the node-id → event-name → ordered action list. Malformed JSON and malformed IR return readable `CoreError`s; no `unwrap`/`panic` on input. |
| `Runtime::state(name)`, `snapshot()`, `version()` | `snapshot()` returns `Vec<Patch>` in state declaration order. |
| `Runtime::dispatch(Event)` | `Event::Tap { node_id }` / `Event::Input { node_id, text }`. Applies every action of the bound event **in order and atomically**: the actions run against a working copy, and the copy is committed only when all of them succeed. Patches are emitted in action order, one per resulting state change. `add` uses `checked_add`, so overflow is an error with no state change. Errors (unknown node, unbound event, bad type) leave state and version untouched. |
| `Patch`, `Value` | `Patch { name, value }`; `Value::Int(i64) \| Bool(bool) \| Str(String)`. Both serialize with `serde_json::to_string` as `{"name":"count","value":1}`. |
| `Scheduler::new(workers)`, `spawn(priority, task)`, `shutdown()` | Bounded std-thread pool, `BinaryHeap` priority (higher `u8` first, FIFO within a priority), `TaskHandle::join() -> Result<T, SchedulerError>`. `shutdown` stops new submissions, drains queued work, joins workers; `Drop` does the same. Task panics are caught, so a worker survives. |
| `AsyncScheduler::new()`, `spawn(priority, task) -> TaskId`, `spawn_with_context(priority, task) -> TaskId`, `sleep_until(id, tick)` / `wake_at(id, tick)`, `tick(delta_ms) -> Vec<TaskId>`, `cancel(id) -> bool`, `pending()`, `drain()`, `now()` | Deterministic R3 cooperative scheduler: logical ticks, higher priority first then spawn order, sleeping tasks, cancellation and draining. `spawn` takes a plain `FnOnce()`; `spawn_with_context` passes a `TaskContext` for children/continuations. See [the R3 section](#deterministic-async-scheduler-r3); it is **not** async I/O. |
| `PluginBus::new(failure_threshold)` (default 3), `register`, `call`, `is_disabled` | Handler signature `Fn(&str) -> Result<String, String> + Send + Sync`. Every call runs inside `catch_unwind(AssertUnwindSafe(..))`. Handler `Err` and panics increment a consecutive-failure counter; a success resets it; at the threshold the plugin is disabled and later calls return the distinct `PluginError::Disabled`. Other plugins and the runtime keep working. |
| `Host::new()` / `with_threshold(n)`, `run(primary, fallback)`, `record_error`, `record_success`, `degraded()` | Standalone degradation policy, no adapter crate dependency. Three consecutive primary `Err`s flip `degraded()` to true and `active_adapter()` to `"headless"`; once degraded, `run` calls only the fallback. A success resets the consecutive count. |

### IR R2 additions

`events.<name>` accepts either one action object (v0.1) or a wrapper object:

```json
{ "tap": { "actions": [
    { "op": "add", "target": "count", "value": 1 },
    { "op": "set", "target": "count", "valueFrom": "other" }
] } }
```

- Actions run **in order, atomically**; patches come out in the same order. An empty `actions`
  array is a legal no-op handler (no patch, no version bump).
- `valueFrom: "<state>"` reads the current value of another state **at execution time**, so an
  action sees the values written by the preceding actions of the same event. Both states must
  declare the same type; `add` additionally requires an `int` source. A mismatch is rejected
  when the IR is parsed, not when it runs.
- `toggle` never carries a value: `value` and `valueFrom` are both rejected for it.

## Deterministic async scheduler (R3)

`AsyncScheduler` is a **deterministic, single-threaded, cooperative scheduler**
driven by a logical tick counter: a schedulable coroutine queue. It is **NOT
OS async I/O**, **NOT reactor/epoll**, and **NOT a claim about network
concurrency**: there is no reactor, no epoll/kqueue, no thread pool, no
sockets, no randomness and no wall clock. Real async I/O stays R4+ work. It
implements [section 4 of R3_ADDENDUM.md](../opurom-spec/R3_ADDENDUM.md).

```rust
use opurom_core::{AsyncScheduler, TaskId};

let mut sched = AsyncScheduler::new();
assert_eq!(sched.now(), 0);                    // logical time starts at 0

let low = sched.spawn(1, || {});               // queued for the next tick
let high = sched.spawn(9, || {});
let later = sched.spawn(1, || {});
assert!(sched.sleep_until(later, 5));          // wake_at(later, 5) is an alias

let ready: Vec<TaskId> = sched.tick(0);
assert_eq!(ready, vec![high, low]);            // higher priority, then spawn order
assert_eq!(sched.tick(5), vec![later]);        // ready exactly at its target tick
assert_eq!(sched.now(), 5);                    // advanced only by tick
assert_eq!(sched.pending(), 0);
```

Rules, all deterministic:

- `tick(delta_ms)` advances the logical clock (saturating), wakes every task
  whose target tick has arrived, runs the whole ready batch (freshly woken
  tasks and anything already queued) and returns the ids that ran, ordered by
  higher priority first and spawn order second. `drain()` runs the ready batch
  without advancing time or waking sleepers. `tick(0)` flushes ready tasks.
- `sleep_until(id, tick)` (alias `wake_at`) defers a pending task; retargeting
  a sleeping task updates its tick; a target at or before `now()` makes the
  task ready immediately. `cancel(id)` removes a pending or sleeping task.
  `pending()` counts tasks that have neither run nor been cancelled; `now()`
  starts at 0, advances only in `tick`, and equals 0 at construction.
- `spawn` takes a plain one-shot `FnOnce()`. For tasks that need the
  scheduler, `spawn_with_context` passes a small `TaskContext` with
  `id()`, `now()`, `spawn(priority, task)` for children, and
  `spawn_at(priority, tick, task)`: a future-dated continuation, which is the
  ergonomic way to "reschedule" since a one-shot closure cannot run twice.
  The context deliberately exposes no `drain`/`cancel`/`tick`, so a running
  task cannot re-enter the run loop or move the clock mid-batch. Children and
  continuations join the queue for a later tick, never the current batch.
- The same spawn/tick sequence on two schedulers always yields the same ready
  sequence: sorting is a total order of `(priority, spawn order)`, and no
  `HashMap`, clock, thread or random source influences it. A panicking task
  unwinds into the caller of `tick`/`drain`.

## Plugin ABI (protocol v1)

Plugins are **separate processes**. A plugin crash, hang or protocol violation is contained by
the OS process boundary — the kernel never links plugin code into its own address space.

### Transport

One JSON object per line on stdin (request), one JSON object per line on stdout (response).
Logs must go to stderr; stdout carries protocol traffic only.

```json
{"id":0,"method":"hello","protocol":"1.0"}
{"id":0,"ok":true,"result":{"name":"camera","version":"0.1.0","capabilities":["camera.takePhoto"]}}
{"id":1,"method":"invoke","capability":"camera.takePhoto","params":{"quality":"high"}}
{"id":1,"ok":true,"result":{"path":"/tmp/photo.png"}}
```

Responses must echo the request `id`. A malformed JSON line, a wrong id, a missing `ok`, a
missing `result`/`error`, or no answer within the timeout is a **protocol violation**.

### Manifest

```json
{
  "name": "camera",
  "version": "0.1.0",
  "protocol": "1.0",
  "capabilities": ["camera.takePhoto"],
  "entry": "./bin/camera",
  "description": "Camera access"
}
```

Validated **before spawn**: `name` matches `[a-z][a-z0-9-]{1,31}`, `version` is semver-ish
`x.y.z`, `protocol == "1.0"`, `capabilities` is a non-empty list of dotted names, and `entry`
resolves to an executable file **inside** the plugin directory (absolute paths, `..` traversal
and symlinks pointing outside are rejected).

### Host API

| Item | Notes |
| --- | --- |
| `PluginHost::new(allowlist)` | The allowlist is the app-supplied set of capabilities the app is willing to call. A capability outside it is refused with `PluginError::NotAllowed` before any request is written. |
| `PluginHost::with_timeout(Duration)` | Per-request timeout, default **500 ms**, clamped to a 50 ms minimum. The `hello` handshake gets a wider 2 s window because process startup is not part of the invoke latency budget. |
| `PluginHost::load(dir) -> Result<PluginInfo, PluginError>` | Validates the manifest, spawns `entry`, performs the `hello` handshake and verifies that the advertised capabilities are a **subset** of the declared ones. |
| `PluginHost::invoke(capability, params)` | Checks the allowlist, writes one JSON line, reads exactly one response with the matching id under the timeout, and validates `{id, ok, result\|error}`. |
| `PluginHost::invoke_on(plugin, capability, params)` | Same, but targets one named plugin directly (used when two plugins share a capability name). |
| `PluginHost::list() -> Result<Vec<PluginInfo>, PluginError>` | Reports `name`, `version`, `capabilities`, `status` (`Active`/`Disabled`) and the invocation count. |
| `PluginInfo`, `PluginStatus` | Plain data types, no lifetime or borrow ties to the host. |
| `PluginError` | `NotFound`, `Failed`, `Panicked`, `Disabled`, `Io`, `InvalidManifest`, `Spawn`, `Timeout`, `ProtocolViolation`, `Crashed`, `NotAllowed`, `UnknownCapability`, `AmbiguousCapability`. Every failure is a **value**; no panic crosses the boundary. |

### Fault policy

- 3 consecutive **timeouts / protocol violations / crashes** ⇒ the plugin is marked `Disabled`
  and its process is killed. Other plugins keep serving.
- A successful call resets the counter — but only when no crash happened. A crash is never
  cleared by a later success.
- After a fault the faulty process is killed and the next call starts a fresh process with a
  fresh handshake.
- `catch_unwind` guards the host entry points. It cannot contain an abort, a `SIGSEGV` or a
  `panic = "abort"` build — process isolation is the real containment here.

## Resource manager

`ResourceManager<K, V>` is a bounded LRU cache with a byte budget.

| Item | Notes |
| --- | --- |
| `ResourceManager::new(budget)` | Byte budget for the whole cache. |
| `insert(key, value, size) -> Result<Option<V>, ResourceError>` | Charges `size` bytes (a zero-size value still costs one byte, so the entry count stays bounded). A value larger than the whole budget is rejected with `ResourceError::BudgetExceeded`; otherwise the least recently used entries are evicted until the new entry fits. |
| `get(key)` | Returns a clone and marks the entry as most recently used. |
| `evict(key)` | Removes one entry and refunds its bytes. |
| `used_bytes()`, `len()`, `keys_lru()`, `budget()` | Introspection for diagnostics and tests. |
| `Runtime::invoke_plugin(capability, params, invoke)` | The bridge between the plugin bus and the runtime: it calls the plugin, caches the JSON result under the runtime's byte budget (`DEFAULT_RESOURCE_BUDGET`, 64 KiB) and returns the cached value for a repeated `(capability, params)` pair. Plugin failures surface as `CoreError::Plugin`. |

## CLI

`opurom-core <app.opuir.json>` reads one IR file, prints the initial state, then reads commands
from stdin and prints the resulting patches:

```sh
printf 'tap 4\nstate\nquit\n' | cargo run -- tests/fixtures/counter.opuir.json
```

```
state [{"name":"count","value":0}] version 0
patches [{"name":"count","value":1}] version 1
state [{"name":"count","value":1}] version 1
```

Commands: `tap <id>`, `input <id> <text>` (text may contain spaces), `state`, `quit`.
Bad commands and dispatch errors print `error: <diagnostic>` and the loop continues. Exit codes:
0 on `quit`/EOF, 1 for unreadable or invalid IR, 2 when the file argument is missing. Note that
input lines are whitespace-trimmed, so leading/trailing spaces in input text are not preserved.

`opurom-core plugins <pluginDir> <capability>` is the cross-repo integration entry point: it
loads the plugin, performs the handshake, invokes the capability with `{}` and prints the JSON
result on stdout (exit 0) or a readable diagnostic on stderr (exit 1).

```sh
opurom-core plugins /path/to/plugin camera.takePhoto
{"path":"/tmp/photo.png"}
```

Fixtures in `tests/fixtures/`: `counter.opuir.json` is a copy of the spec's golden IR,
`form.opuir.json` exercises string input, bool toggle and typed set, `bad-version.opuir.json`
is a negative case. `tests/fixtures/plugins/stub/` is a self-contained stub plugin (a POSIX
shell script) used by the round 2 tests; its behaviour is driven by a `schedule` file, so one
fixture covers handshake, success, unknown capability, timeout, crash and protocol violations.

## Honest status

| Area | Implemented and verified here | Not implemented (planned / belongs to another repo) |
| --- | --- | --- |
| IR v0.1 loading | Version, state types, unique names/ids, action legality, malformed-JSON diagnostics; negative tests in `tests/integration.rs` | Node `kind` whitelist, theme values and prop semantics are not validated (fields are preserved); interpolation `{name}` is a renderer concern |
| IR R2 extensions | Multi-action events with ordered, atomic patches; `valueFrom` reads at execution time with same-type and `int` validation; `when` preserved on nodes; empty action lists | Interpretation of `when` (subtree hiding), the other R2 prop/theme/node-kind additions — those belong to the renderer and compiler |
| Reactive state | `int`/`bool`/`string`, `set`/`add`/`toggle`, patches, version counter, overflow and error atomicity | Derived/computed state, multi-state transactions, undo, persistence, animation |
| Scheduler | Bounded worker pool, priority ordering, FIFO tie-break, graceful drain, panic containment; 3 tests | Work stealing, cancellation, timeouts, task metrics, real-time scheduling |
| Async scheduler (R3) | Deterministic cooperative tick queue: priority with spawn-order tie-break, logical-time sleep/wake boundaries, cancel, drain, `TaskContext` children/continuations, determinism; 17 tests. No wall clock, threads or randomness | OS async I/O, reactor/epoll, OS timers, network concurrency; R4+ for real async I/O |
| Plugin bus (in-process) | `catch_unwind` isolation, consecutive-failure circuit breaker, `PluginError::Disabled`, default threshold 3; 2 tests | Unload/reload, recovery after cooldown |
| Plugin host (process) | Manifest validation with traversal guard, spawn, `hello` handshake, capability-subset check, app allowlist, 500 ms timeout, protocol validation, 3-strike `Disabled` breaker, crash-vs-timeout counter rule, `list()`; 18 tests against a real stub process | **No sandboxing**: no seatbelt/landlock/seccomp, no filesystem or network namespace restriction, no resource limits (rlimits) for the child. **No plugin signing or signature verification**, no trust chain, no version pinning. **No remote marketplace** — the local `opurom-plugins/registry.json` index and `opurom plugin verify` live in `opurom-plugins` |
| Resource manager | Bounded LRU cache with a byte budget, deterministic eviction order, budget-exceeded error, zero-size handling; 6 tests | Memory pressure heuristics, per-plugin quotas, cache persistence, metrics |
| Runtime ↔ plugin bus | `Runtime::invoke_plugin` caches plugin results under the runtime byte budget and reports plugin errors as values | Automatic plugin discovery, capability-to-state mapping, streaming/long-lived plugin calls |
| Adapter degradation | Consecutive-error counting, threshold 3, fallback-only `run`, `degraded()`; 2 tests | Real `PlatformAdapter` trait, actual adapter swap, recovery, frame presentation — those live in `opurom-platform`. This `Host` is a small standalone policy with no cross-repo dependency |
| CLI | IR file + stdin command loop, patch output, error diagnostics; 3 end-to-end binary tests; `plugins <dir> <capability>` subcommand | `opurom build/dev/test`, hot reload, watch mode (see `opurom-cli`) |
| Renderer / pipeline | Nothing | RGBA output, layout, dirty rects, deterministic pixel hash, target emitters, GPU backend, LSP, debugger |
| Quality evidence | `cargo test`: 75 passed, 0 failed, no warnings | No performance, fps, cold-start or binary-size numbers are reported: no GPU backend or device matrix exists yet. Pixel-consistency and golden-hash checks belong to `opurom-render`/`opurom-examples` |

**Gap summary:** a narrow, tested kernel slice, not a shipping SDK. Process isolation of
plugins is real and verified, but it is **not** a security boundary: a plugin still runs with
the kernel's user privileges until R3 adds OS-level sandboxing. The R3 scheduler is a
deterministic queue only: it never performs I/O, and real async I/O/network concurrency
remains R4+ work. Cross-repo end-to-end execution, real platform adapters, renderer
output and any benchmark figures are outside this round.
