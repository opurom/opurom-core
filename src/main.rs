use opurom_core::{Event, FrameReport, FrameSample, Patch, PluginHost, Runtime, Value};
use serde_json::json;
use serde_json::Value as Json;
use std::io::{self, BufRead};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(first) = args.next() else {
        eprintln!(
            "usage: opurom-core <app.opuir.json> | opurom-core plugins <pluginDir> <capability>"
        );
        return ExitCode::from(2);
    };
    if first == "plugins" {
        return run_plugins(args);
    }
    run_ir(first, args)
}

/// `opurom-core plugins <pluginDir> <capability>` loads the plugin, handshakes,
/// invokes the capability with `{}` and prints the JSON result.
fn run_plugins(mut args: impl Iterator<Item = std::ffi::OsString>) -> ExitCode {
    let Some(dir) = args.next() else {
        eprintln!("usage: opurom-core plugins <pluginDir> <capability>");
        return ExitCode::from(2);
    };
    let Some(capability) = args.next() else {
        eprintln!("usage: opurom-core plugins <pluginDir> <capability>");
        return ExitCode::from(2);
    };
    let capability = capability.to_string_lossy().into_owned();
    // The CLI acts as the app: it allows exactly the capability it was asked for.
    let host = PluginHost::new([capability.clone()]);
    if let Err(error) = host.load(&dir) {
        eprintln!("error: cannot load plugin: {error}");
        return ExitCode::from(1);
    }
    match host.invoke(&capability, json!({})) {
        Ok(result) => match serde_json::to_string(&result) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("error: cannot encode plugin result: {error}");
                ExitCode::from(1)
            }
        },
        Err(error) => {
            eprintln!("error: plugin invocation failed: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_ir(path: std::ffi::OsString, _rest: impl Iterator<Item = std::ffi::OsString>) -> ExitCode {
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("error: cannot read {}: {error}", path.to_string_lossy());
            return ExitCode::from(1);
        }
    };
    let mut runtime = match Runtime::from_ir_json(&text) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    print_state(&runtime);

    for line in io::stdin().lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                println!("error: {error}");
                break;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // `frames` takes an optional argument, so it matches by prefix.
        if let Some(rest) = line.strip_prefix("frames") {
            print_frames(&runtime, rest.trim());
            continue;
        }
        // `record_frame` takes a JSON payload, so it matches by prefix too.
        if let Some(rest) = line.strip_prefix("record_frame") {
            feed_frame(&mut runtime, rest.trim());
            continue;
        }
        // `patch <json>` carries a host-supplied reserved patch such as
        // `{"name":"@locale","value":"fr"}`; the space keeps `patches` distinct.
        if let Some(rest) = line.strip_prefix("patch ") {
            apply_patch(&mut runtime, rest.trim());
            continue;
        }
        match line {
            "quit" => break,
            "state" => print_state(&runtime),
            "layers" => print_layers(&runtime),
            "network" => print_network(&runtime),
            "resource" => print_resource(&runtime),
            "pages" => print_pages(&runtime),
            "memory" => print_memory(&runtime),
            "patch" => apply_patch(&mut runtime, ""),
            _ => match parse_command(line) {
                Ok(event) => match runtime.dispatch(event) {
                    Ok(patches) => match serde_json::to_string(&patches) {
                        Ok(json) => println!("patches {json} version {}", runtime.version()),
                        Err(error) => println!("error: {error}"),
                    },
                    Err(error) => println!("error: {error}"),
                },
                Err(message) => println!("error: {message}"),
            },
        }
    }
    ExitCode::SUCCESS
}

fn print_state(runtime: &Runtime) {
    match serde_json::to_string(&runtime.snapshot()) {
        Ok(json) => println!("state {json} version {}", runtime.version()),
        Err(error) => println!("error: {error}"),
    }
}

fn print_resource(runtime: &Runtime) {
    let cache = runtime.resource_cache();
    let payload = serde_json::json!({
        "budget": cache.budget(),
        "usedBytes": cache.used_bytes(),
        "entries": cache.len(),
    });
    match serde_json::to_string(&payload) {
        Ok(json) => println!("resource {json} version {}", runtime.version()),
        Err(error) => println!("error: {error}"),
    }
}

fn print_layers(runtime: &Runtime) {
    match serde_json::to_string(&runtime.layer_records()) {
        Ok(json) => println!("layers {json} version {}", runtime.version()),
        Err(error) => println!("error: {error}"),
    }
}

fn print_pages(runtime: &Runtime) {
    match serde_json::to_string(&runtime.page_records()) {
        Ok(json) => println!("pages {json} version {}", runtime.version()),
        Err(error) => println!("error: {error}"),
    }
}

fn print_network(runtime: &Runtime) {
    match serde_json::to_string(&runtime.network_records()) {
        Ok(json) => println!("network {json} version {}", runtime.version()),
        Err(error) => println!("error: {error}"),
    }
}

/// `frames [sinceSeq]`: the inspector frames channel — every held record, or
/// the records past the given sequence number, plus the report and channel
/// availability.
fn print_frames(runtime: &Runtime, rest: &str) {
    let since_seq = if rest.is_empty() {
        None
    } else {
        match rest.parse::<u64>() {
            Ok(seq) => Some(seq),
            Err(error) => {
                println!("error: frames expects `frames [sinceSeq]`: {error}");
                return;
            }
        }
    };
    match serde_json::to_string(&runtime.frames_channel(since_seq)) {
        Ok(json) => println!("frames {json} version {}", runtime.version()),
        Err(error) => println!("error: {error}"),
    }
}

fn print_memory(runtime: &Runtime) {
    match serde_json::to_string(&runtime.memory_report()) {
        Ok(json) => println!("memory {json} version {}", runtime.version()),
        Err(error) => println!("error: {error}"),
    }
}

/// `record_frame <json>`: the host frame loop feeds one measured frame plus
/// its frame-budget machine snapshot, so the frames channel goes `full`.
/// Malformed payloads are typed errors on one line; the kernel keeps running.
fn feed_frame(runtime: &mut Runtime, rest: &str) {
    if rest.is_empty() {
        println!("error: record_frame expects `record_frame <json>`");
        return;
    }
    let payload: Json = match serde_json::from_str(rest) {
        Ok(payload) => payload,
        Err(error) => {
            println!("error: record_frame payload is not valid JSON: {error}");
            return;
        }
    };
    let sample = match parse_frame_sample(&payload) {
        Ok(sample) => sample,
        Err(message) => {
            println!("error: {message}");
            return;
        }
    };
    runtime.record_frame(sample);
    let seq = runtime
        .frame_records(None)
        .last()
        .map(|record| record.seq)
        .unwrap_or(0);
    println!("recorded seq {seq} version {}", runtime.version());
}

/// `patch <json>`: applies one host-supplied patch. Reserved names such as
/// `@locale` are validated by the kernel; bad values are one typed error line
/// and the REPL keeps reading.
fn apply_patch(runtime: &mut Runtime, rest: &str) {
    if rest.is_empty() {
        println!("error: patch expects `patch <json>`");
        return;
    }
    let patch = match parse_patch(rest) {
        Ok(patch) => patch,
        Err(message) => {
            println!("error: {message}");
            return;
        }
    };
    match runtime.apply_patch(&patch) {
        Ok(applied) => match serde_json::to_string(&applied) {
            Ok(json) => println!("patch {json} version {}", runtime.version()),
            Err(error) => println!("error: {error}"),
        },
        Err(error) => println!("error: {error}"),
    }
}

fn parse_patch(text: &str) -> Result<Patch, String> {
    let payload: Json = serde_json::from_str(text)
        .map_err(|error| format!("patch payload is not valid JSON: {error}"))?;
    let object = payload
        .as_object()
        .ok_or_else(|| "patch payload must be a JSON object".to_owned())?;
    let name = object
        .get("name")
        .and_then(Json::as_str)
        .ok_or_else(|| "patch.name must be a string".to_owned())?;
    let value = match object.get("value") {
        Some(Json::String(text)) => Value::Str(text.clone()),
        Some(Json::Number(number)) => number
            .as_i64()
            .map(Value::Int)
            .ok_or_else(|| "patch.value must be an integer".to_owned())?,
        Some(Json::Bool(flag)) => Value::Bool(*flag),
        _ => return Err("patch.value must be a string, integer or bool".to_owned()),
    };
    Ok(Patch {
        name: name.to_owned(),
        value,
    })
}

fn parse_frame_sample(payload: &Json) -> Result<FrameSample, String> {
    let object = payload
        .as_object()
        .ok_or_else(|| "record_frame payload must be a JSON object".to_owned())?;
    let int_field = |key: &str| -> Result<i64, String> {
        object
            .get(key)
            .and_then(Json::as_i64)
            .ok_or_else(|| format!("record_frame.{key} must be an integer"))
    };
    let nodes = u32::try_from(int_field("nodes")?)
        .map_err(|_| "record_frame.nodes must fit u32".to_owned())?;
    let effects = u32::try_from(int_field("effects")?)
        .map_err(|_| "record_frame.effects must fit u32".to_owned())?;
    let report = object
        .get("report")
        .and_then(Json::as_object)
        .ok_or_else(|| "record_frame.report must be an object".to_owned())?;
    let report_field = |key: &str| -> Result<i64, String> {
        report
            .get(key)
            .and_then(Json::as_i64)
            .ok_or_else(|| format!("record_frame.report.{key} must be an integer"))
    };
    let tier = report
        .get("tier")
        .and_then(Json::as_str)
        .ok_or_else(|| "record_frame.report.tier must be a string".to_owned())?;
    Ok(FrameSample {
        start_ms: int_field("startMs")?,
        frame_ms: int_field("frameMs")?,
        budget_ms: int_field("budgetMs")?,
        nodes,
        effects,
        report: FrameReport {
            p50_ms: report_field("p50Ms")?,
            p95_ms: report_field("p95Ms")?,
            fps_x10: report_field("fpsX10")?,
            tier: tier.to_owned(),
        },
    })
}

fn parse_command(line: &str) -> Result<Event, String> {
    let mut parts = line.splitn(2, ' ');
    let command = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match command {
        "tap" => {
            let node_id = rest
                .parse::<u32>()
                .map_err(|error| format!("tap expects `tap <id>`: {error}"))?;
            Ok(Event::Tap { node_id })
        }
        "doubleTap" | "longPress" | "drag" => {
            let node_id = rest
                .parse::<u32>()
                .map_err(|error| format!("{command} expects `{command} <id>`: {error}"))?;
            Ok(match command {
                "doubleTap" => Event::DoubleTap { node_id },
                "longPress" => Event::LongPress { node_id },
                _ => Event::Drag { node_id },
            })
        }
        "input" => {
            let mut parts = rest.splitn(2, ' ');
            let node_id = parts
                .next()
                .unwrap_or("")
                .parse::<u32>()
                .map_err(|error| format!("input expects `input <id> <text>`: {error}"))?;
            let text = parts
                .next()
                .ok_or_else(|| "input expects `input <id> <text>`".to_owned())?;
            Ok(Event::Input {
                node_id,
                text: text.to_owned(),
            })
        }
        _ => Err(format!(
            "unknown command {command:?}; expected tap, doubleTap, longPress, drag, input, state, layers, network, resource, pages, memory, frames, record_frame, patch or quit"
        )),
    }
}
