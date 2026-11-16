//! Plugin dependency graph (`PLUGIN_PROTOCOL.md`): the kernel validates the
//! whole batch before spawning — dependencies must exist in the registry,
//! cycles are rejected with the cycle path, plugins load in dependency order,
//! and a plugin whose dependency failed is not spawned. The `chain` fixture is
//! a renamed copy of the stub plugin that writes a `started` marker, so a test
//! can prove whether a plugin ever spawned.

use opurom_core::{PluginError, PluginHost};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/plugins/chain")
        .parent()
        .unwrap()
        .to_path_buf()
}

/// A fresh copy of the chain fixture on a scratch root of `N` plugins named
/// `p0`, `p1`, ... with the given dependency edges. Returned paths are in the
/// order the caller asked for.
fn graph(specs: &[(&str, &[&str], &str)]) -> (PathBuf, Vec<PathBuf>) {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let scratch = std::env::temp_dir().join(format!(
        "opurom-deps-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&scratch).unwrap();
    let source = fixtures().join("chain");
    let mut dirs = Vec::new();
    for (name, deps, _caps) in specs {
        let dir = scratch.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::copy(source.join("stub.sh"), dir.join("stub.sh"))
            .unwrap_or_else(|error| panic!("copy stub.sh into {}: {error}", dir.display()));
        fs::write(
            dir.join("plugin.json"),
            json!({
                "name": name,
                "version": "0.1.0",
                "protocol": "1.0",
                "capabilities": [format!("{name}.do")],
                "dependencies": deps,
                "entry": "./stub.sh"
            })
            .to_string(),
        )
        .unwrap();
        // The stub reads `caps` from its directory; align the advertised set
        // with this plugin's declared capability so hello validates.
        fs::write(dir.join("caps"), format!("{name}.do")).unwrap();
        dirs.push(dir);
    }
    (scratch, dirs)
}

fn cleanup(scratch: &Path) {
    // Another test may have swept the shared temp root; ignore.
    let _ = fs::remove_dir_all(scratch);
}

fn started(dir: &Path) -> bool {
    dir.join("started").exists()
}

#[test]
fn chained_dependencies_load_in_dependency_order() {
    // c depends on b, which depends on a: the batch is given in reverse.
    let (scratch, dirs) = graph(&[
        ("pc", &["pb"], "pc.do"),
        ("pa", &[], "pa.do"),
        ("pb", &["pa"], "pb.do"),
    ]);
    let host = PluginHost::new(["pc.do", "pa.do", "pb.do"]);
    // Batch order: pc, pa, pb. Loaded order must be pa, pb, pc.
    let infos = host.load_all(&dirs).unwrap();
    let order: Vec<&str> = infos.iter().map(|info| info.name.as_str()).collect();
    assert_eq!(order, ["pa", "pb", "pc"]);

    // Transitive deps are present and routable.
    assert_eq!(
        host.invoke("pc.do", json!({})).unwrap()["capability"],
        "pc.do"
    );
    assert!(started(&dirs[0]) && started(&dirs[1]) && started(&dirs[2]));

    let listed: BTreeSet<String> = host
        .list()
        .unwrap()
        .into_iter()
        .map(|info| info.name)
        .collect();
    assert_eq!(listed, ["pa", "pb", "pc"].map(String::from).into());
    host.shutdown();
    cleanup(&scratch);
}

#[test]
fn a_dependency_already_loaded_satisfies_the_graph() {
    let (scratch, dirs) = graph(&[("pa", &[], "pa.do"), ("pb", &["pa"], "pb.do")]);
    let host = PluginHost::new(["pa.do", "pb.do"]);
    host.load(&dirs[0]).unwrap();
    // Loading the dependent alone only works because pa is in the registry.
    host.load(&dirs[1]).unwrap();
    assert_eq!(
        host.invoke("pb.do", json!({})).unwrap()["capability"],
        "pb.do"
    );
    host.shutdown();
    cleanup(&scratch);
}

#[test]
fn a_cycle_is_rejected_with_the_path_in_the_diagnostic() {
    let (scratch, dirs) = graph(&[
        ("pa", &["pc"], "pa.do"),
        ("pb", &["pa"], "pb.do"),
        ("pc", &["pb"], "pc.do"),
    ]);
    let host = PluginHost::new(["pa.do", "pb.do", "pc.do"]);
    let error = host.load_all(&dirs).unwrap_err();
    let message = error.to_string();
    assert!(
        matches!(error, PluginError::InvalidManifest(_)),
        "{error:?}"
    );
    assert!(message.contains("dependency cycle"), "{message}");
    assert!(message.contains(" -> "), "cycle path is shown: {message}");
    for name in ["pa", "pb", "pc"] {
        assert!(message.contains(name), "cycle path names {name}: {message}");
    }
    // Nothing spawned: the whole batch was rejected before any process.
    for dir in &dirs {
        assert!(!started(dir), "no plugin in a cyclic batch may spawn");
    }
    host.shutdown();
    cleanup(&scratch);
}

#[test]
fn a_self_dependency_is_rejected_at_manifest_parse() {
    let (scratch, dirs) = graph(&[("pa", &["pa"], "pa.do")]);
    let host = PluginHost::new(["pa.do"]);
    let error = host.load_all(&dirs).unwrap_err();
    assert!(
        matches!(&error, PluginError::InvalidManifest(message) if message.contains("cannot depend on itself")),
        "{error:?}"
    );
    assert!(!started(&dirs[0]));
    cleanup(&scratch);
}

#[test]
fn a_missing_dependency_is_rejected_before_spawn() {
    // pb depends on pa, which is not in the batch and not loaded.
    let (scratch, dirs) = graph(&[("pb", &["pa"], "pb.do")]);
    let host = PluginHost::new(["pb.do"]);
    let error = host.load_all(&dirs).unwrap_err();
    assert!(
        matches!(&error, PluginError::InvalidManifest(message)
            if message.contains("not in the registry") && message.contains("pa")),
        "{error:?}"
    );
    assert!(!started(&dirs[0]), "the dependent must not spawn");

    // The single-plugin `load` path cannot batch-verify, so the same dangling
    // dependency surfaces as the spawn-gate error instead.
    let error = host.load(&dirs[0]).unwrap_err();
    assert!(
        matches!(&error, PluginError::DependencyFailed { plugin, dependency }
            if plugin == "pb" && dependency == "pa"),
        "{error:?}"
    );
    assert!(!started(&dirs[0]));
    cleanup(&scratch);
}

#[test]
fn a_dependency_that_fails_to_load_is_not_spawned_nor_are_its_dependents() {
    let (scratch, dirs) = graph(&[("pa", &[], "pa.do"), ("pb", &["pa"], "pb.do")]);
    // pa exits before its hello reply, so its load fails (handshake).
    fs::write(dirs[0].join("crash_hello"), "").unwrap();
    let host = PluginHost::new(["pa.do", "pb.do"]);
    let infos = host.load_all(&dirs).unwrap();
    // pa failed to spawn, so pb (which depends on it) is skipped too.
    assert!(
        infos.is_empty(),
        "neither plugin reports as loaded: {infos:?}"
    );
    assert!(!started(&dirs[0]), "pa never completed its handshake");
    assert!(!started(&dirs[1]), "pb must not spawn without pa");
    host.shutdown();
    cleanup(&scratch);
}

#[test]
fn a_batch_with_a_duplicate_name_is_rejected() {
    let (scratch, dirs) = graph(&[("pa", &[], "pa.do")]);
    let host = PluginHost::new(["pa.do"]);
    let error = host
        .load_all([dirs[0].clone(), dirs[0].clone()])
        .unwrap_err();
    assert!(
        matches!(&error, PluginError::InvalidManifest(message) if message.contains("listed twice")),
        "{error:?}"
    );
    cleanup(&scratch);
}

#[test]
fn a_malformed_dependencies_field_is_rejected() {
    let (scratch, dirs) = graph(&[("pa", &[], "pa.do")]);
    fs::write(
        dirs[0].join("plugin.json"),
        json!({
            "name": "pa", "version": "0.1.0", "protocol": "1.0",
            "capabilities": ["pa.do"],
            "dependencies": "not-an-array",
            "entry": "./stub.sh"
        })
        .to_string(),
    )
    .unwrap();
    let host = PluginHost::new(["pa.do"]);
    let error = host.load(&dirs[0]).unwrap_err();
    assert!(
        matches!(&error, PluginError::InvalidManifest(message) if message.contains("dependencies must be an array")),
        "{error:?}"
    );
    cleanup(&scratch);
}

#[test]
fn an_empty_dependencies_list_is_a_plain_plugin() {
    let (scratch, dirs) = graph(&[("pa", &[], "pa.do")]);
    let host = PluginHost::new(["pa.do"]);
    host.load(&dirs[0]).unwrap();
    assert_eq!(
        host.invoke("pa.do", json!({})).unwrap()["capability"],
        "pa.do"
    );
    host.shutdown();
    cleanup(&scratch);
    // A fresh graph on a different scratch root is untouched by any earlier
    // test's mutation.
    let (scratch, dirs) = graph(&[("plain", &[], "plain.do")]);
    let host = PluginHost::new(["plain.do"]);
    host.load(&dirs[0]).unwrap();
    assert_eq!(
        host.invoke("plain.do", json!({})).unwrap()["capability"],
        "plain.do"
    );
    host.shutdown();
    cleanup(&scratch);
}
