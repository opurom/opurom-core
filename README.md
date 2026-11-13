# opurom-core

**Kernel slice** of the OPUROM kernel. This is one independent Rust crate and one
independent local git repo; it does not import or link any other OPUROM project. It implements
the state/event part of [IR v0.1](../opurom-spec/IR_SCHEMA.md) plus the interactive UI additions from
[IR_ADDITIONS.md](../opurom-spec/IR_ADDITIONS.md), the process-isolated plugin host
from [PLUGIN_PROTOCOL.md](../opurom-spec/PLUGIN_PROTOCOL.md), the deterministic
cooperative scheduler from [COMPONENTS_AND_A11Y.md](../opurom-spec/COMPONENTS_AND_A11Y.md) section 4,
the multi-threaded work-stealing scheduler from
[PRODUCTION_BREADTH.md](../opurom-spec/PRODUCTION_BREADTH.md) section 5,
and the explicit native-capability permission gate from
[COMPONENT_LIBRARY_HARDENING.md](../opurom-spec/COMPONENT_LIBRARY_HARDENING.md) section 4.
It is **not** the whole framework: no renderer, compiler, target emitters, platform adapters,
GUI or build pipeline live here.

## Build and verify

```sh
cd opurom-core
cargo fmt --all
cargo test --offline
RUSTFLAGS="-D warnings" cargo test --offline
cargo run --offline --example parallel_speedup
```

`cargo test --offline -- --nocapture` also prints the work-stealing scheduler test's measured
`Instant` timings.
Integration tests cover IR validation, interactive UI actions, conditional event dispatch, the
deterministic cooperative scheduler, the work-stealing scheduler and native capability permissions.
The Rust suite contains 101 integration tests and no unit or doc tests. Dependencies are
`serde_json` and `serde_core`; the work-stealing scheduler and the capability gate use pure `std`
and add no crate. The capability tests drive a real shell plugin process through JSON lines,
including crashes, malformed frames, cooldown recovery and PID reaping.

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
| `Ir::parse(&str) -> Result<Ir, CoreError>` | Validates `ir == "0.1"`, state types (`int`/`bool`/`string` with matching `init`), unique state names, unique `u32` node ids, `when` conditions naming declared `bool` states, and action legality. `add` needs an int target + integer value, `toggle` needs a bool target and no value, `set` needs a type-matching value. A `set` without value is legal only on `input` events with a string target. `props` and `theme` are preserved but not interpreted. |
| `Node.events` | `HashMap<String, Vec<Action>>`: an event maps to an **ordered action list**. A v0.1 single action object is stored as a one-element list, so every earlier IR file keeps working. |
| `Node.when` | `Option<String>`: the conditional node field names a declared `bool` state. The kernel evaluates it for event dispatch; the renderer applies it to visibility. See [conditional event dispatch](#conditional-event-dispatch). |
| `Action` | `{ op, target, value: Option<Value>, value_from: Option<String> }`. `value` and `value_from` are mutually exclusive. |
| `Runtime::from_ir_json(&str)` | Builds the state map and the node-id -> event-name -> ordered action list. Malformed JSON and malformed IR return readable `CoreError`s; no `unwrap`/`panic` on input. |
| `Runtime::state(name)`, `snapshot()`, `version()` | `snapshot()` returns `Vec<Patch>` in state declaration order. |
| `Runtime::dispatch(Event)` | `Event::Tap { node_id }` / `Event::Input { node_id, text }`. Applies every action of the bound event **in order and atomically**: the actions run against a working copy, and the copy is committed only when all of them succeed. Patches are emitted in action order, one per resulting state change. `add` uses `checked_add`, so overflow is an error with no state change. Errors (unknown node, unbound event, bad type) leave state and version untouched. A bound event on a hidden node returns an empty patch list without changing state or version. |
| `Patch`, `Value` | `Patch { name, value }`; `Value::Int(i64) \| Bool(bool) \| Str(String)`. Both serialize with `serde_json::to_string` as `{"name":"count","value":1}`. |
| `Scheduler::new(workers)`, `spawn(priority, task)`, `shutdown()` | Legacy bounded std-thread pool, `BinaryHeap` priority (higher `u8` first, FIFO within a priority), `TaskHandle::join() -> Result<T, SchedulerError>`. `shutdown` stops new submissions, drains queued work, joins workers; `Drop` does the same. Task panics are caught, so a worker survives. This API is unchanged. |
| `Scheduler::with_workers(n)` -> `WorkStealingScheduler` | Builder for N OS worker threads and a bounded total task queue. `spawn(task)` returns a `WorkStealingTaskHandle<T>` with `join() -> Result<T, SchedulerError>`; `spawn_on(worker, task)` pins a task to one worker's deque and gives its closure the executing worker id. Builder methods: `with_queue_capacity`, `with_failure_threshold`, `with_cooldown`, `with_clock`. One worker is deterministic FIFO; more workers use round-robin injection, per-worker deques and stealing. `shutdown()` drains work and joins every thread; `Drop` does the same. |
| `AsyncScheduler::new()`, `spawn(priority, task) -> TaskId`, `spawn_with_context(priority, task) -> TaskId`, `sleep_until(id, tick)` / `wake_at(id, tick)`, `tick(delta_ms) -> Vec<TaskId>`, `cancel(id) -> bool`, `pending()`, `drain()`, `now()` | Deterministic cooperative scheduler: logical ticks, higher priority first then spawn order, sleeping tasks, cancellation and draining. `spawn` takes a plain `FnOnce()`; `spawn_with_context` passes a `TaskContext` for children/continuations. See [the scheduler section](#deterministic-async-scheduler); it is **not** async I/O. |
| `PluginBus::new(failure_threshold)` (default 3), `register`, `call`, `is_disabled` | Handler signature `Fn(&str) -> Result<String, String> + Send + Sync`. Every call runs inside `catch_unwind(AssertUnwindSafe(..))`. Handler `Err` and panics increment a consecutive-failure counter; a success resets it; at the threshold the plugin is disabled and later calls return the distinct `PluginError::Disabled`. Other plugins and the runtime keep working. |
| `Host::new()` / `with_threshold(n)`, `run(primary, fallback)`, `record_error`, `record_success`, `degraded()` | Standalone degradation policy, no adapter crate dependency. Three consecutive primary `Err`s flip `degraded()` to true and `active_adapter()` to `"headless"`; once degraded, `run` calls only the fallback. A success resets the consecutive count. |
| `NATIVE_CAPABILITIES`, `CapabilityHost::new(grants)` | Explicit grants for exactly `camera.takePhoto`, `geolocation.getCurrentPosition`, `bluetooth.scan`, `notification.show`, `payment.request` and `share.send`. `register(dir)` validates the manifest without spawning; `invoke(capability, params)` checks catalog, grant and declaration before lazy-loading the plugin. Denial returns `PluginError::NotAllowed`; an undeclared capability returns `PluginError::UnknownCapability`; neither starts a process. |
| `CapabilityHost` / `PluginHost` builders and introspection | `with_failure_threshold(n)`, `with_cooldown(ticks)`, `with_clock(Arc<dyn Clock>)`, `with_timeout(Duration)`, `consecutive_failures(plugin)`, `circuit_open(plugin)`, `breaker_state(plugin)`, `retry_after(plugin)`, `shutdown()`. Builder methods configure the breaker before `load`/`register`; `shutdown` kills and reaps all plugin processes. |
| `BreakerState`, `PluginStatus::CircuitOpen`, `PluginError::CircuitOpen { retry_after }` | Breaker states are `Closed`, `HalfOpen` and `Open`. After cooldown one half-open trial closes on success or reopens on failure. The legacy default remains a permanent `Disabled` status after three faults; finite cooldown is opt-in. |

### Interactive UI IR additions

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
- A node carrying `when` is inert for event dispatch while its condition chain is false;
  see [conditional event dispatch](#conditional-event-dispatch).

### Conditional event dispatch

A node may carry `"when": "<boolStateName>"`. `Ir::parse` requires the name to be a declared
`bool` state, and each node inherits every condition declared by its ancestors. `Runtime::dispatch`
evaluates the inherited chain against the current state at dispatch time: while any condition is
false, a bound event on that node is inert — no action runs, no patch is emitted and the version
does not change. Both `tap` and `input` events follow this rule, and unknown nodes and unbound
events keep reporting their errors. Guards read state at dispatch time, so an event that flips a
condition immediately changes which nodes accept later events. In the example, node 7 carries a
`when` condition on `visible`.

```rust
use opurom_core::{Event, Runtime, Value};

let json = std::fs::read_to_string("app.opuir.json")?;
let mut runtime = Runtime::from_ir_json(&json)?;
assert_eq!(runtime.state("visible"), Some(&Value::Bool(false)));
assert!(runtime.dispatch(Event::Tap { node_id: 7 })?.is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Deterministic async scheduler

`AsyncScheduler` is a **deterministic, single-threaded, cooperative scheduler**
driven by a logical tick counter: a schedulable coroutine queue. It is **NOT
OS async I/O**, **NOT reactor/epoll**, and **NOT a claim about network
concurrency**: there is no reactor, no epoll/kqueue, no thread pool, no
sockets, no randomness and no wall clock. It implements
[section 4 of COMPONENTS_AND_A11Y.md](../opurom-spec/COMPONENTS_AND_A11Y.md).

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

## Parallel work-stealing scheduler

`Scheduler::with_workers(n)` builds a `WorkStealingScheduler` using only `std::thread`,
`Mutex`, `Condvar`, `VecDeque` and channels. It starts N real worker threads. Each thread owns
a deque; `spawn` injects tasks round robin. A worker pops from its own deque (back for N > 1)
and, when idle, steals from the front of another worker's deque. `with_queue_capacity` limits
the total number of queued tasks across all deques; a full queue rejects `spawn` with
`SchedulerError::QueueFull` instead of blocking. With one worker, submission is a deterministic
round-robin fallback to that one deque and execution is FIFO. With more workers, completion
order is unspecified but every accepted result is delivered.

`catch_unwind` wraps each task. A panic becomes `SchedulerError::TaskPanicked(String)` and does
not kill the worker; later tasks still run. `with_failure_threshold(k)` and
`with_cooldown(ticks)` configure the circuit breaker: k consecutive task failures open it, new
spawns return `SchedulerError::CircuitOpen { retry_after }`, and the first spawn at or after
the injected clock's cooldown closes it. `Clock` / `CounterClock` let tests advance logical
time without wall-clock sleeps; `SystemClock` is the default. A success resets consecutive
failures while the circuit is closed. Panic isolation contains unwinding Rust panics, not
`abort` or a process kill.

`measure_speedup(workers, tasks, iterations)` runs the same CPU-bound integer workload with 1
worker and N workers, times each run with `Instant`, and returns both durations, both result
vectors and `ratio = one_worker / many_workers`. The integration test asserts complete,
identical results and that the ratio was computed from the two durations; it deliberately does
**not** assert a fixed speedup. The example prints the measured numbers:

```sh
cargo run --offline --example parallel_speedup
# optional: cargo run --offline --example parallel_speedup -- 16 300000 8
```

One real debug run on a 12-logical-CPU macOS host (Rust 1.99.1, 16 tasks, 300,000 iterations
per task, 8 workers) printed:

| run | workers | `Instant` elapsed (ms) | result checksum |
| --- | ---: | ---: | ---: |
| 1-worker | 1 | 28.328 | 7297643462028919011 |
| N-worker | 8 | 5.903 | 7297643462028919011 |

Measured ratio: `28.328 / 5.903 = 4.799x`. These are observed numbers from that run, not an
estimate; they vary with host core count, load and build profile.

One mutex protects all deques (not lock-free); there is no cancellation, timeout, task metrics
or real-time scheduling, and no GPU scheduling, OS async I/O, priority preemption or
cross-process scheduling. The legacy `Scheduler::new` priority pool remains separate and
unchanged.

## Plugin ABI (protocol v1)

Plugins are **separate processes**. A plugin crash, hang or protocol violation is contained by
the OS process boundary - the kernel never links plugin code into its own address space.

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
| `PluginHost::list() -> Result<Vec<PluginInfo>, PluginError>` | Reports `name`, `version`, `capabilities`, `status` (`Active`/`CircuitOpen`/`Disabled`) and the invocation count. |
| `PluginInfo`, `PluginStatus`, `BreakerState` | Plain data types, no lifetime or borrow ties to the host. |
| `PluginError` | `NotFound`, `Failed`, `Panicked`, `Disabled`, `Io`, `InvalidManifest`, `Spawn`, `Timeout`, `ProtocolViolation`, `Crashed`, `NotAllowed`, `UnknownCapability`, `AmbiguousCapability`, `CircuitOpen { retry_after }`. Every failure is a **value**; no panic crosses the boundary. |

### Fault policy

- The legacy default is unchanged: 3 consecutive **timeouts / protocol violations / crashes**
  mark the plugin `Disabled` and kill its process. Other plugins keep serving. A crash remains
  sticky in this permanent-disable mode.
- With `with_failure_threshold(n)` and `with_cooldown(ticks)`, the breaker has
  `Closed`, `HalfOpen` and `Open` states. `with_clock` injects a logical clock; an open circuit
  returns `PluginError::CircuitOpen { retry_after }`. At cooldown one trial closes on success or
  reopens on failure. In this mode a success resets the consecutive-failure streak.
- After any fault the faulty process is killed and the next call starts a fresh process with a
  fresh handshake. An abrupt exit or malformed JSON-lines frame is reported as
  `PluginError::Crashed` or `PluginError::ProtocolViolation`; the host process survives.
- `shutdown()` kills and reaps all loaded plugin processes. The capability test checks every
  recorded child PID after crashes, malformed frames and shutdown.
- `catch_unwind` guards the host entry points. It cannot contain an abort, a `SIGSEGV` or a
  `panic = "abort"` build - process isolation is the real containment here.

### Native capability permissions

`CapabilityHost` is a lazy, application-level permission gate in front of `PluginHost`.
`NATIVE_CAPABILITIES` contains exactly six capabilities:

| Capability | Deterministic mock reply |
| --- | --- |
| `camera.takePhoto` | fixed image path, width and height |
| `geolocation.getCurrentPosition` | fixed latitude, longitude and accuracy |
| `bluetooth.scan` | one fixed beacon and RSSI |
| `notification.show` | fixed delivery record |
| `payment.request` | fixed approved transaction |
| `share.send` | fixed share-sheet target |

`CapabilityHost::new(grants)` receives an explicit set; there is no implicit grant.
`register(dir)` reads and validates the manifest but does not spawn. `invoke` checks the catalog,
then the grant, then the registered declaration, and only then calls `PluginHost::load`. A denied
capability returns `PluginError::NotAllowed`; an undeclared capability returns
`PluginError::UnknownCapability`; neither executes the plugin. The inner host repeats the
allowlist check before writing a request.

```rust
use opurom_core::CapabilityHost;
use serde_json::json;

let host = CapabilityHost::new(["camera.takePhoto"]);
host.register("tests/fixtures/plugins/native")?;
let photo = host.invoke("camera.takePhoto", json!({}))?;
assert_eq!(photo["mock"], true);
host.shutdown();
# Ok::<(), opurom_core::PluginError>(())
```

The devices in `tests/fixtures/plugins/native/` are **deterministic mocks** speaking real JSON
lines over a real process boundary. There are no real OS permission prompts, no real camera or
Bluetooth hardware, no real payment provider and no real share sheet. This permission sandbox is
**not** an OS-level sandbox: no seatbelt, landlock, seccomp, filesystem or network namespace
restriction, or rlimits are applied.

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
shell script) used by the interactive UI tests; its behaviour is driven by a `schedule` file, so one
fixture covers handshake, success, unknown capability, timeout, crash and protocol violations.

## Status

| Area | Implementation |
| --- | --- |
| IR v0.1 loading | Version, state types, unique names/ids, action legality, malformed-JSON diagnostics; negative tests in `tests/integration.rs`. Node `kind` whitelist, theme values and prop semantics are not validated (fields are preserved); interpolation `{name}` is a renderer concern |
| Interactive UI IR extensions | Multi-action events with ordered, atomic patches; `valueFrom` reads at execution time with same-type and `int` validation; `when` conditions gate event dispatch on hidden nodes and their subtrees; empty action lists. Prop values, theme tokens and node kinds are preserved for the renderer and compiler |
| Reactive state | `int`/`bool`/`string`, `set`/`add`/`toggle`, patches, version counter, overflow and error atomicity |
| Priority pool and work-stealing scheduler | Bounded priority pool unchanged (3 tests); N-thread work stealing with a bounded queue, FIFO one-worker mode, `catch_unwind` typed panic errors, circuit breaker with injected-clock cooldown, panic isolation, graceful drain/join and measured 1-worker vs N-worker `Instant` timings (10 tests) |
| Async scheduler | Deterministic cooperative tick queue: priority with spawn-order tie-break, logical-time sleep/wake boundaries, cancel, drain, `TaskContext` children/continuations, determinism; 17 tests. No wall clock, threads or randomness |
| Plugin bus (in-process) | `catch_unwind` isolation, consecutive-failure circuit breaker, `PluginError::Disabled`, default threshold 3; 2 tests |
| Plugin host (process) | Manifest validation with traversal guard, spawn, `hello` handshake, capability-subset check, app allowlist, 500 ms timeout, protocol validation, legacy 3-strike `Disabled` breaker, configurable breaker with cooldown/half-open recovery, typed `CircuitOpen` errors, crash/malformed-frame isolation, `list()` and `shutdown()`; 18 existing tests plus 7 native-capability process tests. No OS-level sandboxing (no seatbelt/landlock/seccomp, no filesystem or network namespace restriction, no resource limits for the child), no plugin signing or signature verification, no trust chain and no version pinning. The local `opurom-plugins/registry.json` index and `opurom plugin verify` live in `opurom-plugins` |
| Native capability permissions | Exact six-capability catalog, explicit grants, lazy manifest registration, typed `NotAllowed`/`UnknownCapability` denial before spawn, deterministic mock replies, permission matrix and no-child-process checks in `tests/capabilities.rs` |
| Resource manager | Bounded LRU cache with a byte budget, deterministic eviction order, budget-exceeded error, zero-size handling; 6 tests |
| Runtime <-> plugin bus | `Runtime::invoke_plugin` caches plugin results under the runtime byte budget and reports plugin errors as values |
| Adapter degradation | Consecutive-error counting, threshold 3, fallback-only `run`, `degraded()`; 2 tests. The real `PlatformAdapter` trait, the actual adapter swap and frame presentation live in `opurom-platform`; this `Host` is a small standalone policy with no cross-repo dependency |
| CLI | IR file + stdin command loop, patch output, error diagnostics; 3 end-to-end binary tests; `plugins <dir> <capability>` subcommand; `parallel_speedup` example prints real measured scheduler timings. `opurom build/dev/test`, hot reload and watch mode live in `opurom-cli` |
| Renderer / pipeline | Nothing: RGBA output, layout, dirty rects, deterministic pixel hash, target emitters, GPU backend, LSP and debugger belong to other repositories |
| Quality evidence | 101 integration tests, 0 failed, 0 ignored; formatting, warning and documentation checks cover the Rust crate and published Markdown; `parallel_speedup` prints measured one-worker and N-worker timings. Pixel-consistency and golden-hash checks belong to `opurom-render`/`opurom-examples` |

Process isolation of plugins is real and verified. The permission gate and the deterministic mock
devices are application-level, and the cooperative scheduler is a deterministic queue only: it
never performs I/O. The work-stealing scheduler is real OS-threaded work stealing with a bounded
queue and panic isolation. Cross-repo end-to-end execution, real platform adapters, renderer
output, fps and competitive benchmarks are outside this repository; the only reported performance
numbers are the real 1-worker/N-worker `Instant` timings above.
