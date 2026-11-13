//! Guest SDK for FractalEngine extensions (WIT `fractalengine:plugin@2.0.0`, world `extension`; `extension-with-hooks` with feature `hooks`).
//! Design notes: tools/fe-ext/AGENTS.md §guest-sdk. Contract: docs/extensions/authoring-guide.md.

/// The generated bindings, straight from `fe-plugin/wit/fractal-plugin-2.wit`.
pub mod bindings {
    #[cfg(not(feature = "hooks"))]
    wit_bindgen::generate!({
        world: "extension",
        path: "../../../fe-plugin/wit/fractal-plugin-2.wit",
        pub_export_macro: true,
    });
    // Feature `hooks`: the `extension-with-hooks` world, for a manifest that declares `provides`.
    #[cfg(feature = "hooks")]
    wit_bindgen::generate!({
        world: "extension-with-hooks",
        path: "../../../fe-plugin/wit/fractal-plugin-2.wit",
        pub_export_macro: true,
    });
}

pub mod hooks;
pub mod readings_batch;
pub mod tracking;
pub mod window;

pub use hooks::{Call as HookCall, Failure as HookFailure, HookFn, Output as HookOutput};
pub use bindings::exports::fractalengine::plugin::bus_hooks::{BusEvent, CommandError};
pub use bindings::exports::fractalengine::plugin::ingress_hooks::{ConnectionEvent, IngressFrame, IngressReply, Transport};
pub use bindings::exports::fractalengine::plugin::lifecycle::{DeactivateReason, TickInfo};
pub use bindings::fractalengine::plugin::egress::{self, IncomingResponse, OutgoingRequest};
pub use bindings::fractalengine::plugin::types::{Binding, Caller, CallerKind, HostError, NodeSnapshot, Quat, Vec3};
pub use bindings::fractalengine::plugin::{bus, context, ingress, kv, log, readings, routes, scene, txn};

/// Every hook of the world with a no-op default; only `on_tick` is required.
pub trait Extension: 'static {
    /// First activation of this binding. An error refuses activation.
    fn on_install(_binding: Binding) -> Result<(), String> {
        Ok(())
    }
    /// Every activation, including after restarts and updates. An error refuses activation.
    fn on_activate(_binding: Binding) -> Result<(), String> {
        Ok(())
    }
    /// The instance stops.
    fn on_deactivate(_reason: DeactivateReason) {}
    /// The binding is removed or the package uninstalled.
    fn on_uninstall(_binding: Binding) {}
    /// Called on the declared cadence; the staged window flushes when this returns.
    fn on_tick(info: TickInfo);
    /// One inbound request, frame or message on a declared endpoint.
    fn on_ingress(_frame: IngressFrame) -> Option<IngressReply> {
        None
    }
    /// A websocket connection opened or closed.
    fn on_connection(_endpoint: String, _connection: u64, _event: ConnectionEvent) {}
    /// A queued egress request completed.
    fn on_egress_result(_request: u64, _outcome: Result<IncomingResponse, HostError>) {}
    /// A batch of bus deliveries.
    fn on_events(_events: Vec<BusEvent>) {}
    /// A declared bus command; unknown commands are refused.
    fn on_command(command: String, _args: String, _caller: Caller) -> Result<String, CommandError> {
        Err(CommandError::Refused(format!("unknown command {command}")))
    }
    /// A queued `bus.call` completed.
    fn on_call_result(_call: u64, _outcome: Result<String, HostError>) {}
    /// A hook call for a resolved binding (feature `hooks`); unhandled hooks are `Failed`, so the built-in serves.
    #[cfg(feature = "hooks")]
    fn on_hook(call: HookCall) -> Result<HookOutput, HookFailure> {
        Err(HookFailure::Failed(format!("hook {} has no handler in this extension", call.hook)))
    }
}

/// Generated `hook-call` in, generated `hook-output` out, through [`hooks::Call::parse`] and [`hooks::Output::render`].
#[cfg(feature = "hooks")]
#[doc(hidden)]
pub fn __dispatch_hook<E: Extension>(
    call: bindings::exports::fractalengine::plugin::hooks::HookCall,
) -> Result<bindings::exports::fractalengine::plugin::hooks::HookOutput, bindings::exports::fractalengine::plugin::hooks::HookError> {
    use bindings::exports::fractalengine::plugin::hooks::{HookError, HookOutput as WitOutput};
    let to_wit = |failure: HookFailure| match failure {
        HookFailure::Refused(reason) => HookError::Refused(reason),
        HookFailure::InvalidInput(reason) => HookError::InvalidInput(reason),
        HookFailure::Failed(reason) => HookError::Failed(reason),
    };
    let parsed = HookCall::parse(call.hook, call.key, call.binding_scope, &call.config, &call.input, call.body).map_err(to_wit)?;
    let (output, body) = E::on_hook(parsed).and_then(HookOutput::render).map_err(to_wit)?;
    Ok(WitOutput { output, body })
}

/// The `hooks` export glue of [`extension!`]: present with feature `hooks`, empty without it.
#[cfg(feature = "hooks")]
#[doc(hidden)]
#[macro_export]
macro_rules! __hooks_glue {
    ($extension:ty, $glue:ident) => {
        impl $crate::bindings::exports::fractalengine::plugin::hooks::Guest for $glue {
            fn on_hook(
                call: $crate::bindings::exports::fractalengine::plugin::hooks::HookCall,
            ) -> Result<
                $crate::bindings::exports::fractalengine::plugin::hooks::HookOutput,
                $crate::bindings::exports::fractalengine::plugin::hooks::HookError,
            > {
                $crate::__dispatch_hook::<$extension>(call)
            }
        }
    };
}

/// The `hooks` export glue of [`extension!`]: present with feature `hooks`, empty without it.
#[cfg(not(feature = "hooks"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __hooks_glue {
    ($extension:ty, $glue:ident) => {};
}

/// Export `$extension` (a type implementing [`Extension`]) as the component's four hook interfaces (five with feature `hooks`).
#[macro_export]
macro_rules! extension {
    ($extension:ty) => {
        #[doc(hidden)]
        pub struct FeExtGlue;

            impl $crate::bindings::exports::fractalengine::plugin::lifecycle::Guest for FeExtGlue {
                fn on_install(binding: $crate::Binding) -> Result<(), String> {
                    <$extension as $crate::Extension>::on_install(binding)
                }
                fn on_activate(binding: $crate::Binding) -> Result<(), String> {
                    <$extension as $crate::Extension>::on_activate(binding)
                }
                fn on_deactivate(reason: $crate::DeactivateReason) {
                    <$extension as $crate::Extension>::on_deactivate(reason)
                }
                fn on_uninstall(binding: $crate::Binding) {
                    <$extension as $crate::Extension>::on_uninstall(binding)
                }
                fn on_tick(info: $crate::TickInfo) {
                    <$extension as $crate::Extension>::on_tick(info)
                }
            }

            impl $crate::bindings::exports::fractalengine::plugin::ingress_hooks::Guest for FeExtGlue {
                fn on_ingress(frame: $crate::IngressFrame) -> Option<$crate::IngressReply> {
                    <$extension as $crate::Extension>::on_ingress(frame)
                }
                fn on_connection(endpoint: String, connection: u64, event: $crate::ConnectionEvent) {
                    <$extension as $crate::Extension>::on_connection(endpoint, connection, event)
                }
            }

            impl $crate::bindings::exports::fractalengine::plugin::egress_hooks::Guest for FeExtGlue {
                fn on_egress_result(request: u64, outcome: Result<$crate::IncomingResponse, $crate::HostError>) {
                    <$extension as $crate::Extension>::on_egress_result(request, outcome)
                }
            }

            impl $crate::bindings::exports::fractalengine::plugin::bus_hooks::Guest for FeExtGlue {
                fn on_events(events: Vec<$crate::BusEvent>) {
                    <$extension as $crate::Extension>::on_events(events)
                }
                fn on_command(command: String, args: String, caller: $crate::Caller) -> Result<String, $crate::CommandError> {
                    <$extension as $crate::Extension>::on_command(command, args, caller)
                }
                fn on_call_result(call: u64, outcome: Result<String, $crate::HostError>) {
                    <$extension as $crate::Extension>::on_call_result(call, outcome)
                }
            }

            $crate::__hooks_glue!($extension, FeExtGlue);

            $crate::bindings::export!(FeExtGlue with_types_in $crate::bindings);
    };
}
