//! The service runtime: one thread owns the engine; listeners, the API and the operator talk to it
//! through a bounded message queue (spec §3.6, §11).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tpt_app_av_automation_actions::{
    Actions, ExecPolicy, Incident, IncidentSink, ThreadSleeper, TracingSink,
};
use tpt_app_av_automation_core::{
    slugify_rule_id, ActionId, Clock, ConditionId, DeviceHealth, Error, Event, Result, RuleId,
    Timestamp,
};
use tpt_app_av_automation_devices::dmx_serial::{Dmx512Assembler, DmxSerialReader};
use tpt_app_av_automation_devices::{build_endpoint, DeviceFile, DeviceRegistry, Endpoint};
use tpt_app_av_automation_engine::{CancelHandle, Engine, EngineConfig, Mode};
use tpt_app_av_automation_model::{has_errors, ExecutionRecord, Rule, RulePack};
use tpt_app_av_automation_report::machine_result;
use tpt_app_av_automation_triggers::{Inbound, InboundLimits, LocalClock};

use crate::config::{ListenerConfig, ServiceConfig};
use crate::store::Store;

const RECENT_LIMIT: usize = 200;
const QUEUE_DEPTH: usize = 8192;

/// A request to the engine thread that needs an answer.
///
/// The pack-editing variants (`ValidatePack`, `LoadPack`, `UpsertRule`, `RemoveRule`) report
/// authoring mistakes as a structured `Ok` answer (`valid: false` plus diagnostics) rather than an
/// `Err`: a rejected pack is a normal, expected outcome of the editor, not a service failure.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlRequest {
    /// Arm a rule.
    Arm(String),
    /// Disarm a rule.
    Disarm(String),
    /// Run a rule now.
    Run(String),
    /// The current pack as YAML.
    PackYaml,
    /// The current pack as JSON — what the builder canvas renders (spec §12.2).
    PackJson,
    /// Parse and validate a candidate pack without applying it.
    ValidatePack(String),
    /// Parse, validate and hot-load a pack (rules unchanged in id *and* version stay armed).
    LoadPack(String),
    /// Insert or replace one rule (matched by id) in the current pack.
    UpsertRule(Value),
    /// Remove one rule from the current pack.
    RemoveRule(String),
    /// Switch the whole engine between live and simulation.
    Simulation(bool),
    /// Probe one device's endpoint now (device manager "test" button).
    PingDevice(String),
}

/// The answer to a [`ControlRequest`].
pub type ControlReply = std::result::Result<Value, String>;

/// Turns a pack-parse failure into the structured "rejected pack" answer the editor renders.
fn validation_failure(e: Error) -> Value {
    match e {
        Error::Validation(diagnostics) => json!({
            "valid": false,
            "diagnostics": diagnostics,
        }),
        other => json!({
            "valid": false,
            "diagnostics": [{
                "severity": "error",
                "location": "pack",
                "message": other.to_string(),
            }],
        }),
    }
}

/// The success answer for a pack change.
fn loaded_reply(pack: &RulePack) -> Value {
    json!({
        "valid": true,
        "name": pack.name,
        "revision": pack.revision,
        "rules": pack.rules.len(),
    })
}

/// A dotted identifier not present in `taken`, extending with `-2`, `-3`, … when needed.
fn unique_id(base: &str, taken: &[String]) -> String {
    let mut candidate = if base.is_empty() {
        "rule".to_string()
    } else {
        base.to_string()
    };
    let mut n = 1;
    while taken.iter().any(|id| id == &candidate) {
        n += 1;
        candidate = format!("{base}-{n}");
    }
    candidate
}

/// Fills blank step and condition ids in a builder-authored rule.
///
/// YAML authors write ids by hand; the visual builder works with display names, so blank ids get
/// stable generated ones (`c1`, `a2`, `f1`) that execution records can reference.
fn assign_step_and_condition_ids(rule: &mut Rule) {
    let mut taken: Vec<String> = rule
        .conditions
        .iter()
        .map(|c| c.id.as_str().to_owned())
        .filter(|id| !id.trim().is_empty())
        .collect();
    taken.extend(
        rule.actions
            .iter()
            .chain(rule.policy.fallback.iter())
            .map(|s| s.id.as_str().to_owned())
            .filter(|id| !id.trim().is_empty()),
    );
    let mut next = |prefix: &str| {
        let base = format!("{prefix}{}", taken.len() + 1);
        let id = unique_id(&base, &taken);
        taken.push(id.clone());
        id
    };
    for (i, condition) in rule.conditions.iter_mut().enumerate() {
        if condition.id.is_blank() {
            condition.id = ConditionId::new(next(&format!("c{}", i + 1)));
        }
    }
    for (i, step) in rule.actions.iter_mut().enumerate() {
        if step.id.is_blank() {
            step.id = ActionId::new(next(&format!("a{}", i + 1)));
        }
    }
    for (i, step) in rule.policy.fallback.iter_mut().enumerate() {
        if step.id.is_blank() {
            step.id = ActionId::new(next(&format!("f{}", i + 1)));
        }
    }
}

enum Msg {
    Event(Event),
    Heartbeat(String),
    Control(ControlRequest, Sender<ControlReply>),
}

/// Rule summary for dashboards and the API.
#[derive(Debug, Clone, Serialize)]
pub struct RuleInfo {
    /// Rule id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Rule version.
    pub version: u32,
    /// Armed state — always visible (spec §24).
    pub armed: bool,
    /// Priority label.
    pub priority: String,
    /// Trigger type label.
    pub trigger: String,
    /// Tags.
    pub tags: Vec<String>,
}

/// Device summary for dashboards and the API.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    /// Device id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Category.
    pub kind: Value,
    /// Protocol name.
    pub protocol: String,
    /// Address or port name.
    pub address: Option<String>,
    /// Health label.
    pub health: String,
    /// Last time the device was heard from (epoch ms).
    pub last_seen_ms: Option<u64>,
}

/// A consistent view of the running service, refreshed by the engine thread.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Pack name.
    pub pack_name: String,
    /// Whether the engine is forced into simulation.
    pub simulation: bool,
    /// Rules.
    pub rules: Vec<RuleInfo>,
    /// Devices.
    pub devices: Vec<DeviceInfo>,
    /// Most recent executions, oldest first.
    pub recent: VecDeque<ExecutionRecord>,
    /// `(execution id, rule id)` of chains currently running.
    pub active_chains: Vec<(String, String)>,
    /// Worst device health seen.
    pub worst_health: Option<DeviceHealth>,
}

/// Counters for traffic the service refused.
#[derive(Debug, Default)]
pub struct InboundStats {
    /// Datagrams that failed validation.
    pub malformed: AtomicU64,
    /// Events dropped because the engine queue was full.
    pub queue_dropped: AtomicU64,
}

/// Keeps a worker thread's slot in [ServiceHandle::live_workers] while it runs.
///
/// The guard exists so that a listener which *panics* is counted the same as one which returns.
/// Nothing else in the runtime notices a thread dying: the process stays up, the engine keeps
/// ticking and /health keeps answering, so without this a single panicking listener would
/// silently remove one protocol from the running configuration for the rest of the show.
pub(crate) struct WorkerGuard {
    live: Arc<AtomicUsize>,
    reported: AtomicBool,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
        // One message per worker, on the way out of an unwind that may be unwinding already.
        if !self.reported.swap(true, Ordering::SeqCst) {
            tracing::error!(
                "a listener thread exited unexpectedly; that protocol is no longer receiving"
            );
        }
    }
}
/// A cloneable handle for talking to a running (or about-to-run) service.
#[derive(Clone)]
pub struct ServiceHandle {
    tx: SyncSender<Msg>,
    pub(crate) snapshot: Arc<Mutex<Snapshot>>,
    pub(crate) store: Option<Arc<Mutex<Store>>>,
    subscribers: Arc<Mutex<Vec<Sender<String>>>>,
    shutdown: Arc<AtomicBool>,
    cancel: CancelHandle,
    api_addr: Arc<Mutex<Option<SocketAddr>>>,
    ready: Arc<AtomicBool>,
    pub(crate) stats: Arc<InboundStats>,
    pub(crate) inbound: Arc<Inbound>,
    /// Listener/API threads currently running.
    ///
    /// A listener that panics takes only its own thread down, and the process keeps serving
    /// `/health` as if nothing had happened, so the loss has to be counted and reported or one
    /// protocol goes quietly deaf. See [`WorkerGuard`].
    live_workers: Arc<AtomicUsize>,
    /// Listener/API threads this run started, for comparison against `live_workers`.
    expected_workers: Arc<AtomicUsize>,
}

impl ServiceHandle {
    /// Registers a worker thread as expected to run, returning its liveness guard.
    ///
    /// Holding the guard for as long as the thread runs is what makes a dead listener visible:
    /// [Self::live_workers] falls back the moment the thread exits, whether it returned or
    /// unwound.
    pub(crate) fn register_worker(&self) -> WorkerGuard {
        self.expected_workers.fetch_add(1, Ordering::SeqCst);
        self.live_workers.fetch_add(1, Ordering::SeqCst);
        WorkerGuard {
            live: self.live_workers.clone(),
            reported: AtomicBool::new(false),
        }
    }

    /// Listener/API threads still running.
    pub fn live_workers(&self) -> usize {
        self.live_workers.load(Ordering::SeqCst)
    }

    /// Listener/API threads this run started.
    pub fn expected_workers(&self) -> usize {
        self.expected_workers.load(Ordering::SeqCst)
    }

    /// Asks the service to stop.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    /// Whether shutdown was requested.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Queues an event for the engine. Returns `false` (and counts a drop) when the queue is full.
    pub fn send_event(&self, event: Event) -> bool {
        match self.tx.try_send(Msg::Event(event)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.stats.queue_dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Records a device heartbeat.
    pub fn send_heartbeat(&self, device: &str) {
        let _ = self.tx.try_send(Msg::Heartbeat(device.to_owned()));
    }

    /// Sends a control request and waits for the engine thread's answer.
    pub fn control(&self, request: ControlRequest) -> ControlReply {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(Msg::Control(request, reply_tx))
            .map_err(|_| "service is not running".to_string())?;
        reply_rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| "the engine did not answer in time".to_string())?
    }

    /// Cancels running chains (spec §10.2).
    pub fn cancel_handle(&self) -> &CancelHandle {
        &self.cancel
    }

    /// Subscribes to the live execution stream; each message is one JSON document.
    pub fn subscribe(&self) -> Receiver<String> {
        let (tx, rx) = mpsc::channel();
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(tx);
        rx
    }

    /// The current snapshot.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The bound API address, once the API is up.
    pub fn api_addr(&self) -> Option<SocketAddr> {
        *self.api_addr.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Blocks until the service is running (listeners bound, API up) or the timeout passes.
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.ready.load(Ordering::SeqCst) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        self.ready.load(Ordering::SeqCst)
    }

    /// Datagrams refused so far: `(malformed, rate limited, queue overflow)`.
    pub fn rejected_counts(&self) -> (u64, u64, u64) {
        (
            self.stats.malformed.load(Ordering::Relaxed),
            self.inbound.rate_limited(),
            self.stats.queue_dropped.load(Ordering::Relaxed),
        )
    }

    /// Adaptive-backoff episodes started so far (a source repeatedly exceeding its budget).
    pub fn backoff_episodes(&self) -> u64 {
        self.inbound.backoff_episodes()
    }

    /// Datagrams rejected while their source was already muted by backoff.
    pub fn backed_off(&self) -> u64 {
        self.inbound.backed_off()
    }

    pub(crate) fn set_api_addr(&self, addr: SocketAddr) {
        *self.api_addr.lock().unwrap_or_else(|e| e.into_inner()) = Some(addr);
    }
}

/// The automation service.
pub struct Service {
    engine: Engine,
    config: ServiceConfig,
    handle: ServiceHandle,
    rx: Receiver<Msg>,
    probes: Vec<(String, Arc<dyn Endpoint>)>,
    /// Every bound endpoint by device id, for manual pings from the device manager.
    ping_targets: BTreeMap<String, Arc<dyn Endpoint>>,
    device_by_ip: HashMap<IpAddr, String>,
}

impl Service {
    /// Builds a service from already-parsed inputs. Nothing is started and nothing is bound yet.
    ///
    /// Everything that can be wrong with the configuration is rejected here: pack errors, device
    /// file errors, rules that target devices that do not exist, unsafe API settings.
    pub fn build(
        pack: RulePack,
        devices: DeviceFile,
        config: ServiceConfig,
        store: Option<Store>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let mut diagnostics = config.validate();
        diagnostics.extend(devices.validate());
        if has_errors(&diagnostics) {
            return Err(Error::Validation(diagnostics));
        }

        let store = store.map(|s| Arc::new(Mutex::new(s)));
        let sink = Arc::new(TracingSink);
        // With a database, incidents are persisted (and still logged); without one they are only logged.
        let incidents: Arc<dyn IncidentSink> = match &store {
            Some(store) => Arc::new(StoreIncidents {
                store: store.clone(),
                clock: clock.clone(),
            }),
            None => sink.clone(),
        };
        let mut actions = Actions::new(sink, incidents, Arc::new(ThreadSleeper));
        if !config.exec_allow.is_empty() {
            actions.set_exec_policy(ExecPolicy::allowing(config.exec_allow.iter().cloned()));
        }

        let mut registry = DeviceRegistry::new();
        let mut probes = Vec::new();
        let mut ping_targets = BTreeMap::new();
        let mut device_by_ip = HashMap::new();
        for device in &devices.devices {
            registry.register(device.to_device());
            let endpoint: Arc<dyn Endpoint> = Arc::from(build_endpoint(device)?);
            actions.bind_endpoint(device.id.clone(), endpoint.clone());
            ping_targets.insert(device.id.clone(), endpoint.clone());
            match device.protocol.as_str() {
                // Connectionless network protocols cannot be probed; their health comes from the
                // traffic they send us, so only these endpoint kinds are pinged.
                "virtual" | "midi" => probes.push((device.id.clone(), endpoint)),
                _ => {}
            }
            if let Some(address) = &device.address {
                if let Some(addr) = address.to_socket_addrs().ok().and_then(|mut a| a.next()) {
                    device_by_ip.insert(addr.ip(), device.id.clone());
                }
            }
        }

        let engine_config = EngineConfig {
            mode: if config.simulate {
                Mode::Simulation
            } else {
                Mode::Live
            },
            local_clock: LocalClock::new(config.utc_offset_minutes),
            ..EngineConfig::default()
        };
        let mut engine = Engine::new(pack, registry, actions, clock, engine_config)?;
        let device_errors = engine.check_devices();
        if has_errors(&device_errors) {
            return Err(Error::Validation(device_errors));
        }

        if let Some(shared) = &store {
            let mut store = shared.lock().unwrap_or_else(|e| e.into_inner());
            let now = engine.now().as_millis();
            store.save_pack(engine.pack(), now)?;
            store.save_devices(&devices)?;
            if let Some(state) = store.load_state()? {
                engine.restore(state);
            }
            drop(store);
            // Write-ahead: persist a schedule firing before its actions run, so a hard kill mid-chain
            // cannot cause a one-shot to fire again after the restart.
            let hook_store = shared.clone();
            engine.set_state_hook(move |state| {
                let store = hook_store.lock().unwrap_or_else(|e| e.into_inner());
                if let Err(e) = store.save_state(state) {
                    tracing::error!("could not persist scheduler state: {e}");
                }
            });
        }

        let (tx, rx) = mpsc::sync_channel(QUEUE_DEPTH);
        let handle = ServiceHandle {
            tx,
            snapshot: Arc::new(Mutex::new(Snapshot::default())),
            store,
            subscribers: Arc::new(Mutex::new(Vec::new())),
            shutdown: Arc::new(AtomicBool::new(false)),
            cancel: engine.cancel_handle(),
            api_addr: Arc::new(Mutex::new(None)),
            ready: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(InboundStats::default()),
            inbound: Arc::new(Inbound::new(InboundLimits::default())),
            live_workers: Arc::new(AtomicUsize::new(0)),
            expected_workers: Arc::new(AtomicUsize::new(0)),
        };
        let mut service = Self {
            engine,
            config,
            handle,
            rx,
            probes,
            ping_targets,
            device_by_ip,
        };
        service.refresh_snapshot();
        Ok(service)
    }

    /// A handle for controlling the service from other threads.
    pub fn handle(&self) -> ServiceHandle {
        self.handle.clone()
    }

    /// Direct access to the engine, for embedding and tests.
    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// Runs until shutdown is requested. Binds listeners and the API first; any bind failure is
    /// returned before the engine starts.
    pub fn run(mut self) -> Result<()> {
        let mut workers: Vec<JoinHandle<()>> = Vec::new();
        for listener in self.config.listeners.clone() {
            workers.push(spawn_listener(
                &listener,
                self.handle.clone(),
                self.device_by_ip.clone(),
            )?);
        }
        if self.config.api.enabled {
            let (addr, worker) = crate::api::spawn(self.handle.clone(), &self.config.api)?;
            self.handle.set_api_addr(addr);
            workers.push(worker);
        }
        self.handle.ready.store(true, Ordering::SeqCst);
        tracing::info!(rules = self.engine.pack().rules.len(), "service started");

        self.main_loop();

        self.handle.shutdown();
        self.persist_state();
        for worker in workers {
            let _ = worker.join();
        }
        tracing::info!("service stopped");
        Ok(())
    }

    fn main_loop(&mut self) {
        let tick = Duration::from_millis(self.config.tick_ms.max(1));
        let ping = Duration::from_millis(self.config.heartbeat_interval_ms.max(1));
        let wait = tick.min(Duration::from_millis(50));
        let mut last_tick = Instant::now();
        let mut last_ping = Instant::now() - ping;
        let mut last_state_save = Instant::now();
        let mut dirty = false;

        while !self.handle.is_shutdown() {
            let first = match self.rx.recv_timeout(wait) {
                Ok(msg) => Some(msg),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let mut batch = Vec::new();
            batch.extend(first);
            while batch.len() < 256 {
                match self.rx.try_recv() {
                    Ok(msg) => batch.push(msg),
                    Err(_) => break,
                }
            }
            for msg in batch {
                let records = self.handle_msg(msg);
                dirty |= !records.is_empty();
                self.publish(&records);
            }

            if last_tick.elapsed() >= tick {
                last_tick = Instant::now();
                let records = self.engine.tick();
                dirty |= !records.is_empty();
                self.publish(&records);
            }
            if last_ping.elapsed() >= ping {
                last_ping = Instant::now();
                let probes: Vec<_> = self
                    .probes
                    .iter()
                    .map(|(id, ep)| (id.clone(), ep.clone()))
                    .collect();
                for (id, endpoint) in probes {
                    if endpoint.ping().is_ok() {
                        if let Ok(records) = self.engine.device_heartbeat(&id) {
                            dirty |= !records.is_empty();
                            self.publish(&records);
                        }
                    }
                }
            }
            if dirty || last_state_save.elapsed() >= Duration::from_secs(5) {
                self.persist_state();
                last_state_save = Instant::now();
                dirty = false;
            }
            self.refresh_snapshot();
        }
    }

    fn handle_msg(&mut self, msg: Msg) -> Vec<ExecutionRecord> {
        match msg {
            Msg::Event(event) => self.engine.handle_event(event),
            Msg::Heartbeat(device) => self.engine.device_heartbeat(&device).unwrap_or_default(),
            Msg::Control(request, reply) => {
                let (records, answer) = self.control(request);
                let _ = reply.send(answer);
                records
            }
        }
    }

    fn control(&mut self, request: ControlRequest) -> (Vec<ExecutionRecord>, ControlReply) {
        let fail = |e: Error| (Vec::new(), Err(e.to_string()));
        match request {
            ControlRequest::Arm(id) => match self.engine.arm(&id) {
                Ok(()) => (Vec::new(), Ok(json!({ "rule": id, "armed": true }))),
                Err(e) => fail(e),
            },
            ControlRequest::Disarm(id) => match self.engine.disarm(&id) {
                Ok(()) => (Vec::new(), Ok(json!({ "rule": id, "armed": false }))),
                Err(e) => fail(e),
            },
            ControlRequest::Run(id) => match self.engine.run_rule(&id) {
                Ok(records) => {
                    let results: Vec<Value> = records.iter().map(machine_result).collect();
                    (records, Ok(json!({ "executions": results })))
                }
                Err(e) => fail(e),
            },
            ControlRequest::PackYaml => match self.engine.pack().to_yaml_string() {
                Ok(yaml) => (Vec::new(), Ok(Value::String(yaml))),
                Err(e) => fail(e),
            },
            ControlRequest::PackJson => match serde_json::to_value(self.engine.pack()) {
                Ok(pack) => (Vec::new(), Ok(pack)),
                Err(e) => fail(e.into()),
            },
            ControlRequest::ValidatePack(yaml) => (Vec::new(), self.validate_request(&yaml)),
            ControlRequest::LoadPack(yaml) => self.load_pack_request(&yaml),
            ControlRequest::UpsertRule(rule) => match serde_json::from_value::<Rule>(rule) {
                Ok(mut rule) => {
                    // The builder works with names, not ids: steps and conditions an operator
                    // did not name get stable generated ids, exactly as the YAML format expects.
                    let mut taken: Vec<String> = self
                        .engine
                        .pack()
                        .rules
                        .iter()
                        .map(|r| r.id.as_str().to_owned())
                        .collect();
                    taken.retain(|id| id != rule.id.as_str());
                    if rule.id.is_blank() {
                        rule.id = RuleId::new(unique_id(&slugify_rule_id(&rule.name), &taken));
                    }
                    assign_step_and_condition_ids(&mut rule);
                    let mut pack = self.engine.pack().clone();
                    match pack.rules.iter().position(|r| r.id == rule.id) {
                        Some(index) => pack.rules[index] = rule,
                        None => pack.rules.push(rule),
                    }
                    self.apply_pack(pack)
                }
                Err(e) => (Vec::new(), Err(format!("invalid rule: {e}"))),
            },
            ControlRequest::RemoveRule(id) => {
                let mut pack = self.engine.pack().clone();
                let before = pack.rules.len();
                pack.rules.retain(|r| r.id.as_str() != id);
                if pack.rules.len() == before {
                    return (Vec::new(), Err(format!("unknown rule `{id}`")));
                }
                self.apply_pack(pack)
            }
            ControlRequest::Simulation(on) => {
                self.engine
                    .set_mode(if on { Mode::Simulation } else { Mode::Live });
                (Vec::new(), Ok(json!({ "simulation": on })))
            }
            ControlRequest::PingDevice(id) => match self.ping_targets.get(&id) {
                Some(endpoint) => {
                    let reachable = endpoint.ping().is_ok();
                    (
                        Vec::new(),
                        Ok(json!({ "device": id, "reachable": reachable })),
                    )
                }
                None => (Vec::new(), Err(format!("unknown device `{id}`"))),
            },
        }
    }

    /// Validates a candidate pack against the current registry without applying it.
    fn validate_request(&self, yaml: &str) -> ControlReply {
        Ok(match RulePack::from_yaml_str(yaml) {
            Err(e) => validation_failure(e),
            Ok(pack) => {
                let device_errors = self.engine.check_devices_for(&pack);
                if has_errors(&device_errors) {
                    json!({
                        "valid": false,
                        "diagnostics": device_errors,
                    })
                } else {
                    json!({
                        "valid": true,
                        "name": pack.name,
                        "revision": pack.revision,
                        "rules": pack.rules.len(),
                    })
                }
            }
        })
    }

    /// Validates and hot-loads a pack, persisting it to the version history (spec §15).
    fn load_pack_request(&mut self, yaml: &str) -> (Vec<ExecutionRecord>, ControlReply) {
        let pack = match RulePack::from_yaml_str(yaml) {
            Ok(pack) => pack,
            Err(e) => return (Vec::new(), Ok(validation_failure(e))),
        };
        let device_errors = self.engine.check_devices_for(&pack);
        if has_errors(&device_errors) {
            return (
                Vec::new(),
                Ok(json!({ "valid": false, "diagnostics": device_errors })),
            );
        }
        match self.engine.load_pack(pack) {
            Ok(()) => {
                self.persist_pack();
                (Vec::new(), Ok(loaded_reply(self.engine.pack())))
            }
            // load_pack only fails on validation, so the editor still gets a structured answer.
            Err(e) => (Vec::new(), Ok(validation_failure(e))),
        }
    }

    /// Applies an already-assembled pack through the same validation path as [`Self::load_pack_request`].
    fn apply_pack(&mut self, pack: RulePack) -> (Vec<ExecutionRecord>, ControlReply) {
        let device_errors = self.engine.check_devices_for(&pack);
        if has_errors(&device_errors) {
            return (
                Vec::new(),
                Ok(json!({ "valid": false, "diagnostics": device_errors })),
            );
        }
        match self.engine.load_pack(pack) {
            Ok(()) => {
                self.persist_pack();
                (Vec::new(), Ok(loaded_reply(self.engine.pack())))
            }
            Err(e) => (Vec::new(), Ok(validation_failure(e))),
        }
    }

    fn persist_pack(&self) {
        if let Some(store) = &self.handle.store {
            let store = store.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = store.save_pack(self.engine.pack(), self.engine.now().as_millis()) {
                tracing::error!("could not persist rule pack: {e}");
            }
        }
    }

    /// Persists records, then streams them to subscribers. Storage failures are logged, never
    /// fatal: losing history must not stop the show (failure isolation, spec §24).
    fn publish(&mut self, records: &[ExecutionRecord]) {
        if records.is_empty() {
            return;
        }
        if let Some(store) = &self.handle.store {
            let store = store.lock().unwrap_or_else(|e| e.into_inner());
            for record in records {
                if let Err(e) = store.append_execution(record) {
                    tracing::error!("could not persist execution {}: {e}", record.execution_id);
                }
            }
            if let Err(e) = store.prune_executions(self.config.keep_executions) {
                tracing::error!("could not prune execution history: {e}");
            }
        }
        let mut subscribers = self
            .handle
            .subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for record in records {
            if let Ok(line) = serde_json::to_string(record) {
                subscribers.retain(|tx| tx.send(line.clone()).is_ok());
            }
        }
        drop(subscribers);
        let mut snapshot = self
            .handle
            .snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for record in records {
            snapshot.recent.push_back(record.clone());
            while snapshot.recent.len() > RECENT_LIMIT {
                snapshot.recent.pop_front();
            }
        }
    }

    fn persist_state(&self) {
        if let Some(store) = &self.handle.store {
            let store = store.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = store.save_state(&self.engine.snapshot()) {
                tracing::error!("could not persist engine state: {e}");
            }
        }
    }

    fn refresh_snapshot(&mut self) {
        let rules = self
            .engine
            .pack()
            .rules
            .iter()
            .map(|r| RuleInfo {
                id: r.id.to_string(),
                name: r.name.clone(),
                version: r.version,
                armed: self.engine.is_armed(r.id.as_str()),
                priority: r.priority.to_string(),
                trigger: r.trigger.type_label().to_owned(),
                tags: r.tags.clone(),
            })
            .collect();
        let devices = self
            .engine
            .registry()
            .devices()
            .map(|d| DeviceInfo {
                id: d.id.to_string(),
                name: d.name.clone(),
                kind: serde_json::to_value(d.kind).unwrap_or(Value::Null),
                protocol: d.protocol.protocol.clone(),
                address: d.protocol.address.clone(),
                health: d.health.to_string(),
                last_seen_ms: d.last_seen.map(Timestamp::as_millis),
            })
            .collect();
        let mut snapshot = self
            .handle
            .snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        snapshot.pack_name = self.engine.pack().name.clone();
        snapshot.simulation = self.engine.mode() == Mode::Simulation;
        snapshot.rules = rules;
        snapshot.devices = devices;
        snapshot.active_chains = self.handle.cancel.active();
        snapshot.worst_health = Some(self.engine.registry().worst_health());
    }
}

/// Persists `log.incident` entries to the database and mirrors them to the log.
struct StoreIncidents {
    store: Arc<Mutex<Store>>,
    clock: Arc<dyn Clock>,
}

impl IncidentSink for StoreIncidents {
    fn record(&self, incident: Incident) -> std::result::Result<(), String> {
        TracingSink.record(incident.clone())?;
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .append_incident(self.clock.now().as_millis(), &incident)
            .map_err(|e| e.to_string())
    }
}

/// Listens on a MIDI input port, reconnecting if it is not (yet) present.
fn spawn_midi_listener(port: String, handle: ServiceHandle) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let _worker = handle.register_worker();
        let mut connection = None;
        let mut warned = false;
        while !handle.is_shutdown() {
            if connection.is_none() {
                let h = handle.clone();
                let source = format!("midi:{port}");
                let opened = tpt_app_av_automation_devices::midi_port::open_input(
                    &port,
                    move |bytes| match h.inbound.midi(&source, bytes, Timestamp::now()) {
                        Ok(events) => {
                            for event in events {
                                h.send_event(event);
                            }
                        }
                        Err(e) => {
                            h.stats.malformed.fetch_add(1, Ordering::Relaxed);
                            tracing::debug!("rejected inbound MIDI message: {e}");
                        }
                    },
                );
                match opened {
                    Ok(c) => {
                        tracing::info!("listening on MIDI input `{port}`");
                        connection = Some(c);
                    }
                    Err(e) if !warned => {
                        tracing::warn!("MIDI input not available yet ({e}); will keep retrying");
                        warned = true;
                    }
                    Err(_) => {}
                }
            }
            for _ in 0..10 {
                if handle.is_shutdown() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    })
}

/// Reads a DMX512-A serial line, reconnecting if the adapter is not (yet) present.
///
/// The port delivers bytes with no framing of its own, so [`Dmx512Assembler`] recovers the break
/// boundaries and hands back whole frames; each frame is diffed into channel changes exactly as
/// Art-Net and sACN are, so rules do not depend on the transport.
fn spawn_dmx512_listener(port: String, handle: ServiceHandle) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let _worker = handle.register_worker();
        let mut reader: Option<Box<dyn DmxSerialReader>> = None;
        let mut assembler = Dmx512Assembler::new();
        let mut warned = false;
        while !handle.is_shutdown() {
            if reader.is_none() {
                match tpt_app_av_automation_devices::dmx_serial::open_reader(&port, 100) {
                    Ok(r) => {
                        tracing::info!("listening on DMX512 serial port `{port}`");
                        // A reopened adapter starts mid-frame, so any partial state is discarded.
                        assembler = Dmx512Assembler::new();
                        reader = Some(r);
                        warned = false;
                    }
                    Err(e) if !warned => {
                        tracing::warn!("DMX512 port not available yet ({e}); will keep retrying");
                        warned = true;
                    }
                    Err(_) => {}
                }
            }
            if let Some(r) = reader.as_mut() {
                match r.read_timed() {
                    Ok(bytes) => {
                        for frame in assembler.push_all(&bytes) {
                            let result =
                                handle
                                    .inbound
                                    .dmx_frame(1, frame.payload(), Timestamp::now());
                            match result {
                                Ok(events) => {
                                    handle.send_heartbeat(&port);
                                    for event in events {
                                        handle.send_event(event);
                                    }
                                }
                                Err(e) => {
                                    handle.stats.malformed.fetch_add(1, Ordering::Relaxed);
                                    tracing::debug!("rejected inbound DMX512 frame: {e}");
                                }
                            }
                        }
                    }
                    // An adapter pulled mid-show drops the handle so the next pass reopens it,
                    // rather than spinning on a dead port.
                    Err(e) => {
                        tracing::warn!("DMX512 read failed ({e}); reopening the port");
                        reader = None;
                        continue;
                    }
                }
            }
            for _ in 0..5 {
                if handle.is_shutdown() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    })
}

fn spawn_listener(
    config: &ListenerConfig,
    handle: ServiceHandle,
    device_by_ip: HashMap<IpAddr, String>,
) -> Result<JoinHandle<()>> {
    if config.protocol == "midi" {
        return Ok(spawn_midi_listener(config.bind.clone(), handle));
    }
    if config.protocol == "dmx512" {
        return Ok(spawn_dmx512_listener(config.bind.clone(), handle));
    }
    let socket = UdpSocket::bind(&config.bind).map_err(|e| {
        Error::Control(format!(
            "cannot listen on {} ({}): {e}",
            config.bind, config.protocol
        ))
    })?;
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|e| Error::Control(e.to_string()))?;
    let protocol = config.protocol.clone();
    Ok(std::thread::spawn(move || {
        let _worker = handle.register_worker();
        let mut buf = vec![0u8; 8192];
        let mut last_seen: BTreeMap<String, Instant> = BTreeMap::new();
        while !handle.is_shutdown() {
            let (n, from) = match socket.recv_from(&mut buf) {
                Ok(v) => v,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue
                }
                Err(e) => {
                    tracing::warn!("listener receive error: {e}");
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
            };
            let now = Timestamp::now();
            let source = from.ip().to_string();
            let result = match protocol.as_str() {
                "osc" => handle.inbound.osc(&source, &buf[..n], now),
                "artnet" => handle.inbound.artnet(&source, &buf[..n], now),
                // MIDI 2.0 arrives as raw Universal MIDI Packets over the venue network.
                "ump" => handle.inbound.ump(&source, &buf[..n], now),
                _ => handle.inbound.sacn(&source, &buf[..n], now),
            };
            match result {
                Ok(events) => {
                    if let Some(device) = device_by_ip.get(&from.ip()) {
                        let due = last_seen
                            .get(device)
                            .is_none_or(|t| t.elapsed() >= Duration::from_secs(1));
                        if due {
                            last_seen.insert(device.clone(), Instant::now());
                            handle.send_heartbeat(device);
                        }
                    }
                    for event in events {
                        handle.send_event(event);
                    }
                }
                Err(e) => {
                    handle.stats.malformed.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(%source, "rejected inbound {protocol} datagram: {e}");
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A handle with no engine behind it, which is all the worker accounting needs.
    fn bare_handle() -> ServiceHandle {
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        ServiceHandle {
            tx,
            snapshot: Arc::new(Mutex::new(Snapshot::default())),
            store: None,
            subscribers: Arc::new(Mutex::new(Vec::new())),
            shutdown: Arc::new(AtomicBool::new(false)),
            cancel: CancelHandle::default(),
            api_addr: Arc::new(Mutex::new(None)),
            ready: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(InboundStats::default()),
            inbound: Arc::new(Inbound::new(InboundLimits::default())),
            live_workers: Arc::new(AtomicUsize::new(0)),
            expected_workers: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[test]
    fn a_worker_is_counted_while_it_runs_and_unaccounted_when_it_stops() {
        let handle = bare_handle();
        let guard = handle.register_worker();
        assert_eq!(handle.expected_workers(), 1);
        assert_eq!(handle.live_workers(), 1);
        assert_eq!(handle.live_workers(), handle.expected_workers());

        drop(guard);
        assert_eq!(
            handle.live_workers(),
            0,
            "a stopped worker is no longer live"
        );
        assert_eq!(
            handle.expected_workers(),
            1,
            "but it is still expected, which is what makes the loss visible"
        );
        assert!(handle.live_workers() < handle.expected_workers());
    }

    #[test]
    fn a_worker_that_panics_is_still_unaccounted() {
        // The whole point of the guard: a thread that unwinds must release its slot exactly as a
        // thread that returns does, otherwise a panic leaves the service reporting "nominal"
        // while a protocol has stopped being received.
        let handle = bare_handle();
        let worker = handle.clone();
        let panicked = std::thread::spawn(move || {
            let _guard = worker.register_worker();
            panic!("listener died");
        });
        assert!(panicked.join().is_err(), "the thread really did panic");
        // The slot is released as the unwinding thread exits.
        assert_eq!(handle.live_workers(), 0);
        assert_eq!(handle.expected_workers(), 1);
    }

    #[test]
    fn workers_are_counted_independently() {
        let handle = bare_handle();
        let a = handle.register_worker();
        let b = handle.register_worker();
        assert_eq!(handle.live_workers(), 2);
        assert_eq!(handle.expected_workers(), 2);
        drop(a);
        assert_eq!(handle.live_workers(), 1);
        assert_eq!(handle.expected_workers(), 2);
        drop(b);
        assert_eq!(handle.live_workers(), 0);
        assert_eq!(handle.expected_workers(), 2);
    }
}
