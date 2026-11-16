//! Inspector data plane (`INSPECTOR_DATA_PLANE.md`): the kernel collects and
//! holds the frame timeline, the memory report and the network records, all
//! on integers with no wall clock. Hosts inject the monotonic clock and the
//! frame-budget report; the kernel owns the rings and the query shapes.

use opurom_core::{
    CounterClock, Event, FrameReport, FrameSample, Runtime, FRAME_RING, MEMORY_HISTORY_RING,
    NETWORK_RING,
};
use serde_json::{json, Value as Json};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

fn ir_document() -> String {
    json!({
        "ir": "0.1.0", "app": "Inspector",
        "state": [{"name": "count", "type": "int", "init": 0}],
        "pages": ["Home"],
        "tree": {"id": 0, "kind": "Column", "props": {"opacity": 0.9}, "events": {}, "children": [
            {"id": 1, "kind": "Button", "props": {"label": "go"}, "children": [],
             "events": {"tap": {"op": "add", "target": "count", "value": 1}}}
        ]}
    })
    .to_string()
}

fn sample(seq_seed: i64, frame_ms: i64) -> FrameSample {
    FrameSample {
        start_ms: seq_seed * 16,
        frame_ms,
        budget_ms: 16,
        nodes: 42,
        effects: 3,
        report: FrameReport {
            p50_ms: 16,
            p95_ms: 17,
            fps_x10: 590,
            tier: "full".to_owned(),
        },
    }
}

fn record_frames(runtime: &mut Runtime, count: usize) {
    for i in 0..count {
        runtime.record_frame(sample(i as i64, 16 + (i as i64 % 3)));
    }
}

#[test]
fn the_frames_channel_is_absent_before_any_frame() {
    let runtime = Runtime::from_ir_json(&ir_document()).unwrap();
    let channel = runtime.frames_channel(None);
    assert_eq!(channel["frames"], json!([]));
    assert_eq!(channel["report"], Json::Null);
    assert_eq!(channel["availability"], "absent");
    assert_eq!(channel["reason"], "no frame records yet");
}

#[test]
fn record_frame_assigns_seq_and_answers_the_frames_channel() {
    let mut runtime = Runtime::from_ir_json(&ir_document()).unwrap();
    record_frames(&mut runtime, 2);

    let channel = runtime.frames_channel(None);
    assert_eq!(channel["availability"], "full");
    let frames = channel["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 2);
    // The record shape is exactly the spec shape, phaseMs null in v1.
    assert_eq!(
        frames[0],
        json!({"seq": 0, "startMs": 0, "frameMs": 16, "budgetMs": 16,
               "phaseMs": null, "tier": "full", "nodes": 42, "effects": 3})
    );
    assert_eq!(frames[1]["seq"], 1);
    assert_eq!(frames[1]["startMs"], 16);
    assert_eq!(
        channel["report"],
        json!({"p50Ms": 16, "p95Ms": 17, "fpsX10": 590, "tier": "full"})
    );

    // No argument answers every held record; a sinceSeq filter is strictly
    // greater, so the relay pages forward without gaps or repeats.
    assert_eq!(runtime.frame_records(None).len(), 2);
    assert_eq!(runtime.frame_records(Some(0)).len(), 1);
    let filtered = runtime.frame_records(Some(0));
    assert_eq!(filtered[0].seq, 1);
    assert_eq!(runtime.frame_records(Some(5)).len(), 0);
    let since_channel = runtime.frames_channel(Some(0));
    assert_eq!(since_channel["frames"].as_array().unwrap().len(), 1);

    // The held report is the host machine's latest snapshot.
    let report = runtime.frame_report().unwrap();
    assert_eq!(report.p50_ms, 16);
    assert_eq!(report.tier, "full");
}

#[test]
fn the_frame_ring_keeps_the_last_records_with_stable_seqs() {
    let mut runtime = Runtime::from_ir_json(&ir_document()).unwrap();
    record_frames(&mut runtime, FRAME_RING + 5);

    let frames = runtime.frame_records(None);
    assert_eq!(frames.len(), FRAME_RING);
    // Seqs never restart, so `sinceSeq` stays meaningful across eviction.
    assert_eq!(frames[0].seq, 5);
    assert_eq!(frames.last().unwrap().seq, (FRAME_RING + 4) as u64);
    assert_eq!(runtime.frame_records(Some(200)).len(), 4);
}

#[test]
fn the_memory_report_mirrors_the_ledger_and_the_json_sizes() {
    let mut runtime = Runtime::from_ir_json_with_budget(&ir_document(), 4096).unwrap();
    runtime.dispatch(Event::Tap { node_id: 1 }).unwrap();

    let report = runtime.memory_report();
    assert_eq!(
        report["cache"],
        json!({"budget": 4096, "usedBytes": 0, "entries": 0})
    );
    // The sizes are the JSON serializations the kernel itself serves.
    let state_bytes = serde_json::to_string(&runtime.snapshot()).unwrap().len();
    let layers_bytes = serde_json::to_string(&runtime.layer_records())
        .unwrap()
        .len();
    assert_eq!(report["stateBytes"], state_bytes);
    assert_eq!(report["layersBytes"], layers_bytes);
    assert_eq!(report["history"].as_array().unwrap().len(), 1);
    assert_eq!(report["history"][0]["version"], 1);
    assert_eq!(
        report["history"][0]["totalBytes"],
        state_bytes + layers_bytes
    );
    // No raster host: absent data is null, never a fabricated 0.
    assert_eq!(report["images"], Json::Null);
    assert_eq!(report["availability"], "full");

    // A raster host injects its image cache stats; the shape stays additive.
    let with_images = Runtime::from_ir_json(&ir_document())
        .unwrap()
        .with_image_stats(3, 1536);
    assert_eq!(
        with_images.memory_report()["images"],
        json!({"entries": 3, "bytes": 1536})
    );
}

#[test]
fn the_memory_history_appends_per_dispatch_and_caps_at_50() {
    let mut runtime = Runtime::from_ir_json(&ir_document()).unwrap();
    for _ in 0..(MEMORY_HISTORY_RING + 5) {
        runtime.dispatch(Event::Tap { node_id: 1 }).unwrap();
    }
    let history = runtime.memory_report()["history"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(history.len(), MEMORY_HISTORY_RING);
    // Versions never restart, so the ring keeps the most recent entries.
    assert_eq!(history[0]["version"], 6);
    assert_eq!(
        history.last().unwrap()["version"],
        (MEMORY_HISTORY_RING + 5) as u64
    );
}

#[test]
fn network_records_gain_additive_timing_fields() {
    let mut runtime = Runtime::from_ir_json(&ir_document()).unwrap();
    runtime
        .invoke_plugin(
            "network.request",
            json!({"method": "GET", "url": "https://api.example.com/items"}),
            |_, _| Ok(json!({"status": 200, "body": {}})),
        )
        .unwrap();
    runtime
        .invoke_plugin(
            "network.request",
            json!({"method": "POST", "url": "https://api.example.com/items"}),
            |_, _| Ok(json!({"status": 201, "body": {}})),
        )
        .unwrap();

    let records = runtime.network_records();
    let arr = records.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    // The legacy fields are unchanged; the timing fields are additive.
    assert_eq!(arr[0]["method"], "GET");
    assert_eq!(arr[0]["url"], "https://api.example.com/items");
    assert_eq!(arr[0]["status"], 200);
    assert!(arr[0]["requestBytes"].as_u64().unwrap() > 0);
    // The default logical clock is deterministic: one tick per read.
    assert_eq!(arr[0]["id"], 0);
    assert_eq!(arr[0]["startMs"], 0);
    assert_eq!(arr[0]["durationMs"], 1);
    assert_eq!(arr[1]["id"], 1);
    assert_eq!(arr[1]["startMs"], 2);
    assert_eq!(arr[1]["durationMs"], 1);
}

#[test]
fn the_injected_monotonic_clock_drives_the_timing_fields() {
    let mut runtime = Runtime::from_ir_json(&ir_document())
        .unwrap()
        .with_clock(Arc::new(CounterClock::at(100)));
    runtime
        .invoke_plugin(
            "network.request",
            json!({"method": "GET", "url": "https://api.example.com/a"}),
            |_, _| Ok(json!({"status": 200, "body": {}})),
        )
        .unwrap();
    // A stalled injected clock reads the same instant twice: duration 0.
    let arr = runtime.network_records().as_array().unwrap().clone();
    assert_eq!(arr[0]["startMs"], 100);
    assert_eq!(arr[0]["durationMs"], 0);

    // The host advances its clock between requests; the record follows.
    let clock = CounterClock::at(100);
    let mut runtime = Runtime::from_ir_json(&ir_document())
        .unwrap()
        .with_clock(Arc::new(clock.clone()));
    runtime
        .invoke_plugin(
            "network.request",
            json!({"method": "GET", "url": "https://api.example.com/a"}),
            |_, _| Ok(json!({"status": 200, "body": {}})),
        )
        .unwrap();
    clock.advance(7);
    runtime
        .invoke_plugin(
            "network.request",
            json!({"method": "GET", "url": "https://api.example.com/b"}),
            |_, _| Ok(json!({"status": 200, "body": {}})),
        )
        .unwrap();
    let arr = runtime.network_records().as_array().unwrap().clone();
    assert_eq!(arr[0]["startMs"], 100);
    assert_eq!(arr[1]["startMs"], 107);
    assert_eq!(arr[1]["durationMs"], 0);
}

#[test]
fn the_network_ring_keeps_the_last_200_records() {
    let mut runtime = Runtime::from_ir_json(&ir_document()).unwrap();
    for i in 0..NETWORK_RING + 5 {
        runtime
            .invoke_plugin(
                "network.request",
                json!({"method": "GET", "url": format!("https://api.example.com/{i}")}),
                |_, _| Ok(json!({"status": 200, "body": {}})),
            )
            .unwrap();
    }
    let arr = runtime.network_records().as_array().unwrap().clone();
    assert_eq!(arr.len(), NETWORK_RING);
    // Ids never restart, so the relay can see the eviction gap.
    assert_eq!(arr[0]["id"], 5);
    assert_eq!(arr.last().unwrap()["id"], (NETWORK_RING + 4) as u64);
}

#[test]
fn a_failed_capability_call_records_nothing() {
    let mut runtime = Runtime::from_ir_json(&ir_document()).unwrap();
    let error = runtime
        .invoke_plugin(
            "network.request",
            json!({"method": "GET", "url": "x"}),
            |_, _| Err(opurom_core::PluginError::Failed("down".to_owned())),
        )
        .unwrap_err();
    assert!(
        matches!(error, opurom_core::CoreError::Plugin(_)),
        "{error:?}"
    );
    assert_eq!(runtime.network_records(), json!([]));
}

#[test]
fn the_same_event_and_frame_sequence_is_byte_identical() {
    let mut a = Runtime::from_ir_json(&ir_document()).unwrap();
    let mut b = Runtime::from_ir_json(&ir_document()).unwrap();
    for runtime in [&mut a, &mut b] {
        record_frames(runtime, 3);
        runtime.dispatch(Event::Tap { node_id: 1 }).unwrap();
        runtime
            .invoke_plugin(
                "network.request",
                json!({"method": "GET", "url": "https://api.example.com/items"}),
                |_, _| Ok(json!({"status": 200, "body": {}})),
            )
            .unwrap();
    }
    assert_eq!(
        a.frames_channel(None).to_string(),
        b.frames_channel(None).to_string()
    );
    assert_eq!(a.memory_report().to_string(), b.memory_report().to_string());
    assert_eq!(
        a.network_records().to_string(),
        b.network_records().to_string()
    );
}

#[test]
fn the_kernel_render_path_records_one_frame_per_call() {
    let mut runtime = Runtime::from_ir_json(&ir_document()).unwrap();
    for _ in 0..3 {
        let _ = runtime.display_list();
    }

    let frames = runtime.frame_records(None);
    assert_eq!(frames.len(), 3);
    for (position, frame) in frames.iter().enumerate() {
        assert_eq!(frame.seq, position as u64);
        // The logical clock ticks once per read: start and end of the same
        // measured call are one tick apart, deterministically.
        assert_eq!(frame.start_ms, (position * 2) as i64);
        assert_eq!(frame.frame_ms, 1);
        assert_eq!(frame.budget_ms, 16);
        assert!(frame.nodes >= 1, "the rendered tree has nodes");
        assert!(frame.effects >= 1, "the fixture tree declares effects");
    }

    // The per-frame units agree with the layer records the kernel serves.
    let records_json = runtime.layer_records();
    let records = records_json.as_array().unwrap();
    let effects: usize = records
        .iter()
        .map(|record| record["effects"].as_array().map(Vec::len).unwrap_or(0))
        .sum();
    assert_eq!(frames[0].nodes as usize, records.len());
    assert_eq!(frames[0].effects as usize, effects);

    // Kernel-measured frames carry no tier: the channel is partial, not a
    // fabricated full report.
    let channel = runtime.frames_channel(None);
    assert_eq!(channel["availability"], "partial");
    assert_eq!(channel["reason"], "no frame-budget report from the host");
    assert_eq!(channel["report"], Json::Null);
    assert_eq!(channel["frames"].as_array().unwrap().len(), 3);
}

#[test]
fn recording_frames_never_changes_the_rendered_bytes() {
    let mut plain = Runtime::from_ir_json(&ir_document()).unwrap();
    let mut noisy = Runtime::from_ir_json(&ir_document()).unwrap();
    // The noisy runtime renders and dispatches first: rings fill up, yet the
    // display list is a pure function of the IR and the hook.
    noisy.display_list();
    noisy.dispatch(Event::Tap { node_id: 1 }).unwrap();
    noisy.memory_report();
    assert_eq!(plain.display_list(), noisy.display_list());
}

#[test]
fn inspector_reads_do_not_record_frames() {
    let mut runtime = Runtime::from_ir_json(&ir_document()).unwrap();
    runtime.display_list();
    let recorded = runtime.frame_records(None).len();

    // The memory report and layer records are reads, not render steps.
    let _ = runtime.memory_report();
    let _ = runtime.layer_records();
    let _ = runtime.memory_report()["layersBytes"];
    assert_eq!(runtime.frame_records(None).len(), recorded);
}

fn run_cli(input: &str) -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/counter.opuir.json"
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_opurom-core"))
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("cli starts");
    child
        .stdin
        .as_mut()
        .expect("stdin pipe")
        .write_all(input.as_bytes())
        .expect("stdin write");
    let output = child.wait_with_output().expect("cli runs");
    String::from_utf8(output.stdout).expect("stdout utf-8")
}

#[test]
fn the_cli_answers_memory_and_frames_commands() {
    let out = run_cli("memory\nframes\nframes 5\nframes x\nquit\n");
    let lines: Vec<&str> = out.lines().collect();

    // `memory` carries the full channel shape.
    let memory = lines
        .iter()
        .find(|line| line.starts_with("memory {"))
        .expect("memory line");
    let (memory_json, _) = memory["memory ".len()..]
        .rsplit_once(" version")
        .expect("version suffix");
    let payload: Json = serde_json::from_str(memory_json).expect("memory json");
    assert!(payload["stateBytes"].as_u64().unwrap() > 0);
    assert!(payload["cache"]["budget"].as_u64().unwrap() > 0);
    assert_eq!(payload["images"], Json::Null);
    assert_eq!(payload["availability"], "full");

    // `frames` starts absent: the CLI runs no frame loop, so nothing is
    // fabricated.
    let frames = lines
        .iter()
        .find(|line| line.starts_with("frames {"))
        .expect("frames line");
    let (frames_json, _) = frames["frames ".len()..]
        .rsplit_once(" version")
        .expect("version suffix");
    let payload: Json = serde_json::from_str(frames_json).expect("frames json");
    assert_eq!(payload["availability"], "absent");
    assert_eq!(payload["frames"], json!([]));

    // A sinceSeq filter and a bad argument are both answered.
    assert!(
        lines.iter().any(
            |line| line.starts_with("frames {") && line.contains("\"availability\":\"absent\"")
        ),
        "{out}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("error:") && line.contains("frames expects")),
        "{out}"
    );
}
