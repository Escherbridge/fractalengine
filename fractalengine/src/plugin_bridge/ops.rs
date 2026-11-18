use super::*;

// ---------------------------------------------------------------------------
// Ops drain ports (F-OPS)
// ---------------------------------------------------------------------------

/// The audit channel note (`gui` or `cli`, admin-channel.md §7) until fe-database's entry gains a `channel` member.
pub(super) fn channel_note(channel: &str) -> Option<String> {
    Some(format!("channel={channel}"))
}

/// What the ops worker needs from the binary. No method takes the manager lock: the drain calls several of them
/// while it holds it (`rows_event`, `consent_request`).
pub(super) struct BridgeOpsPorts {
    pub(super) governance: ExtensionGovernance,
    pub(super) stager: InstanceStager,
    pub(super) db: Sender<DbCommand>,
    pub(super) owners: VerseOwners,
    pub(super) index: InstallationIndex,
    pub(super) signals: BindingSignals,
    /// This host's last resolution per petal (the Bindings view's states and offers).
    pub(super) view: BindingView,
    /// Identity key and device X25519 secret a licence request names; `None` answers `not-available`.
    pub(super) licence_keys: Option<LicenceKeys>,
    /// Token client over the pinned issuers; `None` when it could not be built (answers `not-available`).
    #[cfg(feature = "entitlement")]
    pub(super) licence_tokens: Option<crate::extension_host::licence_client::LicenceTokens>,
}

/// The licensee's keys, held only for the ops worker's licence ports.
pub(super) struct LicenceKeys {
    pub(super) identity: ed25519_dalek::SigningKey,
    pub(super) device_secret: [u8; 32],
}

impl LicenceKeys {
    pub(super) fn licensee(&self) -> fe_hexon::entitlement_client::Licensee<'_> {
        fe_hexon::entitlement_client::Licensee { identity: &self.identity, device_secret: &self.device_secret }
    }
}

impl Drop for LicenceKeys {
    fn drop(&mut self) {
        // SigningKey zeroizes itself; the raw device secret is wiped here.
        self.device_secret = [0; 32];
    }
}

impl BridgeOpsPorts {
    /// One durable audit entry as the local DID; waits for the DB so the caller can refuse when it is not recorded.
    pub(super) fn append_audit_blocking(&self, entry: PolicyEntry) -> Result<(), String> {
        let entry = serde_json::to_value(&entry).map_err(|error| error.to_string())?;
        let (reply, receiver) = crossbeam::channel::bounded(1);
        self.db
            .send_timeout(DbCommand::AppendExtensionAudit { entry, reply }, DB_SEND_TIMEOUT)
            .map_err(|_| "the database is not accepting commands".to_string())?;
        receiver
            .recv_timeout(DB_REPLY_TIMEOUT)
            .map_err(|_| "the database did not answer; the change may or may not be recorded".to_string())??;
        Ok(())
    }

    pub(super) fn local_entry(&self, action: PolicyAction, target: String) -> PolicyEntry {
        PolicyEntry {
            action,
            installation: None,
            package_id: None,
            content_hash: None,
            actor_did: self.governance.local_did().to_string(),
            target,
            outcome: PolicyOutcome::Ok,
            reason: None,
            args_digest: None,
            verse_id: None,
            capability_digest: None,
            at: rfc3339_utc(now_ms()),
        }
    }
}

impl OpsPorts for BridgeOpsPorts {
    fn now(&self) -> String {
        rfc3339_utc(now_ms())
    }

    fn trust_tier(&self, trusted_by: Option<&str>) -> TrustTier {
        // No vouching root reads as `Publisher`, which `first-party-only` refuses; `DevUnsigned` would be admitted
        // there and only `tier_for(None)` under the debug dev flag may produce it.
        self.governance
            .trust()
            .tier_for(trusted_by)
            .unwrap_or(TrustTier::Publisher)
    }

    fn is_verse_owner(&self, verse_id: &str) -> bool {
        self.owners.is_owner(verse_id)
    }

    fn persist_consent(&self, record: &ConsentRecord) -> Result<(), String> {
        // One durable `consent` audit entry; governance then hands the record to the manager as well.
        self.governance.record_consent(record.clone())?;
        self.signals
            .send(BindingSignal::AllDirty(Trigger::Lifecycle));
        Ok(())
    }

    fn persist_consent_revoke(
        &self,
        installation_id: &str,
        package_id: &str,
    ) -> Result<(), String> {
        let entry = PolicyEntry {
            action: PolicyAction::ConsentRevoke,
            installation: Some(installation_id.to_string()),
            package_id: Some(package_id.to_string()),
            content_hash: None,
            actor_did: self.governance.local_did().to_string(),
            target: installation_id.to_string(),
            outcome: PolicyOutcome::Ok,
            reason: None,
            args_digest: None,
            verse_id: None,
            capability_digest: None,
            at: rfc3339_utc(now_ms()),
        };
        let entry = serde_json::to_value(&entry).map_err(|error| error.to_string())?;
        let (reply, receiver) = crossbeam::channel::bounded(1);
        self.db
            .send_timeout(
                DbCommand::AppendExtensionAudit { entry, reply },
                DB_SEND_TIMEOUT,
            )
            .map_err(|_| "the database is not accepting commands".to_string())?;
        receiver.recv_timeout(DB_REPLY_TIMEOUT).map_err(|_| {
            "the database did not answer; the revoke may or may not be recorded".to_string()
        })??;
        // An uninstall also lands here; the activator drops what it held for the installation.
        self.signals
            .send(BindingSignal::Forget(installation_id.to_string()));
        self.signals
            .send(BindingSignal::AllDirty(Trigger::Lifecycle));
        Ok(())
    }

    fn record_kill_list_override(
        &self,
        verse_id: &str,
        package_id: &str,
        content_hash: &str,
        confirm: &str,
    ) -> Result<(), String> {
        let params = json!({ "verse_id": verse_id, "package_id": package_id, "content_hash": content_hash, "confirm": confirm });
        self.governance
            .kill_list_override(self.governance.local_did(), &params)
            .map_err(|refusal| format!("{}: {}", refusal.code, refusal.message))?;
        self.signals.dirty(verse_id, Trigger::KillList);
        Ok(())
    }

    fn persist_acceptance(
        &self,
        installation_id: &str,
        verse_id: &str,
        package_id: &str,
    ) -> Result<(), String> {
        // fe-database `acceptance_entry` shape: `bind` with the verse, read back by `hydrate_acceptances`.
        let mut entry = self.local_entry(PolicyAction::Bind, format!("{installation_id}@{verse_id}"));
        entry.installation = Some(installation_id.to_string());
        entry.package_id = Some(package_id.to_string());
        entry.verse_id = Some(verse_id.to_string());
        self.append_audit_blocking(entry)?;
        self.governance
            .note_acceptance(self.governance.local_did(), installation_id, verse_id, true);
        self.signals.dirty(verse_id, Trigger::Lifecycle);
        Ok(())
    }

    fn persist_withdrawal(
        &self,
        installation_id: &str,
        verse_id: &str,
        package_id: &str,
    ) -> Result<(), String> {
        // The `unbind`-with-verse twin of `persist_acceptance`; governance's replay reads both.
        let mut entry = self.local_entry(PolicyAction::Unbind, format!("{installation_id}@{verse_id}"));
        entry.installation = Some(installation_id.to_string());
        entry.package_id = Some(package_id.to_string());
        entry.verse_id = Some(verse_id.to_string());
        self.append_audit_blocking(entry)?;
        self.governance
            .note_acceptance(self.governance.local_did(), installation_id, verse_id, false);
        self.signals.dirty(verse_id, Trigger::Lifecycle);
        Ok(())
    }

    fn audit_change(&self, action: PolicyAction, installation_id: &str) {
        self.audit_change_via(action, installation_id, CHANNEL_GUI);
    }

    fn audit_change_via(&self, action: PolicyAction, installation_id: &str, channel: &str) {
        let mut entry = self.local_entry(action, installation_id.to_string());
        if installation_id != "*" {
            entry.installation = Some(installation_id.to_string());
            entry.package_id = Some(installation_id.to_string());
        }
        entry.reason = channel_note(channel);
        self.governance.append_audit(entry);
        self.signals
            .send(BindingSignal::AllDirty(Trigger::Lifecycle));
    }

    fn audit_admin_refusal(&self, action: PolicyAction, target: &str, code: &str, message: &str) {
        // Every refused admin-channel verb is a durable row (admin-channel.md §7), denials included.
        let mut entry = self.local_entry(action, target.to_string());
        entry.outcome = PolicyOutcome::Refused;
        entry.reason = Some(format!("channel=cli; {code}: {message}"));
        self.governance.append_audit(entry);
    }

    fn forget_installation(&self, installation_id: &str) {
        self.signals
            .send(BindingSignal::Forget(installation_id.to_string()));
        self.signals
            .send(BindingSignal::AllDirty(Trigger::Lifecycle));
    }

    fn host_switch_changed(&self, disabled: bool, channel: &str) {
        let action = if disabled { PolicyAction::Disable } else { PolicyAction::Enable };
        self.audit_change_via(action, "*", channel);
        // Disable all reaches the portal too (R-X-33): a mounted bundle unloads, rows it wrote stay (OD-E1).
        portal_host_switch(disabled);
    }

    fn bindings_snapshot(&self, scope: &ActiveScope) -> Result<BindingsSnapshot, String> {
        let petal = self
            .stager
            .petals
            .get(&scope.petal_id)
            .filter(|petal| petal.verse_id == scope.verse_id)
            .ok_or("the open petal is not in the loaded hierarchy yet")?;
        let plane = load_binding_snapshot(&self.db, &petal.verse_id)?;
        let rows = stored_rows(&plane);
        let roles = plane.roles.clone().unwrap_or_default();
        let local_did = self.governance.local_did();
        let local_is_owner = self.owners.is_owner(&petal.verse_id);
        let chain = resolution_scopes(&petal.verse_id, &petal.fractal_id, &petal.petal_id);
        let named = |kind: &str, name: &str| if name.is_empty() { kind.to_string() } else { format!("{kind} {name}") };
        let targets = [
            (
                BindingScopeLevel::Petal,
                named("Petal", &scope.petal_name),
                ScopeTarget {
                    verse_id: petal.verse_id.clone(),
                    fractal_id: Some(petal.fractal_id.clone()),
                    petal_id: Some(petal.petal_id.clone()),
                },
            ),
            (
                BindingScopeLevel::Fractal,
                named("Fractal", &scope.fractal_name),
                ScopeTarget {
                    verse_id: petal.verse_id.clone(),
                    fractal_id: Some(petal.fractal_id.clone()),
                    petal_id: None,
                },
            ),
            (
                BindingScopeLevel::Verse,
                named("Verse", &scope.verse_name),
                ScopeTarget {
                    verse_id: petal.verse_id.clone(),
                    fractal_id: None,
                    petal_id: None,
                },
            ),
        ];
        // Manager+ by subtree authority on the judged role plane (an Owner pin cuts it); without a plane only the
        // verse Owner is offered a scope. The DB plane judges every write again.
        let manages = |scope_string: &str| match &plane.roles {
            Some(roles) => roles
                .authority(local_did, scope_string)
                .is_at_least(RoleLevel::Manager),
            None => local_is_owner,
        };
        let scopes: Vec<ScopeOption> = chain
            .iter()
            .zip(targets.iter())
            .filter(|(scope_string, _)| manages(scope_string))
            .map(|(_, (level, label, target))| ScopeOption {
                level: *level,
                label: label.clone(),
                target: target.clone(),
            })
            .collect();
        let at_scope = |scope_string: &str| {
            chain
                .iter()
                .zip(targets.iter())
                .find(|(candidate, _)| candidate.as_str() == scope_string)
                .map(|(_, (_, label, target))| (label.clone(), target.clone()))
        };
        // Every point the resolver published here, plus every point with a stored row on the chain (a fresh write
        // shows before the worker re-resolves; its state then reads from the next refresh).
        let published = self.view.points_at(&petal.petal_id).unwrap_or_default();
        let mut points: BTreeSet<BindingPoint> = published
            .iter()
            .map(|point| BindingPoint::new(point.hook.clone(), point.key.clone()))
            .collect();
        points.extend(
            rows.iter()
                .filter(|row| chain.contains(&row.record.scope))
                .map(|row| row.record.point()),
        );
        let point_rows = points
            .into_iter()
            .map(|point| {
                let resolution = published
                    .iter()
                    .find(|candidate| candidate.hook == point.hook && candidate.key == point.key)
                    .and_then(|candidate| candidate.resolution.clone());
                let winner =
                    deciding_binding(&rows, &roles, &plane.stored_terrain_petals, &petal, &point).cloned();
                match winner {
                    Some(record) => {
                        let (scope_label, target) = at_scope(&record.scope).unzip();
                        BindingPointRow {
                            hook: point.hook,
                            key: point.key,
                            scope_label: scope_label.unwrap_or_default(),
                            target,
                            mode: record.mode,
                            package_id: record.package_id,
                            version_req: record.version_req,
                            config: record.config,
                            runs_on: record.runs_on,
                            resolution,
                        }
                    }
                    None => BindingPointRow {
                        hook: point.hook,
                        key: point.key,
                        scope_label: String::new(),
                        target: None,
                        mode: BindingMode::Inherit,
                        package_id: None,
                        version_req: None,
                        config: json!({}),
                        runs_on: None,
                        resolution,
                    },
                }
            })
            .collect();
        Ok(BindingsSnapshot {
            verse_id: petal.verse_id.clone(),
            petal_label: named("Petal", &scope.petal_name),
            can_manage: manages(&chain[0]),
            local_is_owner,
            scopes,
            points: point_rows,
            providers: Vec::new(),
        })
    }

    fn write_binding(&self, template: BindingRecord) -> Result<(), String> {
        let verse_id = fe_database::parse_scope(&template.scope)
            .map(|parts| parts.verse_id)
            .map_err(|error| format!("invalid-input: {error}"))?;
        let (reply, receiver) = crossbeam::channel::bounded(1);
        // `caller_did: None` is the GUI: the DB plane judges the local DID (the row's author) at the scope.
        let command = DbCommand::PutBinding {
            caller_did: None,
            binding: template,
            reply,
        };
        self.db
            .send_timeout(command, DB_SEND_TIMEOUT)
            .map_err(|_| "the database is not accepting commands".to_string())?;
        receiver
            .recv_timeout(DB_REPLY_TIMEOUT)
            .map_err(|_| "the database did not answer; the binding may or may not be written".to_string())?
            .map_err(|refusal: BindingRefusal| format!("{}: {}", refusal.code, refusal.message))?;
        self.signals.dirty(&verse_id, Trigger::BindingRow);
        Ok(())
    }

    fn stage_launch(
        &self,
        build: &BuildToRun<'_>,
        scope: &ActiveScope,
    ) -> Result<StagedInstance, String> {
        let petal = self
            .stager
            .petals
            .get(&scope.petal_id)
            .filter(|petal| petal.verse_id == scope.verse_id)
            .ok_or("the open petal is not in the loaded hierarchy yet")?;
        let staged = self.stager.stage(&StageRequest {
            plugin_id: build.installation_id,
            installation_id: build.installation_id,
            package: build.package,
            facts: build.facts,
            petal: &petal,
            first_activation: build.first_activation,
        })?;
        self.signals.dirty(&scope.verse_id, Trigger::Lifecycle);
        Ok(staged)
    }

    fn counters(&self, installation_id: &str) -> Vec<(String, String)> {
        let Some(row) = self.index.row(installation_id) else {
            return Vec::new();
        };
        let totals = tick_totals(&self.stager.ticks, &row.instances);
        let bus = self
            .stager
            .bus
            .counters_for_installation(installation_id)
            .unwrap_or_default();
        let net = self
            .stager
            .net
            .counters_for_installation(installation_id)
            .unwrap_or_default();
        [
            ("Ticks run", totals.ticks_run),
            ("Ticks skipped", totals.ticks_skipped),
            ("Commits", totals.commits),
            ("Refusals", totals.refusals),
            ("Unknown outcomes", totals.unknown_outcomes),
            ("Ingress frames", net.ingress_frames),
            ("Egress requests", net.egress_requests),
            ("Bus events published", bus.published),
            ("Commands answered", bus.commands_ok),
        ]
        .into_iter()
        .map(|(label, value)| (label.to_string(), value.to_string()))
        .collect()
    }

    fn audit(&self, installation_id: &str) -> Vec<AuditLine> {
        self.governance
            .audit_rows(installation_id, DETAILS_AUDIT_LINES)
            .unwrap_or_default()
            .iter()
            .map(|row| {
                let text = |name: &str| {
                    row.get(name)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                let outcome = text("outcome");
                let action = text("action");
                AuditLine {
                    at: text("at"),
                    actor: text("actor_did"),
                    action: if outcome == "ok" {
                        action
                    } else {
                        format!("{action} ({outcome})")
                    },
                }
            })
            .collect()
    }
    /// `(licensee DID, device X25519 did:key)` an acquire link and a token request name (R-BIN-19).
    fn licence_identity(&self) -> Result<(String, String), String> {
        if !cfg!(feature = "entitlement") {
            return Err("not-available: licences need the entitlement feature on this host".into());
        }
        let keys = self
            .licence_keys
            .as_ref()
            .ok_or("not-available: this host has no device key yet; restart once enrolment has created it")?;
        let licensee = keys.licensee();
        Ok((licensee.did(), licensee.device_did()))
    }
    /// Ask the source's issuer through the token client; a verified token is filed where row 6 reads it.
    fn request_licence_token(
        &self,
        request: &crate::extension_host::ops_drain::LicenceRequest<'_>,
    ) -> Result<crate::extension_host::ops_drain::LicenceAnswer, String> {
        if !cfg!(feature = "entitlement") || self.licence_keys.is_none() {
            return Err("not-available: licences need the entitlement feature on this host".into());
        }
        #[cfg(feature = "entitlement")]
        {
            if let (Some(keys), Some(tokens)) = (&self.licence_keys, &self.licence_tokens) {
                return tokens.request(request, keys.licensee(), fe_format::entitlement::unix_now());
            }
        }
        let _ = request;
        Err("not-available: the licence token client could not be built on this host".into())
    }
    /// Re-resolve `verse_id` (every verse when empty) so a changed Install/Update offer shows.
    fn registry_offers_changed(&self, verse_id: &str) {
        if verse_id.is_empty() {
            self.signals.send(BindingSignal::AllDirty(Trigger::Lifecycle));
        } else {
            self.signals.dirty(verse_id, Trigger::Lifecycle);
        }
    }
}

