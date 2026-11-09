//! Component host for WIT `fractalengine:plugin@2.0.0` (world `extension`), the only third-party
//! contract (D-X1, spec R2). Design and rationale: src/wasm/AGENTS.md.

pub mod bindings;
pub mod broker;
pub mod bus_bridge;
mod imports;
pub mod instance;
pub mod kv;
pub mod routes;
pub mod state;
pub mod wasi;
#[cfg(test)]
pub(crate) mod test_guest;

use wasmtime::component::{Component, HasSelf, Linker};

use self::bindings::ExtensionPre;
pub use self::bindings::fractalengine::plugin::types::{Binding, Caller, CallerKind, HostError};
pub use self::broker::{ExtensionBroker, NoBroker};
pub use self::bus_bridge::BusBroker;
pub use self::instance::{window_for_token, ExtensionInstance, HookFault, InstanceHandle, InstanceStats, InstanceStore};
pub use self::kv::{KvBackend, KvSlot, MemoryKv};
pub use self::state::{InstanceSetup, InstanceState};
use crate::trust_roots::RevocationSet;
use crate::wasm::aot::AotCache;

/// Largest `plugin/component.wasm` the host compiles (docs/extensions/package-format.md).
pub const MAX_COMPONENT_BYTES: usize = 16 * 1024 * 1024;

/// The four interfaces every component must export.
pub const REQUIRED_EXPORTS: [&str; 4] = [
    "fractalengine:plugin/lifecycle@2.0.0",
    "fractalengine:plugin/ingress-hooks@2.0.0",
    "fractalengine:plugin/egress-hooks@2.0.0",
    "fractalengine:plugin/bus-hooks@2.0.0",
];

/// Why a component was refused or could not start; `code()` is the install refusal code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ComponentError {
    /// A core module (the retired 1.0 ABI) or a contract version this host does not bind.
    #[error("unsupported-contract: {0}")]
    UnsupportedContract(String),
    /// Not a loadable component under the policy (`forbidden-import <name>`, `missing-export <name>`, ...).
    #[error("package-invalid: {0}")]
    PackageInvalid(String),
    /// The package `content_hash` or the `component_digest` is on an accepted kill list.
    #[error("revoked: {0}")]
    Revoked(String),
    /// Instantiation or the binding/context pairing failed; nothing ran.
    #[error("activation-refused: {0}")]
    Activation(String),
    /// The host itself failed (engine or linker setup).
    #[error("component-host: {0}")]
    Host(String),
}

impl ComponentError {
    /// The refusal code governance.md and the REST install route print.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedContract(_) => "unsupported-contract",
            Self::PackageInvalid(_) => "package-invalid",
            Self::Revoked(_) => "revoked",
            Self::Activation(_) => "activation-refused",
            Self::Host(_) => "component-host",
        }
    }
}

fn host_error(error: wasmtime::Error) -> ComponentError {
    ComponentError::Host(format!("{error:#}"))
}

/// The binary layer: only a component (layer 1) is a 2.0 guest; a core module is the retired ABI.
fn check_binary(bytes: &[u8]) -> Result<(), ComponentError> {
    if bytes.len() > MAX_COMPONENT_BYTES {
        return Err(ComponentError::PackageInvalid("component-too-large".into()));
    }
    if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
        return Err(ComponentError::PackageInvalid("not-wasm".into()));
    }
    // Preamble: magic, u16 version, u16 layer (0 = core module, 1 = component).
    match (bytes[6], bytes[7]) {
        (1, 0) => Ok(()),
        (0, 0) => Err(ComponentError::UnsupportedContract(
            "a core module is the retired 1.0 ABI; build a component against fractalengine:plugin@2.0.0".into(),
        )),
        _ => Err(ComponentError::PackageInvalid("unknown wasm layer".into())),
    }
}

/// A component that passed the install checks, pre-linked against the 2.0 world.
pub struct ExtensionComponent {
    pre: ExtensionPre<InstanceState>,
    digest: String,
}

impl ExtensionComponent {
    /// blake3 hex of the component bytes (package-format.md `component_digest`).
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// Engine and linker for 2.0 guests: the world's imports plus the closed WASI list.
pub struct ComponentHost {
    engine: wasmtime::Engine,
    linker: Linker<InstanceState>,
}

impl ComponentHost {
    /// Fuel-metered engine with the component model; on-demand allocation bounded by `StoreLimits`.
    pub fn new() -> Result<Self, ComponentError> {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        let engine = wasmtime::Engine::new(&config).map_err(host_error)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(host_error)?;
        bindings::Extension::add_to_linker::<InstanceState, HasSelf<InstanceState>>(&mut linker, |state| state)
            .map_err(host_error)?;
        Ok(Self { engine, linker })
    }

    /// The component engine (AOT artifacts are only valid on it).
    pub fn engine(&self) -> &wasmtime::Engine {
        &self.engine
    }

    /// Compile and admit `plugin/component.wasm` (package-format.md §Component checks).
    pub fn compile(&self, bytes: &[u8]) -> Result<ExtensionComponent, ComponentError> {
        check_binary(bytes)?;
        let component = Component::new(&self.engine, bytes)
            .map_err(|error| ComponentError::PackageInvalid(format!("not-a-component {error:#}")))?;
        self.admit(component, AotCache::hash_bytes(bytes))
    }

    /// [`compile`](Self::compile) for a sealed package body: `revoked` when the package
    /// `content_hash` OR this component's digest is on the kill list, under any package id
    /// (governance.md precondition 5). The lifecycle runs this at install and every activation.
    pub fn compile_unrevoked(
        &self,
        bytes: &[u8],
        content_hash: &str,
        revocations: &RevocationSet,
    ) -> Result<ExtensionComponent, ComponentError> {
        let digest = AotCache::hash_bytes(bytes);
        for (label, hash) in [("content_hash", content_hash), ("component_digest", digest.as_str())] {
            if revocations.is_revoked(hash) {
                return Err(ComponentError::Revoked(format!("{label} {hash} is revoked")));
            }
        }
        self.compile(bytes)
    }

    /// [`compile`](Self::compile) through the AOT cache (keyed by the component bytes).
    pub fn compile_cached(&self, cache: &AotCache, bytes: &[u8]) -> Result<ExtensionComponent, ComponentError> {
        check_binary(bytes)?;
        let compiled = cache
            .get_or_compile_component(&self.engine, bytes)
            .map_err(|error| ComponentError::PackageInvalid(format!("not-a-component {error}")))?;
        let component = cache
            .load_compiled_component(&self.engine, &compiled)
            .map_err(|error| ComponentError::Host(error.to_string()))?;
        self.admit(component, AotCache::hash_bytes(bytes))
    }

    /// Import policy, required exports, then a typed pre-link against the world.
    fn admit(&self, component: Component, digest: String) -> Result<ExtensionComponent, ComponentError> {
        let ty = component.component_type();
        for (name, _) in ty.imports(&self.engine) {
            wasi::check_import(name)?;
        }
        if let Some(missing) = REQUIRED_EXPORTS
            .iter()
            .find(|name| ty.get_export(&self.engine, name).is_none())
        {
            return Err(ComponentError::PackageInvalid(format!("missing-export {missing}")));
        }
        let linked = self
            .linker
            .instantiate_pre(&component)
            .map_err(|error| ComponentError::PackageInvalid(format!("unlinkable {error:#}")))?;
        let pre = ExtensionPre::new(linked)
            .map_err(|error| ComponentError::PackageInvalid(format!("export-mismatch {error:#}")))?;
        Ok(ExtensionComponent { pre, digest })
    }

    /// Instantiate one binding of an admitted component. Fuel and clock are set before any
    /// guest code (start functions included) runs. No hook is called here.
    pub fn instantiate(
        &self,
        component: &ExtensionComponent,
        setup: InstanceSetup,
    ) -> Result<ExtensionInstance, ComponentError> {
        setup.check()?;
        let clock = setup.clock.clone();
        let hook_fuel = setup.hook_fuel;
        let mut store = wasmtime::Store::new(&self.engine, InstanceState::new(setup));
        store.limiter(|state| &mut state.limits);
        store.set_fuel(hook_fuel).map_err(host_error)?;
        store.data().freeze_clock(clock.now_ms());
        let pre = &component.pre;
        let bindings = instance::outside_async(|| pre.instantiate(&mut store))
            .map_err(|error| ComponentError::Activation(format!("instantiate failed: {error:#}")))?;
        Ok(ExtensionInstance::new(store, bindings, clock, hook_fuel))
    }
}
