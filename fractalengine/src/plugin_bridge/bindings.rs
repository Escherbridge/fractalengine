use super::*;

/// Summed tick counters of an installation's instances.
#[derive(Default)]
pub(super) struct TickTotals {
    pub(super) ticks_run: u64,
    pub(super) ticks_skipped: u64,
    pub(super) flushes: u64,
    pub(super) commits: u64,
    pub(super) refusals: u64,
    pub(super) unknown_outcomes: u64,
}

pub(super) fn tick_totals(ticks: &TickHost, instances: &[RunningInstance]) -> TickTotals {
    instances
        .iter()
        .filter_map(|instance| ticks.counters(&instance.plugin_id))
        .fold(TickTotals::default(), |mut totals, counters| {
            totals.ticks_run += counters.ticks_run;
            totals.ticks_skipped += counters.ticks_skipped;
            totals.flushes += counters.flushes;
            totals.commits += counters.commits;
            totals.refusals += counters.refusals;
            totals.unknown_outcomes += counters.unknown_outcomes;
            totals
        })
}

// ---------------------------------------------------------------------------
// fe-api seams: ExtensionHost and BindingHost
// ---------------------------------------------------------------------------

/// Everything the API hosts read; cheap to clone.
#[derive(Resource, Clone)]
pub(crate) struct ExtensionHostParts {
    pub(super) local_did: String,
    pub(super) local_client_did: Option<String>,
    pub(super) bus: Bus,
    pub(super) net: Net,
    pub(super) runtime: Option<tokio::runtime::Handle>,
    pub(super) ticks: TickHost,
    pub(super) governance: ExtensionGovernance,
    pub(super) index: InstallationIndex,
    pub(super) view: BindingView,
    pub(super) live: LiveInstances,
    pub(super) roles: Arc<dyn RoleSource>,
    pub(super) audit: Arc<dyn AuditSink>,
    pub(super) signals: Option<BindingSignals>,
    pub(super) trace: fe_runtime::extension_trace::ExtensionTraceBuffer,
}

impl ExtensionHostParts {
    /// Run `task` on the caller's runtime, else the network runtime, else a short-lived thread.
    pub(super) fn spawn(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        if let Some(handle) = tokio::runtime::Handle::try_current()
            .ok()
            .or_else(|| self.runtime.clone())
        {
            handle.spawn(task);
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("fe-ext-forward".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(task),
                    Err(error) => tracing::warn!(%error, "extension forward runtime did not start"),
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "extension forward thread did not start");
        }
    }

    pub(super) fn signal(&self, signal: BindingSignal) {
        if let Some(signals) = &self.signals {
            signals.send(signal);
        }
    }
}

/// [`ExtensionHost`] over the bus, the network broker, governance and the plugin manager.
pub(crate) struct BusExtensionHost {
    pub(super) parts: ExtensionHostParts,
}

impl BusExtensionHost {
    fn is_local_client(&self, caller: &ApiCaller) -> bool {
        self.parts.local_client_did.as_deref().is_some_and(|cid| {
            !cid.is_empty() && caller.client.as_deref() == Some(cid)
        })
    }

    pub(super) fn bus_caller(&self, caller: &ApiCaller) -> Caller {
        let kind = match caller.kind {
            ApiCallerKind::Rest => CallerKind::Rest,
            ApiCallerKind::Mcp => CallerKind::Mcp,
        };
        // The bus recomputes `local`; the token's max_role caps the node DID's Owner role (G1 security fix).
        Caller {
            did: caller.did.clone(),
            kind,
            package_id: None,
            local: caller.did == self.parts.bus.local_did()
                && (caller.role_ceiling.is_at_least(RoleLevel::Owner) || self.is_local_client(caller)),
            role_ceiling: Some(caller.role_ceiling),
        }
    }

    /// Role floor first, capped by the token's ceiling (the network broker sees no ceiling); refusal is the uniform 404.
    pub(super) fn ingress_gate(
        &self,
        caller_did: &str,
        ceiling: RoleLevel,
        petal_id: &str,
        package_id: &str,
        endpoint: &str,
        websocket: bool,
    ) -> Result<(), IngressRefusal> {
        let key = InstanceKey::new(petal_id.to_string(), package_id.to_string());
        let floor = self
            .parts
            .live
            .manifest_at(&key)
            .and_then(|manifest| endpoint_floor(&manifest, endpoint, websocket))
            .ok_or(IngressRefusal::NotFound)?;
        if capped(self.parts.roles.role_at(caller_did, petal_id), ceiling).is_at_least(floor) {
            return Ok(());
        }
        self.parts.audit.record(AuditEntry {
            action: AuditAction::Denial,
            actor_did: caller_did.to_string(),
            caller_kind: None,
            installation_id: None,
            target: format!("{package_id}/ingress/{endpoint}"),
            args_digest: None,
            outcome: "not-found".into(),
            at_ms: now_ms(),
        });
        Err(IngressRefusal::NotFound)
    }

    pub(super) fn status_doc(&self, installation_id: &str, row: &IndexRow) -> Value {
        let ticks = &self.parts.ticks;
        let totals = tick_totals(ticks, &row.instances);
        let bus = self.parts.bus.counters_for_installation(installation_id);
        let net = self.parts.net.counters_for_installation(installation_id);
        let any_degraded = row.instances.iter().any(|instance| instance.degraded);
        let any_running = row.instances.iter().any(|instance| !instance.degraded);
        let state = if any_running {
            "active"
        } else if any_degraded {
            "degraded"
        } else if row.revoked {
            "revoked"
        } else if row.disabled {
            "disabled"
        } else {
            "installed"
        };
        let instances: Vec<Value> = row
            .instances
            .iter()
            .map(|instance| {
                json!({
                    "instance_id": instance.plugin_id,
                    "verse_id": instance.verse_id,
                    "petal_id": instance.petal_id,
                    "content_hash": instance.content_hash,
                    "origin": if instance.origin == InstanceOrigin::Binding { "binding" } else { "local-bind" },
                    "degraded": instance.degraded,
                })
            })
            .collect();
        let commands_refused: u64 = bus
            .as_ref()
            .map_or(0, |counters| counters.commands_refused.values().sum());
        // Host-wide binding counts (`bindings.resolved`, `bindings.unresolved.<state>`).
        let bindings: serde_json::Map<String, Value> = self
            .parts
            .view
            .status_rows()
            .into_iter()
            .map(|(name, count)| (name, json!(count)))
            .collect();
        json!({
            "installation_id": installation_id,
            "package_id": row.package_id,
            "version": row.version,
            "name": row.name,
            "state": state,
            "consented": row.consented,
            "instances": instances,
            "counters": {
                "ticks_run": totals.ticks_run,
                "ticks_skipped": totals.ticks_skipped,
                "flushes": totals.flushes,
                "commits": totals.commits,
                "refusals": totals.refusals,
                "unknown_outcomes": totals.unknown_outcomes,
                "ingress_frames": net.as_ref().map_or(0, |counters| counters.ingress_frames),
                "commands_ok": bus.as_ref().map_or(0, |counters| counters.commands_ok),
                "commands_refused": commands_refused,
            },
            "bus": bus,
            "net": net,
            "bindings": bindings,
        })
    }
}

pub(super) fn host_refusal(refusal: BusRefusal) -> HostRefusal {
    match refusal {
        BusRefusal::NotFound => HostRefusal::NotFound,
        BusRefusal::NotAvailable(detail) => HostRefusal::NotAvailable(detail),
        BusRefusal::CapabilityDenied(detail) => HostRefusal::CapabilityDenied(detail),
        BusRefusal::RateLimited(detail) => HostRefusal::RateLimited(detail),
        BusRefusal::InvalidInput(detail) => HostRefusal::InvalidInput(detail),
        BusRefusal::LimitReached(detail) => HostRefusal::LimitReached(detail),
    }
}

pub(super) fn ingress_refusal(refusal: NetRefusal) -> IngressRefusal {
    match refusal {
        NetRefusal::NotFound => IngressRefusal::NotFound,
        NetRefusal::TooManyRequests => IngressRefusal::TooManyRequests,
        NetRefusal::PayloadTooLarge => IngressRefusal::PayloadTooLarge,
        NetRefusal::MethodNotAllowed => IngressRefusal::MethodNotAllowed,
        NetRefusal::UnsupportedMediaType => IngressRefusal::UnsupportedMediaType,
    }
}

pub(super) fn command_failure(outcome: CommandOutcome) -> Result<String, CommandFailure> {
    match outcome {
        CommandOutcome::Ok(text) => Ok(text),
        CommandOutcome::Guest(CommandError::InvalidInput(detail)) => {
            Err(CommandFailure::InvalidInput(detail))
        }
        CommandOutcome::Guest(CommandError::Refused(detail)) => {
            Err(CommandFailure::Refused(detail))
        }
        CommandOutcome::Guest(CommandError::Failed(detail)) => Err(CommandFailure::Failed(detail)),
        CommandOutcome::Host(HostError::UnknownOutcome(detail)) => {
            Err(CommandFailure::UnknownOutcome(detail))
        }
        CommandOutcome::Host(HostError::NotAvailable(detail)) => {
            Err(CommandFailure::NotAvailable(detail))
        }
        CommandOutcome::Host(other) => Err(CommandFailure::Failed(other.to_string())),
    }
}

pub(super) fn admin_refusal(code: &str, message: impl Into<String>) -> AdminRefusal {
    AdminRefusal {
        code: code.to_string(),
        message: message.into(),
    }
}

impl ExtensionHost for BusExtensionHost {
    fn local_did(&self) -> String {
        self.parts.bus.local_did().to_string()
    }

    fn admit_remote(&self, did: &str) -> bool {
        let admitted = self.parts.bus.admit_remote(did);
        if admitted {
            // The bus role view learns this DID's grants at every known petal.
            self.parts.governance.watch_did(did);
        }
        admitted
    }

    fn list(&self) -> Vec<Value> {
        self.parts
            .index
            .rows()
            .iter()
            .map(|(id, row)| self.status_doc(id, row))
            .collect()
    }

    fn status(&self, installation: &str) -> Option<Value> {
        let row = self.parts.index.row(installation)?;
        Some(self.status_doc(installation, &row))
    }

    fn audit(&self, installation: &str, limit: usize) -> Option<Vec<Value>> {
        self.parts
            .governance
            .audit_rows(installation, limit.min(MAX_AUDIT_ROWS))
    }

    fn administer(&self, _actor_did: &str, op: AdminOp) -> Result<Value, AdminRefusal> {
        // Fail closed even if a door ever reaches here: administration is the Extensions panel and the local
        // `fe-ext admin` channel only, both through the ops worker (user decision 2026-11-09; src/AGENTS.md §admin-channel).
        Err(admin_refusal(
            "admin-gui-only",
            format!("{} runs through the Extensions panel or fe-ext admin on this host", op.verb.as_str()),
        ))
    }

    fn record_denied_admin(&self, actor_did: &str, target: &str) {
        self.parts.audit.record(AuditEntry {
            action: AuditAction::Denial,
            actor_did: actor_did.to_string(),
            caller_kind: None,
            installation_id: None,
            target: target.to_string(),
            args_digest: None,
            outcome: "non-local-admin".into(),
            at_ms: now_ms(),
        });
    }

    fn call_command(
        &self,
        caller: &ApiCaller,
        petal_id: &str,
        package_id: &str,
        command: &str,
        args: &Value,
    ) -> Result<CommandReply, HostRefusal> {
        let bus_caller = self.bus_caller(caller);
        let outcome = if self.is_local_client(caller) {
            self.parts.bus.call_command_local_client(
                &bus_caller,
                petal_id,
                package_id,
                command,
                args,
            )
        } else {
            self.parts.bus.call_command(
                &bus_caller,
                petal_id,
                package_id,
                command,
                args,
            )
        };
        let ticket = match outcome {
            Ok(ticket) => ticket,
            Err(refusal) => {
                let err = host_refusal(refusal);
                self.parts.trace.record(fe_runtime::extension_trace::ExtensionTraceInput {
                    kind: "command".into(),
                    package_id: package_id.to_owned(),
                    petal_id: Some(petal_id.to_owned()),
                    hook: None,
                    key: Some(command.to_owned()),
                    outcome: "refused".into(),
                    detail: Some(format!("{err:?}")),
                    duration_ms: None,
                });
                return Err(err);
            }
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        let outcome = ticket.reply;
        let trace = self.parts.trace.clone();
        let pkg = package_id.to_owned();
        let petal = petal_id.to_owned();
        let cmd = command.to_owned();
        let started = Instant::now();
        self.parts.spawn(async move {
            let result = match outcome.await {
                Ok(outcome) => command_failure(outcome),
                Err(_) => Err(CommandFailure::UnknownOutcome(
                    "the command reply was dropped".into(),
                )),
            };
            let (status, detail) = match &result {
                Ok(_) => ("ok", None),
                Err(e) => ("error", Some(format!("{e:?}"))),
            };
            trace.record(fe_runtime::extension_trace::ExtensionTraceInput {
                kind: "command".into(),
                package_id: pkg,
                petal_id: Some(petal),
                hook: None,
                key: Some(cmd),
                outcome: status.into(),
                detail,
                duration_ms: Some(started.elapsed().as_millis() as u64),
            });
            let _ = tx.send(result);
        });
        Ok(rx)
    }

    fn ingress(
        &self,
        call: IngressCall,
    ) -> Result<tokio::sync::oneshot::Receiver<Option<IngressAnswer>>, IngressRefusal> {
        if let Err(err) = self.ingress_gate(
            &call.caller_did,
            call.role_ceiling,
            &call.petal_id,
            &call.package_id,
            &call.endpoint,
            false,
        ) {
            self.parts.trace.record(fe_runtime::extension_trace::ExtensionTraceInput {
                kind: "ingress".into(),
                package_id: call.package_id.clone(),
                petal_id: Some(call.petal_id.clone()),
                hook: None,
                key: Some(call.endpoint.clone()),
                outcome: "refused".into(),
                detail: Some(format!("{err:?}")),
                duration_ms: None,
            });
            return Err(err);
        }
        let pkg = call.package_id.clone();
        let petal = call.petal_id.clone();
        let endpoint = call.endpoint.clone();
        let receiver = match self
            .parts
            .net
            .accept_http(HttpIngressRequest {
                petal_id: call.petal_id,
                package_id: call.package_id,
                endpoint: call.endpoint,
                caller_did: call.caller_did,
                method: call.method,
                sub_path: call.sub_path,
                query: call.query,
                headers: call.headers,
                content_type: call.content_type,
                body: call.body,
            }) {
            Ok(rx) => rx,
            Err(refusal) => {
                let err = ingress_refusal(refusal);
                self.parts.trace.record(fe_runtime::extension_trace::ExtensionTraceInput {
                    kind: "ingress".into(),
                    package_id: pkg,
                    petal_id: Some(petal),
                    hook: None,
                    key: Some(endpoint),
                    outcome: "refused".into(),
                    detail: Some(format!("{err:?}")),
                    duration_ms: None,
                });
                return Err(err);
            }
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        let trace = self.parts.trace.clone();
        let started = Instant::now();
        self.parts.spawn(async move {
            // A dropped guest answer (instance stopped) drops `tx`: fe-api answers it as unavailable, never 202.
            if let Ok(answer) = receiver.await {
                let status_label = match &answer {
                    Some(reply) => format!("http_{}", reply.status),
                    None => "dropped".into(),
                };
                trace.record(fe_runtime::extension_trace::ExtensionTraceInput {
                    kind: "ingress".into(),
                    package_id: pkg,
                    petal_id: Some(petal),
                    hook: None,
                    key: Some(endpoint),
                    outcome: status_label,
                    detail: None,
                    duration_ms: Some(started.elapsed().as_millis() as u64),
                });
                let _ = tx.send(answer.map(|reply: IngressReply| IngressAnswer {
                    status: reply.status,
                    content_type: reply.content_type,
                    body: reply.body,
                }));
            }
        });
        Ok(rx)
    }

    fn open_websocket(&self, call: WsAcceptCall) -> Result<WsHostSession, IngressRefusal> {
        self.ingress_gate(
            &call.caller_did,
            call.role_ceiling,
            &call.petal_id,
            &call.package_id,
            &call.endpoint,
            true,
        )?;
        let session = self
            .parts
            .net
            .open_websocket(WsAcceptRequest {
                petal_id: call.petal_id,
                package_id: call.package_id,
                endpoint: call.endpoint,
                caller_did: call.caller_did,
            })
            .map_err(ingress_refusal)?;
        let connection = session.connection;
        let mut outbound = session.outbound;
        let (tx, rx) = tokio::sync::mpsc::channel(fe_plugin::net::OUTBOUND_QUEUE);
        self.parts.spawn(async move {
            while let Some(out) = outbound.recv().await {
                let mapped = match out {
                    WsOutbound::Frame(WsFrame::Text(text)) => WsOut::Frame(WsData::Text(text)),
                    WsOutbound::Frame(WsFrame::Binary(bytes)) => {
                        WsOut::Frame(WsData::Binary(bytes))
                    }
                    WsOutbound::Close { code, reason } => WsOut::Close { code, reason },
                };
                if tx.send(mapped).await.is_err() {
                    break;
                }
            }
        });
        Ok(WsHostSession {
            connection,
            outbound: rx,
        })
    }

    fn websocket_frame(&self, connection: u64, frame: WsData) {
        let frame = match frame {
            WsData::Text(text) => WsFrame::Text(text),
            WsData::Binary(bytes) => WsFrame::Binary(bytes),
        };
        self.parts.net.websocket_frame(connection, frame);
    }

    fn websocket_closed(&self, connection: u64) {
        self.parts.net.websocket_closed(connection);
    }

    fn open_client(&self, caller: &ApiCaller) -> (u64, Arc<tokio::sync::Notify>) {
        let bus_caller = self.bus_caller(caller);
        let handle = self.parts.bus.open_client_capped(
            &caller.did,
            bus_caller.kind,
            bus_caller.role_ceiling,
            ClientGrants::Api,
        );
        (handle.id, handle.notify)
    }

    fn client_subscribe(
        &self,
        client: u64,
        petal_id: &str,
        source: &str,
        params: &Value,
    ) -> Result<u64, HostRefusal> {
        self.parts
            .bus
            .client_subscribe(client, petal_id, source, params)
            .map_err(host_refusal)
    }

    fn client_unsubscribe(&self, client: u64, subscription: u64) {
        self.parts.bus.client_unsubscribe(client, subscription);
    }

    fn take_client_events(&self, client: u64, max: usize) -> Vec<String> {
        self.parts
            .bus
            .take_client_events(client, max)
            .iter()
            .map(|event| event.envelope_text())
            .collect()
    }

    fn close_client(&self, client: u64) {
        self.parts.bus.close_client(client);
    }

    fn verse_policy(&self, verse_id: &str) -> AdminReply {
        self.parts.governance.verse_policy(verse_id)
    }

    fn set_verse_policy(&self, actor_did: &str, verse_id: &str, policy: Value) -> AdminReply {
        self.parts
            .governance
            .set_verse_policy(actor_did, verse_id, policy)
    }
}

/// The binding plane as Bevy systems, fe-api and the terrain bridge reach it; cheap to clone.
#[derive(Resource, Clone)]
pub(crate) struct ExtensionBindings {
    pub(super) local_did: String,
    pub(super) local_client_did: Option<String>,
    pub(super) db: Sender<DbCommand>,
    pub(super) bus: Bus,
    pub(super) roles: Arc<dyn RoleSource>,
    pub(super) governance: ExtensionGovernance,
    pub(super) view: BindingView,
    pub(super) petals: PetalDirectory,
    pub(super) signals: BindingSignals,
}

impl ExtensionBindings {
    /// Ask the worker to call the instance serving `point` at `petal_id` (lazily started); poll the receiver.
    pub(crate) fn call_hook(
        &self,
        petal_id: &str,
        point: BindingPoint,
        input: Value,
    ) -> Receiver<HookAnswer> {
        let (reply, receiver) = crossbeam::channel::bounded(1);
        self.signals.send(BindingSignal::Call {
            petal_id: petal_id.to_string(),
            point,
            input,
            reply,
        });
        receiver
    }

    /// Moves each time the worker republishes the resolution cache (bound portals reload on a change).
    pub(crate) fn generation(&self) -> u64 {
        self.view.generation()
    }

    /// `petal.map` at `petal_id` as `PetalMapResolver::decide` reads it; never blocks, never touches the activator.
    pub(crate) fn petal_map_need(&self, petal_id: &str) -> Need {
        self.view
            .need_at(petal_id, &BindingPoint::new("petal.map", ""))
    }

    /// This host's last resolution of every bound point at `petal_id` (local view; never replicated).
    pub(crate) fn effective_at(&self, petal_id: &str) -> Vec<EffectivePoint> {
        self.view.points_at(petal_id).unwrap_or_default()
    }

    /// Re-resolve a verse (a binding or role change the caller observed).
    pub(crate) fn mark_dirty(&self, verse_id: &str) {
        self.signals.dirty(verse_id, Trigger::BindingRow);
    }

    /// An installation was removed: drop what the activator holds for it.
    pub(crate) fn forget_installation(&self, installation_id: &str) {
        self.signals
            .send(BindingSignal::Forget(installation_id.to_string()));
    }
}

/// Wait for one crossbeam DB reply off the caller's thread and hand it to a oneshot.
pub(super) fn forward_binding_reply<T: Send + 'static>(
    receiver: Receiver<Result<T, BindingRefusal>>,
    then: impl FnOnce(&Result<T, BindingRefusal>) + Send + 'static,
) -> tokio::sync::oneshot::Receiver<Result<T, BindingRefusal>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let work = move || {
        let outcome = receiver.recv_timeout(DB_REPLY_TIMEOUT).unwrap_or_else(|_| {
            Err(BindingRefusal {
                code: "failed".into(),
                message: "the database did not answer in time".into(),
            })
        });
        then(&outcome);
        let _ = tx.send(outcome);
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(work);
        }
        Err(_) => {
            if let Err(error) = std::thread::Builder::new()
                .name("fe-ext-bindings-reply".into())
                .spawn(work)
            {
                tracing::warn!(%error, "binding reply thread did not start");
            }
        }
    }
    rx
}

pub(super) fn refused_now<T>(message: &str) -> tokio::sync::oneshot::Receiver<Result<T, BindingRefusal>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(Err(BindingRefusal {
        code: "failed".into(),
        message: message.into(),
    }));
    rx
}

impl BindingHost for ExtensionBindings {
    fn local_did(&self) -> String {
        self.local_did.clone()
    }

    fn local_client_did(&self) -> Option<String> {
        self.local_client_did.clone()
    }

    fn admit_remote(&self, did: &str) -> bool {
        let admitted = self.bus.admit_remote(did);
        if admitted {
            self.governance.watch_did(did);
        }
        admitted
    }

    fn put_binding(&self, caller_did: &str, template: BindingRecord) -> BindingWriteReply {
        let verse_id = fe_database::parse_scope(&template.scope)
            .ok()
            .map(|parts| parts.verse_id);
        let (reply, receiver) = crossbeam::channel::bounded(1);
        let command = DbCommand::PutBinding {
            caller_did: Some(caller_did.to_string()),
            binding: template,
            reply,
        };
        if self.db.send_timeout(command, DB_SEND_TIMEOUT).is_err() {
            return refused_now("the database is not accepting commands");
        }
        let signals = self.signals.clone();
        forward_binding_reply(receiver, move |outcome| {
            if let (Ok(_), Some(verse_id)) = (outcome, verse_id) {
                signals.dirty(&verse_id, Trigger::BindingRow);
            }
        })
    }

    fn list_bindings(&self, caller_did: &str, scope: &str) -> BindingListReply {
        let (reply, receiver) =
            crossbeam::channel::bounded::<Result<Vec<StoredBinding>, BindingRefusal>>(1);
        let command = DbCommand::ListBindings {
            scope: scope.to_string(),
            caller_did: Some(caller_did.to_string()),
            reply,
        };
        if self.db.send_timeout(command, DB_SEND_TIMEOUT).is_err() {
            return refused_now("the database is not accepting commands");
        }
        forward_binding_reply(receiver, |_| {})
    }

    fn effective(&self, petal_scope: &str, caller_did: &str) -> Option<Vec<EffectivePoint>> {
        let parts = fe_database::parse_scope(petal_scope).ok()?;
        let petal_id = parts.petal_id?;
        if caller_did != self.local_did
            && !self
                .roles
                .role_at(caller_did, &petal_id)
                .is_at_least(RoleLevel::Viewer)
        {
            return None;
        }
        match self.view.points_at(&petal_id) {
            Some(points) => Some(points),
            None if self.petals.contains(&petal_id) => {
                self.signals.dirty(&parts.verse_id, Trigger::Navigation);
                Some(Vec::new())
            }
            None => None,
        }
    }
}

/// The extension host fe-api mounts (`ApiConfig::extension_host`, wired in `main.rs`).
pub(crate) fn api_extension_host(app: &App) -> Option<Arc<dyn ExtensionHost>> {
    let parts = app.world().get_resource::<ExtensionHostParts>()?;
    Some(Arc::new(BusExtensionHost {
        parts: parts.clone(),
    }))
}

/// The binding plane fe-api mounts (`ApiConfig::binding_host`, wired in `main.rs`).
pub(crate) fn api_binding_host(app: &App) -> Option<Arc<dyn BindingHost>> {
    let bindings = app.world().get_resource::<ExtensionBindings>()?;
    Some(Arc::new(bindings.clone()))
}

/// Extension execution trace ring buffer (`GET /api/v1/debug/extensions-trace`).
pub(crate) fn extension_trace(
    app: &App,
) -> Option<fe_runtime::extension_trace::ExtensionTraceBuffer> {
    app.world()
        .get_resource::<fe_runtime::extension_trace::ExtensionTraceBuffer>()
        .cloned()
}

