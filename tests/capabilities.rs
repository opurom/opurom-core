use opurom_core::{
    BreakerState, CapabilityHost, Clock, CounterClock, PluginError, PluginStatus,
    NATIVE_CAPABILITIES,
};
use serde_json::{json, Value as Json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const NATIVE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/plugins/native");

fn native_plugin(tag: &str, schedule: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("opurom-core-capabilities-{tag}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for file in ["plugin.json", "native.sh"] {
        fs::copy(Path::new(NATIVE_DIR).join(file), dir.join(file)).unwrap();
    }
    fs::write(dir.join("schedule"), format!("{schedule}\n")).unwrap();
    set_executable(&dir.join("native.sh"));
    dir
}

fn set_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

fn expected(capability: &str) -> Json {
    match capability {
        "camera.takePhoto" => {
            json!({"path":"/mock/camera/photo.png","width":640,"height":480,"mock":true})
        }
        "geolocation.getCurrentPosition" => {
            json!({"latitude":37.7749,"longitude":-122.4194,"accuracy":5,"mock":true})
        }
        "bluetooth.scan" => {
            json!({"devices":[{"id":"mock-beacon-1","rssi":-42}],"mock":true})
        }
        "notification.show" => json!({"delivered":true,"id":"mock-notification-1","mock":true}),
        "payment.request" => {
            json!({"approved":true,"transaction":"mock-payment-1","mock":true})
        }
        "share.send" => json!({"shared":true,"target":"mock-share-sheet","mock":true}),
        "network.request" => json!({"status":200,"body":{"ok":true},"mock":true}),
        other => panic!("unexpected capability {other}"),
    }
}

fn pid_is_alive(pid: &str) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .arg("-0")
            .arg(pid)
            .output()
            .is_ok_and(|output| output.status.success())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

#[test]
fn capability_catalog_and_manifest_declare_all_seven_mock_devices() {
    assert_eq!(
        NATIVE_CAPABILITIES,
        [
            "camera.takePhoto",
            "geolocation.getCurrentPosition",
            "bluetooth.scan",
            "notification.show",
            "payment.request",
            "share.send",
            "network.request",
        ]
    );
    let dir = native_plugin("catalog", "ok");
    let host = CapabilityHost::default().with_timeout(Duration::from_millis(300));
    let declared = host.register(&dir).unwrap();
    assert_eq!(
        declared,
        NATIVE_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_owned())
            .collect::<Vec<_>>()
    );
    host.shutdown();
    cleanup(&dir);
}

#[test]
fn every_capability_requires_an_explicit_grant_and_returns_a_mock_reply() {
    for capability in NATIVE_CAPABILITIES {
        let tag = capability.replace('.', "-");
        let dir = native_plugin(&tag, "ok");
        let host = CapabilityHost::new([capability]).with_timeout(Duration::from_millis(300));
        host.register(&dir).unwrap();
        let result = host.invoke(capability, json!({})).unwrap();
        assert_eq!(result, expected(capability), "{capability}");
        assert_eq!(host.list().unwrap()[0].status, PluginStatus::Active);
        host.shutdown();
        cleanup(&dir);
    }
}

#[test]
fn permission_matrix_denies_ungranted_and_undeclared_capabilities_without_spawning() {
    let dir = native_plugin("permissions", "ok");
    let host = CapabilityHost::new(["camera.takePhoto"]).with_timeout(Duration::from_millis(300));
    host.register(&dir).unwrap();

    for capability in NATIVE_CAPABILITIES.iter().skip(1) {
        match host.invoke(capability, json!({})) {
            Err(PluginError::NotAllowed(name)) => assert_eq!(name, *capability),
            other => panic!("expected NotAllowed for {capability}, got {other:?}"),
        }
    }
    match host.invoke("camera.nope", json!({})) {
        Err(PluginError::UnknownCapability(name)) => assert_eq!(name, "camera.nope"),
        other => panic!("expected UnknownCapability, got {other:?}"),
    }
    assert!(!dir.join("started").exists(), "a denial spawned the plugin");
    assert!(
        host.list().unwrap().is_empty(),
        "a denial loaded the plugin"
    );

    let result = host.invoke("camera.takePhoto", json!({})).unwrap();
    assert_eq!(result, expected("camera.takePhoto"));
    assert!(dir.join("started").exists());
    host.shutdown();
    cleanup(&dir);
}

#[test]
fn an_abrupt_plugin_exit_is_isolated_and_the_host_recovers() {
    let dir = native_plugin("crash", "crash,ok");
    let host = CapabilityHost::new(["camera.takePhoto"])
        .with_timeout(Duration::from_millis(300))
        .with_cooldown(1000);
    host.register(&dir).unwrap();
    assert!(matches!(
        host.invoke("camera.takePhoto", json!({})),
        Err(PluginError::Crashed(_))
    ));
    assert_eq!(host.breaker_state("native"), Some(BreakerState::HalfOpen));
    assert_eq!(
        host.invoke("camera.takePhoto", json!({})).unwrap(),
        expected("camera.takePhoto")
    );
    assert_eq!(host.breaker_state("native"), Some(BreakerState::Closed));
    assert_eq!(host.consecutive_failures("native"), 0);
    host.shutdown();
    cleanup(&dir);
}

#[test]
fn a_malformed_json_lines_frame_is_isolated_and_the_host_recovers() {
    let dir = native_plugin("garbage", "garbage,ok");
    let host = CapabilityHost::new(["camera.takePhoto"]).with_timeout(Duration::from_millis(300));
    host.register(&dir).unwrap();
    assert!(matches!(
        host.invoke("camera.takePhoto", json!({})),
        Err(PluginError::ProtocolViolation(_))
    ));
    assert_eq!(host.consecutive_failures("native"), 1);
    assert_eq!(
        host.invoke("camera.takePhoto", json!({})).unwrap(),
        expected("camera.takePhoto")
    );
    assert_eq!(host.consecutive_failures("native"), 0);
    host.shutdown();
    cleanup(&dir);
}

#[test]
fn breaker_states_trip_after_configured_faults_and_recover_after_cooldown() {
    let dir = native_plugin("breaker", "crash,crash,ok");
    let clock = CounterClock::at(0);
    let clock_trait: Arc<dyn Clock> = Arc::new(clock.clone());
    let host = CapabilityHost::new(["camera.takePhoto"])
        .with_timeout(Duration::from_millis(300))
        .with_failure_threshold(2)
        .with_cooldown(5)
        .with_clock(clock_trait);
    host.register(&dir).unwrap();

    for _ in 0..2 {
        assert!(matches!(
            host.invoke("camera.takePhoto", json!({})),
            Err(PluginError::Crashed(_))
        ));
    }
    assert_eq!(host.breaker_state("native"), Some(BreakerState::Open));
    assert!(host.circuit_open("native"));
    assert_eq!(host.consecutive_failures("native"), 2);
    assert_eq!(host.list().unwrap()[0].status, PluginStatus::CircuitOpen);
    assert_eq!(host.retry_after("native"), Some(5));

    match host.invoke("camera.takePhoto", json!({})) {
        Err(PluginError::CircuitOpen { retry_after }) => assert_eq!(retry_after, 5),
        other => panic!("expected CircuitOpen, got {other:?}"),
    }
    assert_eq!(clock.advance(4), 4);
    match host.invoke("camera.takePhoto", json!({})) {
        Err(PluginError::CircuitOpen { retry_after }) => assert_eq!(retry_after, 1),
        other => panic!("expected CircuitOpen before cooldown, got {other:?}"),
    }

    assert_eq!(clock.advance(1), 5);
    assert_eq!(
        host.invoke("camera.takePhoto", json!({})).unwrap(),
        expected("camera.takePhoto")
    );
    assert_eq!(host.breaker_state("native"), Some(BreakerState::Closed));
    assert!(!host.circuit_open("native"));
    assert_eq!(host.consecutive_failures("native"), 0);
    assert_eq!(host.list().unwrap()[0].status, PluginStatus::Active);
    host.shutdown();
    cleanup(&dir);
}

#[test]
fn no_child_plugin_processes_remain_after_faults_and_shutdown() {
    let dir = native_plugin("reap", "crash,garbage,ok");
    let host = CapabilityHost::new(["camera.takePhoto"])
        .with_timeout(Duration::from_millis(300))
        .with_failure_threshold(3);
    host.register(&dir).unwrap();
    assert!(matches!(
        host.invoke("camera.takePhoto", json!({})),
        Err(PluginError::Crashed(_))
    ));
    assert!(matches!(
        host.invoke("camera.takePhoto", json!({})),
        Err(PluginError::ProtocolViolation(_))
    ));
    assert_eq!(
        host.invoke("camera.takePhoto", json!({})).unwrap(),
        expected("camera.takePhoto")
    );
    host.shutdown();

    let pids = fs::read_to_string(dir.join("pids")).unwrap();
    let pids: Vec<&str> = pids.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(pids.len(), 3);
    for pid in pids {
        let mut alive = pid_is_alive(pid);
        for _ in 0..20 {
            if !alive {
                break;
            }
            thread::sleep(Duration::from_millis(10));
            alive = pid_is_alive(pid);
        }
        assert!(!alive, "plugin pid {pid} is still alive");
    }
    cleanup(&dir);
}
