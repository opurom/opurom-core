# opurom-core

Round 1 **vertical slice** of the OPUROM kernel. This is one independent Rust crate and one
independent local git repo; it does not import or link any other OPUROM project. It implements
the state/event part of [IR v0.1](../opurom-spec/IR_SCHEMA.md) plus the three fault-isolation
mechanisms from [ARCHITECTURE.md](../opurom-spec/ARCHITECTURE.md). It is **not** the whole
framework: no renderer, compiler, target emitters, platform adapters, GUI or build pipeline live
here.

## Build and verify

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cd /Users/mac/work/opurom/opurom-core
cargo test
```

Verified on Rust 1.99.1: **20 integration tests passed, 0 failed, 0 ignored; 0 doc tests;
zero compiler warnings in a fresh build**. Dependencies are `serde_json` and `serde_core`
(no derive, no network fetch needed beyond the crates.io index).

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
| `Runtime::from_ir_json(&str)` | Builds the state map and the node-id → event-name → action index. Malformed JSON and malformed IR return readable `CoreError`s; no `unwrap`/`panic` on input. |
| `Runtime::state(name)`, `snapshot()`, `version()` | `snapshot()` returns `Vec<Patch>` in state declaration order. |
| `Runtime::dispatch(Event)` | `Event::Tap { node_id }` / `Event::Input { node_id, text }`. Applies `set`/`add`/`toggle` atomically: compute first, write once, then bump `version`. `add` uses `checked_add`, so overflow is an error with no state change. Errors (unknown node, unbound event, bad type) leave state and version untouched. |
| `Patch`, `Value` | `Patch { name, value }`; `Value::Int(i64) \| Bool(bool) \| Str(String)`. Both serialize with `serde_json::to_string` as `{"name":"count","value":1}`. |
| `Scheduler::new(workers)`, `spawn(priority, task)`, `shutdown()` | Bounded std-thread pool, `BinaryHeap` priority (higher `u8` first, FIFO within a priority), `TaskHandle::join() -> Result<T, SchedulerError>`. `shutdown` stops new submissions, drains queued work, joins workers; `Drop` does the same. Task panics are caught, so a worker survives. |
| `PluginBus::new(failure_threshold)` (default 3), `register`, `call`, `is_disabled` | Handler signature `Fn(&str) -> Result<String, String> + Send + Sync`. Every call runs inside `catch_unwind(AssertUnwindSafe(..))`. Handler `Err` and panics increment a consecutive-failure counter; a success resets it; at the threshold the plugin is disabled and later calls return the distinct `PluginError::Disabled`. Other plugins and the runtime keep working. |
| `Host::new()` / `with_threshold(n)`, `run(primary, fallback)`, `record_error`, `record_success`, `degraded()` | Standalone degradation policy, no adapter crate dependency. Three consecutive primary `Err`s flip `degraded()` to true and `active_adapter()` to `"headless"`; once degraded, `run` calls only the fallback. A success resets the consecutive count. |

## CLI

`cargo run -- <app.opuir.json>` (or `target/debug/opurom-core <app.opuir.json>`) reads one IR file,
prints the initial state, then reads commands from stdin and prints the resulting patches:

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

Fixtures in `tests/fixtures/`: `counter.opuir.json` is a copy of the spec's golden IR,
`form.opuir.json` exercises string input, bool toggle and typed set, `bad-version.opuir.json`
is a negative case.

## Honest status

| Area | Implemented and verified here | Not implemented (planned / belongs to another repo) |
| --- | --- | --- |
| IR v0.1 loading | Version, state types, unique names/ids, action legality, malformed-JSON diagnostics; negative tests in `tests/integration.rs` | Node `kind` whitelist, theme values and prop semantics are not validated (fields are preserved); interpolation `{name}` is a renderer concern |
| Reactive state | `int`/`bool`/`string`, `set`/`add`/`toggle`, patches, version counter, overflow and error atomicity; 6 tests | Derived/computed state, multi-state transactions, undo, persistence, animation |
| Scheduler | Bounded worker pool, priority ordering, FIFO tie-break, graceful drain, panic containment; 3 tests | Work stealing, cancellation, timeouts, task metrics, real-time scheduling |
| Plugin bus | `catch_unwind` isolation, consecutive-failure circuit breaker, `PluginError::Disabled`, default threshold 3; 2 tests | Process/WASM sandboxing, capability checks, unload/reload, recovery after cooldown. `catch_unwind` cannot contain an abort, `SIGSEGV` or a `panic = "abort"` build |
| Adapter degradation | Consecutive-error counting, threshold 3, fallback-only `run`, `degraded()`; 2 tests | Real `PlatformAdapter` trait, actual adapter swap, recovery, frame presentation — those live in `opurom-platform`. This `Host` is a small standalone policy with no cross-repo dependency |
| CLI | IR file + stdin command loop, patch output, error diagnostics; 3 end-to-end binary tests | `opurom build/dev/test`, hot reload, watch mode (see `opurom-cli`) |
| Renderer / pipeline | Nothing | RGBA output, layout, dirty rects, deterministic pixel hash, target emitters, GPU backend, LSP, debugger |
| Quality evidence | `cargo test`: 20 passed, 0 failed, no warnings | No performance, fps, cold-start or binary-size numbers are reported: no GPU backend or device matrix exists yet. Pixel-consistency and golden-hash checks belong to `opurom-render`/`opurom-examples` |

**Gap summary:** a narrow, tested kernel slice, not a shipping SDK. Cross-repo end-to-end
execution, real platform adapters, renderer output and any benchmark figures are outside this
round. The plugin circuit breaker and the `Host` policy are local mechanisms; wiring them into a
real adapter ABI or a plugin host is future work.
