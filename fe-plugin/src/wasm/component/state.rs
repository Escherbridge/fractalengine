//! Per-instance host state behind every 2.0 import; see src/wasm/AGENTS.md §instance-state.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fe_sdk::manifest_v2::HOST_FUEL_CEILING;
use fe_sdk::plugin_host::{NodeReader, PendingOp};

use crate::capability::V2Grants;
use crate::context::PluginContext;
use crate::tick::{Clock, SharedWindow, DEFAULT_TICK_FUEL};
use crate::transaction::{StagedWindow, WindowError};

use super::bindings::fractalengine::plugin::types::{Binding, HostError};
use super::broker::{ExtensionBroker, NoBroker};
use super::kv::KvSlot;
use super::routes::RouteCache;
use super::wasi::{build_wasi_ctx, GuestLog};
use super::ComponentError;

/// Linear-memory ceiling for each memory of an instance (today's pooled cap, OD-E4 measures).
pub const INSTANCE_MEMORY_BYTES: usize = 16 * 1024 * 1024;
/// Core instances a component may create (main module, WASI adapter, shims).
const MAX_CORE_INSTANCES: usize = 32;
/// Tables and memories a component may create.
const MAX_TABLES: usize = 32;
const MAX_MEMORIES: usize = 8;
/// Elements in any one table.
const MAX_TABLE_ELEMENTS: usize = 65_536;

/// Everything the host hands one instance at activation.
pub struct InstanceSetup {
    /// The WIT `binding` record (host-minted; never guest input).
    pub binding: Binding,
    /// The admitted context: v2 token, bound petal, reader and sinks.
    pub context: PluginContext,
    /// The window `txn` / `readings` stage into and the tick scheduler flushes.
    pub window: SharedWindow,
    /// The injected clock (`WallClock` in production, the `SimClock` adapter in the lab).
    pub clock: Arc<dyn Clock>,
    /// Fuel for each hook call (`on-tick` is refuelled by the scheduler with the same budget).
    pub hook_fuel: u64,
    /// The `kv` grant, absent when the manifest declares none.
    pub kv: Option<KvSlot>,
    /// Bus, egress and ingress broker (E5); [`NoBroker`] answers `not-available`.
    pub broker: Arc<dyn ExtensionBroker>,
    /// Unfiltered reader for route geometry; without it `routes` sees only in-scope properties.
    pub geometry_reader: Option<Arc<Mutex<dyn NodeReader>>>,
    /// Activation count of this binding; seeds the guest's deterministic randomness.
    pub activation: u64,
}

impl InstanceSetup {
    /// Defaults: hook fuel from the token's tick budget, no kv, no broker, activation 1.
    pub fn new(binding: Binding, context: PluginContext, window: SharedWindow, clock: Arc<dyn Clock>) -> Self {
        let hook_fuel = context
            .capabilities()
            .tick_budget()
            .map_or(DEFAULT_TICK_FUEL, |budget| budget.fuel);
        Self {
            binding,
            context,
            window,
            clock,
            hook_fuel: hook_fuel.min(HOST_FUEL_CEILING),
            kv: None,
            broker: Arc::new(NoBroker),
            geometry_reader: None,
            activation: 1,
        }
    }

    /// Grant `kv` (the caller passes the manifest's `kv.max_bytes`).
    pub fn with_kv(mut self, slot: KvSlot) -> Self {
        self.kv = Some(slot);
        self
    }

    /// Route bus, egress and ingress calls to the E5 broker.
    pub fn with_broker(mut self, broker: Arc<dyn ExtensionBroker>) -> Self {
        self.broker = broker;
        self
    }

    /// Read route geometry through the host's own reader (same petal pin, no property filter).
    pub fn with_geometry_reader(mut self, reader: Arc<Mutex<dyn NodeReader>>) -> Self {
        self.geometry_reader = Some(reader);
        self
    }

    /// Set the activation count (restart / update), which reseeds randomness.
    pub fn with_activation(mut self, activation: u64) -> Self {
        self.activation = activation;
        self
    }

    /// Override the per-hook fuel, clamped to the host ceiling.
    pub fn with_hook_fuel(mut self, fuel: u64) -> Self {
        self.hook_fuel = fuel.min(HOST_FUEL_CEILING);
        self
    }

    /// The binding must describe the context it runs under; a mismatch is a host bug, refused.
    pub(crate) fn check(&self) -> Result<(), ComponentError> {
        let grants = self
            .context
            .capabilities()
            .grants_v2
            .as_ref()
            .ok_or_else(|| ComponentError::Activation("a 2.0 instance needs a manifest v2 token".into()))?;
        if self.binding.installation_id.is_empty() {
            return Err(ComponentError::Activation("binding has no installation id".into()));
        }
        if self.binding.petal_id != self.context.petal_id() {
            return Err(ComponentError::Activation("binding petal differs from the bound context".into()));
        }
        if self.binding.package_id != grants.package_id {
            return Err(ComponentError::Activation("binding package differs from the token".into()));
        }
        Ok(())
    }
}

/// The `T` of every `wasmtime::Store` the component host creates.
pub struct InstanceState {
    pub(crate) binding: Binding,
    pub(crate) context: PluginContext,
    pub(crate) window: SharedWindow,
    /// `stage-create-node` ordinals handed out in the current window.
    pub(crate) created_in_window: u32,
    /// The injected clock, frozen for the duration of one hook call.
    pub(crate) now_ms: Arc<AtomicU64>,
    pub(crate) kv: Option<KvSlot>,
    pub(crate) broker: Arc<dyn ExtensionBroker>,
    pub(crate) routes: RouteCache,
    pub(crate) geometry_reader: Option<Arc<Mutex<dyn NodeReader>>>,
    pub(crate) log: Arc<Mutex<GuestLog>>,
    wasi: wasmtime_wasi::WasiCtx,
    table: wasmtime::component::ResourceTable,
    pub(crate) limits: wasmtime::StoreLimits,
}

impl InstanceState {
    pub(crate) fn new(setup: InstanceSetup) -> Self {
        let now_ms = Arc::new(AtomicU64::new(setup.clock.now_ms()));
        let log = Arc::new(Mutex::new(GuestLog::new(
            &setup.binding.installation_id,
            &setup.binding.package_id,
            now_ms.clone(),
        )));
        let wasi = build_wasi_ctx(&setup.binding.installation_id, setup.activation, now_ms.clone(), log.clone());
        let limits = wasmtime::StoreLimitsBuilder::new()
            .memory_size(INSTANCE_MEMORY_BYTES)
            .table_elements(MAX_TABLE_ELEMENTS)
            .instances(MAX_CORE_INSTANCES)
            .tables(MAX_TABLES)
            .memories(MAX_MEMORIES)
            .build();
        Self {
            binding: setup.binding,
            context: setup.context,
            window: setup.window,
            created_in_window: 0,
            now_ms,
            kv: setup.kv,
            broker: setup.broker,
            routes: RouteCache::default(),
            geometry_reader: setup.geometry_reader,
            log,
            wasi,
            table: wasmtime::component::ResourceTable::new(),
            limits,
        }
    }

    /// The v2 grants of the instance token.
    pub(crate) fn grants(&self) -> Option<&V2Grants> {
        self.context.capabilities().grants_v2.as_ref()
    }

    /// The frozen hook-call instant (epoch ms).
    pub(crate) fn now(&self) -> u64 {
        self.now_ms.load(Ordering::Acquire)
    }

    /// Freeze the clock for the next hook call.
    pub(crate) fn freeze_clock(&self, now_ms: u64) {
        self.now_ms.store(now_ms, Ordering::Release);
    }

    /// Refuse every effect once the host revoked this instance's lease.
    pub(crate) fn lease_active(&self) -> Result<(), HostError> {
        self.context
            .revocation_lease()
            .require_active()
            .map_err(HostError::Revoked)
    }

    /// Run a stage against the shared window; an emptied window restarts the create ordinals.
    pub(crate) fn with_window<R>(
        &mut self,
        stage: impl FnOnce(&mut StagedWindow, &mut u32) -> Result<R, WindowError>,
    ) -> Result<R, HostError> {
        let mut window = self
            .window
            .lock()
            .map_err(|_| HostError::Backend("staged window lock poisoned".into()))?;
        if window.is_empty() {
            self.created_in_window = 0;
        }
        stage(&mut window, &mut self.created_in_window).map_err(window_error)
    }

    /// Authorize then stage one op; a `create-node` returns its ordinal in this window.
    pub(crate) fn stage_op(&mut self, op: PendingOp) -> Result<Option<u32>, HostError> {
        self.lease_active()?;
        self.context.authorize_op(&op).map_err(context_error)?;
        let creates = matches!(op, PendingOp::CreateNode { .. });
        self.with_window(|window, created| {
            window.stage_op(op)?;
            if !creates {
                return Ok(None);
            }
            let ordinal = *created;
            *created += 1;
            Ok(Some(ordinal))
        })
    }

    /// Drop the window (guest `txn.discard`, or a faulted hook).
    pub(crate) fn discard_window(&mut self) {
        if let Ok(mut window) = self.window.lock() {
            window.discard();
        }
        self.created_in_window = 0;
    }
}

impl wasmtime_wasi::WasiView for InstanceState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

// ---------------------------------------------------------------------------
// host-error mapping (src/wasm/AGENTS.md §host-errors)
// ---------------------------------------------------------------------------

/// Largest `json` argument, in bytes.
pub const MAX_JSON_ARG_BYTES: usize = fe_sdk::plugin_host::MAX_HOST_BYTES;

/// `invalid-input` with a reason.
pub(crate) fn invalid(detail: impl Into<String>) -> HostError {
    HostError::InvalidInput(detail.into())
}

/// `capability-denied` for an undeclared surface.
pub(crate) fn denied(surface: &str) -> HostError {
    HostError::CapabilityDenied(format!("the manifest does not grant {surface}"))
}

/// Parse a guest `json` argument: bounded, well-formed, finite numbers only.
pub(crate) fn parse_json_arg(text: &str) -> Result<serde_json::Value, HostError> {
    if text.len() > MAX_JSON_ARG_BYTES {
        return Err(invalid(format!("json argument exceeds {MAX_JSON_ARG_BYTES} bytes")));
    }
    serde_json::from_str(text).map_err(|error| invalid(format!("json argument: {error}")))
}

/// Classify a `PluginContext` refusal (plain text) into the WIT vocabulary.
pub(crate) fn context_error(message: String) -> HostError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("denied") {
        HostError::CapabilityDenied(message)
    } else if lower.contains("no longer admitted") || lower.contains("ancestry changed") || lower.contains("shutting down") {
        HostError::Revoked(message)
    } else if lower.contains("unavailable") {
        HostError::NotAvailable(message)
    } else if lower.contains("reader") || lower.contains("sink") || lower.contains("poisoned") {
        HostError::Backend(message)
    } else if lower.contains("exceeds") || lower.contains("invalid") || lower.contains("must") || lower.contains("another petal") {
        HostError::InvalidInput(message)
    } else {
        HostError::Backend(message)
    }
}

/// Map a flush refusal (`"<host-error kind>: <detail>"`, fe-runtime `PluginRefusal`) back to its case.
pub(crate) fn refusal_error(message: &str) -> HostError {
    let Some((kind, detail)) = message.split_once(": ") else {
        return context_error(message.to_string());
    };
    let detail = detail.to_string();
    match kind {
        "capability-denied" => HostError::CapabilityDenied(detail),
        "policy-denied" => HostError::PolicyDenied(detail),
        "invalid-input" => HostError::InvalidInput(detail),
        "budget-exceeded" => HostError::BudgetExceeded(detail),
        "rate-limited" => HostError::RateLimited(detail),
        "revoked" => HostError::Revoked(detail),
        "not-available" => HostError::NotAvailable(detail),
        "backend" => HostError::Backend(detail),
        "unknown-outcome" => HostError::UnknownOutcome(detail),
        _ => context_error(message.to_string()),
    }
}

/// A full window is `budget-exceeded`; a value the SDK refuses is `invalid-input`.
pub(crate) fn window_error(error: WindowError) -> HostError {
    match error {
        WindowError::Full(limit) => HostError::BudgetExceeded(format!("staged window is full: {limit}")),
        WindowError::Invalid(detail) => HostError::InvalidInput(detail),
    }
}
