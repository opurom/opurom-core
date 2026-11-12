use crate::error::{panic_message, PluginError};
use serde_json::{json, Map, Value as Json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const DEFAULT_PLUGIN_TIMEOUT: Duration = Duration::from_millis(500);
pub const MIN_PLUGIN_TIMEOUT: Duration = Duration::from_millis(50);
/// Process startup is not part of the invoke latency budget, so the handshake
/// gets a wider window than a normal request.
const HANDSHAKE_FLOOR: Duration = Duration::from_secs(2);
const FAULT_THRESHOLD: u32 = 3;
const READER_QUEUE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginStatus {
    Active,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub capabilities: Vec<String>,
    pub status: PluginStatus,
    pub invocation_count: u64,
}

#[derive(Debug, Clone)]
struct Manifest {
    name: String,
    version: String,
    capabilities: Vec<String>,
    dir: PathBuf,
    entry: PathBuf,
}

enum ReadEvent {
    Line(String),
    Eof,
    Error(String),
}

struct Process {
    child: Child,
    stdin: Option<ChildStdin>,
    receiver: Receiver<ReadEvent>,
    _reader: JoinHandle<()>,
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stdin.take();
        terminate(&mut self.child);
    }
}

struct LoadedPlugin {
    manifest: Manifest,
    advertised: Vec<String>,
    status: PluginStatus,
    invocation_count: u64,
    next_id: u64,
    faults: u32,
    crashed: bool,
    process: Option<Process>,
}

impl LoadedPlugin {
    fn new(manifest: Manifest) -> Self {
        LoadedPlugin {
            manifest,
            advertised: Vec::new(),
            status: PluginStatus::Active,
            invocation_count: 0,
            next_id: 1,
            faults: 0,
            crashed: false,
            process: None,
        }
    }

    fn info(&self) -> PluginInfo {
        PluginInfo {
            name: self.manifest.name.clone(),
            version: self.manifest.version.clone(),
            capabilities: self.advertised.clone(),
            status: self.status,
            invocation_count: self.invocation_count,
        }
    }

    fn disable(&mut self) {
        self.status = PluginStatus::Disabled;
        self.process = None;
    }

    fn restart(&mut self, timeout: Duration) -> Result<(), PluginError> {
        // The old process is killed first: a hung plugin must not keep running.
        self.process = None;
        let mut process = spawn_process(&self.manifest)?;
        let hello = json!({"id": 0, "method": "hello", "protocol": "1.0"});
        let result =
            transact(&mut process, &hello, 0, handshake_timeout(timeout)).map_err(|error| {
                match error {
                    PluginError::Failed(message) => {
                        PluginError::ProtocolViolation(format!("hello failed: {message}"))
                    }
                    other => other,
                }
            })?;
        self.advertised = parse_hello(&result, &self.manifest.capabilities)?;
        self.process = Some(process);
        Ok(())
    }

    /// Brings a plugin back up after a fault; a failed restart counts as a fault too.
    fn ensure_running(&mut self, timeout: Duration) -> Result<(), PluginError> {
        if self.process.is_some() {
            return Ok(());
        }
        match self.restart(timeout) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.faults = self.faults.saturating_add(1);
                if self.faults >= FAULT_THRESHOLD {
                    self.disable();
                }
                Err(error)
            }
        }
    }

    fn invoke(
        &mut self,
        capability: &str,
        params: Json,
        timeout: Duration,
    ) -> Result<Json, PluginError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| PluginError::Failed("request id exhausted".to_owned()))?;
        self.invocation_count = self.invocation_count.saturating_add(1);
        self.ensure_running(timeout)?;
        let request = json!({
            "id": id,
            "method": "invoke",
            "capability": capability,
            "params": params,
        });
        let outcome = {
            let process = self
                .process
                .as_mut()
                .ok_or_else(|| PluginError::Disabled(self.manifest.name.clone()))?;
            transact(process, &request, id, timeout)
        };
        match outcome {
            Ok(result) => {
                // A success clears the fault counter only when no crash happened.
                if !self.crashed {
                    self.faults = 0;
                }
                Ok(result)
            }
            Err(
                error @ (PluginError::Timeout(_)
                | PluginError::ProtocolViolation(_)
                | PluginError::Crashed(_)),
            ) => {
                self.faults = self.faults.saturating_add(1);
                if matches!(error, PluginError::Crashed(_)) {
                    self.crashed = true;
                }
                // A faulty process is killed; the next call starts a fresh one.
                self.process = None;
                if self.faults >= FAULT_THRESHOLD {
                    self.disable();
                }
                Err(error)
            }
            Err(error) => Err(error),
        }
    }
}

pub struct PluginHost {
    allowlist: HashSet<String>,
    timeout: Duration,
    plugins: Mutex<HashMap<String, Arc<Mutex<LoadedPlugin>>>>,
}

impl PluginHost {
    pub fn new<I, S>(allowlist: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        PluginHost {
            allowlist: allowlist.into_iter().map(Into::into).collect(),
            timeout: DEFAULT_PLUGIN_TIMEOUT,
            plugins: Mutex::new(HashMap::new()),
        }
    }

    /// Timeouts are clamped to the protocol minimum of 50 ms.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = if timeout < MIN_PLUGIN_TIMEOUT {
            MIN_PLUGIN_TIMEOUT
        } else {
            timeout
        };
        self
    }

    pub fn load(&self, dir: impl AsRef<Path>) -> Result<PluginInfo, PluginError> {
        guard(|| {
            let manifest = parse_manifest(dir.as_ref())?;
            let mut loaded = LoadedPlugin::new(manifest.clone());
            loaded.restart(self.timeout)?;
            let info = loaded.info();
            let entry = Arc::new(Mutex::new(loaded));
            let mut plugins = lock(&self.plugins);
            if let Some(previous) = plugins.insert(manifest.name.clone(), entry) {
                let mut previous = lock(&previous);
                previous.disable();
            }
            Ok(info)
        })
    }

    /// Routes a capability to the single active plugin that advertised it.
    pub fn invoke(&self, capability: &str, params: Json) -> Result<Json, PluginError> {
        guard(|| self.invoke_route(capability, params))
    }

    /// Invokes a named plugin directly when capability names are intentionally shared.
    pub fn invoke_on(
        &self,
        plugin: &str,
        capability: &str,
        params: Json,
    ) -> Result<Json, PluginError> {
        guard(|| self.invoke_named(plugin, capability, params))
    }

    pub fn list(&self) -> Result<Vec<PluginInfo>, PluginError> {
        guard(|| {
            let entries = self.entries();
            let mut infos = Vec::with_capacity(entries.len());
            for (_, entry) in entries {
                infos.push(lock(&entry).info());
            }
            infos.sort_by(|left, right| left.name.cmp(&right.name));
            Ok(infos)
        })
    }

    fn entries(&self) -> Vec<(String, Arc<Mutex<LoadedPlugin>>)> {
        let mut entries: Vec<_> = lock(&self.plugins)
            .iter()
            .map(|(name, entry)| (name.clone(), Arc::clone(entry)))
            .collect();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }

    fn ensure_allowed(&self, capability: &str) -> Result<(), PluginError> {
        if self.allowlist.contains(capability) {
            Ok(())
        } else {
            Err(PluginError::NotAllowed(capability.to_owned()))
        }
    }

    fn invoke_route(&self, capability: &str, params: Json) -> Result<Json, PluginError> {
        self.ensure_allowed(capability)?;
        let mut active = Vec::new();
        let mut disabled = Vec::new();
        for (name, entry) in self.entries() {
            let (advertised, status) = {
                let plugin = lock(&entry);
                (plugin.advertised.clone(), plugin.status)
            };
            if advertised.iter().any(|item| item == capability) {
                if status == PluginStatus::Disabled {
                    disabled.push(name);
                } else {
                    active.push((name, entry));
                }
            }
        }
        match active.len() {
            1 => {
                let (name, entry) = active.remove(0);
                self.call_arc(&name, &entry, capability, params)
            }
            0 if !disabled.is_empty() => Err(PluginError::Disabled(disabled[0].clone())),
            0 => Err(PluginError::UnknownCapability(capability.to_owned())),
            _ => Err(PluginError::AmbiguousCapability(capability.to_owned())),
        }
    }
    fn invoke_named(
        &self,
        plugin: &str,
        capability: &str,
        params: Json,
    ) -> Result<Json, PluginError> {
        self.ensure_allowed(capability)?;
        let entry = lock(&self.plugins)
            .get(plugin)
            .cloned()
            .ok_or_else(|| PluginError::NotFound(plugin.to_owned()))?;
        self.call_arc(plugin, &entry, capability, params)
    }

    fn call_arc(
        &self,
        name: &str,
        entry: &Arc<Mutex<LoadedPlugin>>,
        capability: &str,
        params: Json,
    ) -> Result<Json, PluginError> {
        let mut plugin = lock(entry);
        if plugin.status == PluginStatus::Disabled {
            return Err(PluginError::Disabled(name.to_owned()));
        }
        if !plugin.advertised.iter().any(|item| item == capability) {
            return Err(PluginError::UnknownCapability(capability.to_owned()));
        }
        plugin.invoke(capability, params, self.timeout)
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        PluginHost::new(Vec::<String>::new())
    }
}

fn guard<T>(work: impl FnOnce() -> Result<T, PluginError>) -> Result<T, PluginError> {
    match catch_unwind(AssertUnwindSafe(work)) {
        Ok(result) => result,
        Err(panic) => Err(PluginError::Panicked(panic_message(panic))),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn handshake_timeout(timeout: Duration) -> Duration {
    if timeout > HANDSHAKE_FLOOR {
        timeout
    } else {
        HANDSHAKE_FLOOR
    }
}

fn parse_manifest(dir: &Path) -> Result<Manifest, PluginError> {
    let dir = fs::canonicalize(dir)
        .map_err(|error| PluginError::Io(format!("{}: {error}", dir.display())))?;
    if !dir.is_dir() {
        return Err(PluginError::InvalidManifest(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    let manifest_path = dir.join("plugin.json");
    let text = fs::read_to_string(&manifest_path)
        .map_err(|error| PluginError::Io(format!("{}: {error}", manifest_path.display())))?;
    let json: Json = serde_json::from_str(&text)
        .map_err(|error| PluginError::InvalidManifest(format!("plugin.json: {error}")))?;
    let object = json
        .as_object()
        .ok_or_else(|| PluginError::InvalidManifest("plugin.json must be an object".to_owned()))?;
    let name = manifest_string(object, "name")?;
    let version = manifest_string(object, "version")?;
    let protocol = manifest_string(object, "protocol")?;
    let entry = manifest_string(object, "entry")?;
    if !valid_name(&name) {
        return Err(PluginError::InvalidManifest(format!(
            "name {name:?} does not match [a-z][a-z0-9-]{{1,31}}"
        )));
    }
    if !valid_semver(&version) {
        return Err(PluginError::InvalidManifest(format!(
            "version {version:?} is not x.y.z"
        )));
    }
    if protocol != "1.0" {
        return Err(PluginError::InvalidManifest(format!(
            "protocol {protocol:?} is not 1.0"
        )));
    }
    let raw_capabilities = object
        .get("capabilities")
        .and_then(Json::as_array)
        .ok_or_else(|| PluginError::InvalidManifest("capabilities must be an array".to_owned()))?;
    if raw_capabilities.is_empty() {
        return Err(PluginError::InvalidManifest(
            "capabilities must not be empty".to_owned(),
        ));
    }
    let mut seen = HashSet::new();
    let mut capabilities = Vec::with_capacity(raw_capabilities.len());
    for item in raw_capabilities {
        let capability = item.as_str().ok_or_else(|| {
            PluginError::InvalidManifest("each capability must be a string".to_owned())
        })?;
        if !valid_capability(capability) {
            return Err(PluginError::InvalidManifest(format!(
                "capability {capability:?} is not a dotted name"
            )));
        }
        if !seen.insert(capability.to_owned()) {
            return Err(PluginError::InvalidManifest(format!(
                "duplicate capability {capability:?}"
            )));
        }
        capabilities.push(capability.to_owned());
    }
    let entry = validate_entry(&dir, &entry)?;
    Ok(Manifest {
        name,
        version,
        capabilities,
        dir,
        entry,
    })
}

fn manifest_string(object: &Map<String, Json>, key: &str) -> Result<String, PluginError> {
    object
        .get(key)
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or_else(|| PluginError::InvalidManifest(format!("{key} must be a string")))
}

fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2
        && bytes.len() <= 32
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn valid_semver(version: &str) -> bool {
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn valid_capability(capability: &str) -> bool {
    let parts: Vec<_> = capability.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
}

fn validate_entry(dir: &Path, entry: &str) -> Result<PathBuf, PluginError> {
    let path = Path::new(entry);
    if entry.is_empty()
        || entry.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::RootDir))
        || entry.split('/').any(|part| part == "..")
    {
        return Err(PluginError::InvalidManifest(format!(
            "entry {entry:?} must stay inside the plugin directory"
        )));
    }
    let candidate = dir.join(path);
    let resolved = candidate.canonicalize().map_err(|error| {
        PluginError::InvalidManifest(format!("cannot resolve entry {entry:?}: {error}"))
    })?;
    if !resolved.starts_with(dir) {
        return Err(PluginError::InvalidManifest(format!(
            "entry {entry:?} escapes the plugin directory"
        )));
    }
    let metadata = fs::metadata(&resolved).map_err(|error| {
        PluginError::InvalidManifest(format!("cannot inspect entry {entry:?}: {error}"))
    })?;
    if !is_executable(&metadata) {
        return Err(PluginError::InvalidManifest(format!(
            "entry {entry:?} is not an executable file"
        )));
    }
    Ok(resolved)
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
}

fn spawn_process(manifest: &Manifest) -> Result<Process, PluginError> {
    let mut child = Command::new(&manifest.entry)
        .current_dir(&manifest.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| PluginError::Spawn(format!("{}: {error}", manifest.entry.display())))?;
    let stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            terminate(&mut child);
            return Err(PluginError::Spawn("stdin pipe is missing".to_owned()));
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate(&mut child);
            return Err(PluginError::Spawn("stdout pipe is missing".to_owned()));
        }
    };
    let (sender, receiver) = sync_channel(READER_QUEUE);
    let reader = thread::Builder::new()
        .name(format!("plugin-{}-reader", manifest.name))
        .spawn(move || reader_loop(stdout, sender))
        .map_err(|error| PluginError::Spawn(format!("cannot start stdout reader: {error}")));
    let reader = match reader {
        Ok(reader) => reader,
        Err(error) => {
            drop(stdin);
            drop(receiver);
            terminate(&mut child);
            return Err(error);
        }
    };
    Ok(Process {
        child,
        stdin: Some(stdin),
        receiver,
        _reader: reader,
    })
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn reader_loop(stdout: ChildStdout, sender: SyncSender<ReadEvent>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                let _ = sender.send(ReadEvent::Eof);
                break;
            }
            Ok(_) => {
                while line.ends_with('\n') || line.ends_with('\r') {
                    line.pop();
                }
                let _ = sender.send(ReadEvent::Line(line));
            }
            Err(error) => {
                let _ = sender.send(ReadEvent::Error(error.to_string()));
                break;
            }
        }
    }
}

fn transact(
    process: &mut Process,
    request: &Json,
    expected_id: u64,
    timeout: Duration,
) -> Result<Json, PluginError> {
    write_request(process, request)?;
    read_response(process, expected_id, timeout)
}

fn write_request(process: &mut Process, request: &Json) -> Result<(), PluginError> {
    let line = serde_json::to_string(request).map_err(|error| {
        PluginError::ProtocolViolation(format!("cannot encode request: {error}"))
    })?;
    let stdin = process
        .stdin
        .as_mut()
        .ok_or_else(|| PluginError::Crashed("stdin is closed".to_owned()))?;
    writeln!(stdin, "{line}").map_err(classify_write_error)?;
    stdin.flush().map_err(classify_write_error)
}

fn classify_write_error(error: std::io::Error) -> PluginError {
    if error.kind() == std::io::ErrorKind::BrokenPipe {
        PluginError::Crashed(error.to_string())
    } else {
        PluginError::Io(error.to_string())
    }
}

fn read_response(
    process: &mut Process,
    expected_id: u64,
    timeout: Duration,
) -> Result<Json, PluginError> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match process.receiver.recv_timeout(remaining) {
            Ok(ReadEvent::Line(line)) => match parse_response(&line, expected_id) {
                Ok(result) => return Ok(result),
                Err(error @ PluginError::ProtocolViolation(_)) => {
                    // A stale answer to a request that already timed out is
                    // ignored; any other id mismatch is a real violation.
                    if response_id(&line).is_some_and(|id| id < expected_id) {
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => return Err(error),
            },
            Ok(ReadEvent::Eof) => {
                return Err(PluginError::Crashed(format!(
                    "stdout closed before response to id {expected_id}"
                )))
            }
            Ok(ReadEvent::Error(message)) => {
                return Err(PluginError::ProtocolViolation(format!(
                    "stdout read failed: {message}"
                )))
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err(PluginError::Timeout(format!(
                    "no response to id {expected_id} within {} ms",
                    timeout.as_millis()
                )))
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(PluginError::Crashed(format!(
                    "reader disconnected before response to id {expected_id}"
                )))
            }
        }
    }
}

/// The `id` of a response line, if it has one.
fn response_id(line: &str) -> Option<u64> {
    let json: Json = serde_json::from_str(line).ok()?;
    json.get("id").and_then(Json::as_u64)
}

fn parse_response(line: &str, expected_id: u64) -> Result<Json, PluginError> {
    let response: Json = serde_json::from_str(line).map_err(|error| {
        PluginError::ProtocolViolation(format!("malformed response JSON: {error}"))
    })?;
    let object = response.as_object().ok_or_else(|| {
        PluginError::ProtocolViolation("response must be a JSON object".to_owned())
    })?;
    let id = object.get("id").and_then(Json::as_u64).ok_or_else(|| {
        PluginError::ProtocolViolation("response.id must be an unsigned integer".to_owned())
    })?;
    if id != expected_id {
        return Err(PluginError::ProtocolViolation(format!(
            "response id {id} does not match request id {expected_id}"
        )));
    }
    let ok = object
        .get("ok")
        .and_then(Json::as_bool)
        .ok_or_else(|| PluginError::ProtocolViolation("response.ok must be a bool".to_owned()))?;
    if ok {
        object.get("result").cloned().ok_or_else(|| {
            PluginError::ProtocolViolation("successful response needs result".to_owned())
        })
    } else {
        let message = object.get("error").and_then(Json::as_str).ok_or_else(|| {
            PluginError::ProtocolViolation("failed response needs a string error".to_owned())
        })?;
        Err(PluginError::Failed(message.to_owned()))
    }
}

fn parse_hello(result: &Json, declared: &[String]) -> Result<Vec<String>, PluginError> {
    let object = result.as_object().ok_or_else(|| {
        PluginError::ProtocolViolation("hello result must be an object".to_owned())
    })?;
    let name = object.get("name").and_then(Json::as_str).ok_or_else(|| {
        PluginError::ProtocolViolation("hello result.name must be a string".to_owned())
    })?;
    let version = object
        .get("version")
        .and_then(Json::as_str)
        .ok_or_else(|| {
            PluginError::ProtocolViolation("hello result.version must be a string".to_owned())
        })?;
    if name.is_empty() || version.is_empty() {
        return Err(PluginError::ProtocolViolation(
            "hello result name and version must not be empty".to_owned(),
        ));
    }
    let raw = object
        .get("capabilities")
        .and_then(Json::as_array)
        .ok_or_else(|| {
            PluginError::ProtocolViolation("hello capabilities must be an array".to_owned())
        })?;
    let allowed: HashSet<&str> = declared.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let mut advertised = Vec::with_capacity(raw.len());
    for item in raw {
        let capability = item.as_str().ok_or_else(|| {
            PluginError::ProtocolViolation("hello capability must be a string".to_owned())
        })?;
        if !valid_capability(capability) {
            return Err(PluginError::ProtocolViolation(format!(
                "hello capability {capability:?} is not a dotted name"
            )));
        }
        if !allowed.contains(capability) {
            return Err(PluginError::ProtocolViolation(format!(
                "advertised capability {capability:?} was not declared"
            )));
        }
        if !seen.insert(capability.to_owned()) {
            return Err(PluginError::ProtocolViolation(format!(
                "duplicate hello capability {capability:?}"
            )));
        }
        advertised.push(capability.to_owned());
    }
    Ok(advertised)
}
