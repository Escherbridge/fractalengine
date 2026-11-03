use std::{collections::HashMap, sync::Arc};

use fe_runtime::{
    messages::{DbCommand, DbResult},
    scene_asset::SceneAssetAdoptionCandidate,
};

use super::{
    Frame,
    context::{self, Context},
};

const MAX_ACCEPTED: usize = 32;
const READ_SECONDS: f64 = 5.0;
const REVIEW_SECONDS: f64 = 60.0;

#[derive(Clone, Debug)]
pub(crate) struct Preview {
    pub id: String,
    pub candidate: SceneAssetAdoptionCandidate,
    context: Context,
    expires: f64,
}

impl Preview {
    pub(super) fn matches_context(
        &self,
        manager: &crate::verse_manager::VerseManager,
        nav: &crate::navigation_manager::NavigationManager,
        nodes: &crate::node_manager::NodeManager,
        role: &crate::plugin::LocalUserRole,
    ) -> bool {
        self.context.matches(manager, nav, nodes, role)
    }
    pub(super) fn current(&self, frame: &Frame<'_>) -> bool {
        frame.now < self.expires
            && self.matches_context(frame.manager, frame.nav, frame.nodes, frame.role)
    }
}

struct Reading {
    context: Context,
    request_id: String,
    expected: Option<Arc<Preview>>,
    queued_at: f64,
    sent_at: Option<f64>,
}

struct QueuedWrite {
    approved: Arc<Preview>,
    request_id: String,
    candidate: Option<SceneAssetAdoptionCandidate>,
    deadline: f64,
}

struct Accepted {
    approved: Arc<Preview>,
    sent_at: f64,
    unknown_reported: bool,
    retired: bool,
}

#[derive(Default)]
enum Phase {
    #[default]
    Empty,
    Reading(Reading),
    Ready(Arc<Preview>),
    Queued(QueuedWrite),
}

#[derive(Default)]
pub(super) struct Adoption {
    phase: Phase,
    accepted: HashMap<String, Accepted>,
}

impl Adoption {
    pub fn accepted_context(&self, request_id: &str) -> Option<Context> {
        self.accepted
            .get(request_id)
            .map(|owner| owner.approved.context.clone())
    }
    pub fn accepted_targets(&self, request_id: &str) -> Vec<String> {
        self.accepted
            .get(request_id)
            .map_or_else(Vec::new, |owner| {
                owner
                    .approved
                    .candidate
                    .reference_fingerprints
                    .iter()
                    .map(|reference| reference.node_id.clone())
                    .collect()
            })
    }
    pub fn pending(&self) -> usize {
        self.accepted.len()
            + usize::from(matches!(self.phase, Phase::Reading(_) | Phase::Queued(_)))
    }

    pub fn preview(&self) -> Option<&Arc<Preview>> {
        match &self.phase {
            Phase::Ready(preview) => Some(preview),
            Phase::Reading(read) => read.expected.as_ref(),
            Phase::Queued(write) => Some(&write.approved),
            Phase::Empty => None,
        }
    }

    pub fn ready(&self) -> bool {
        matches!(self.phase, Phase::Ready(_))
    }

    pub fn status(&self, node: &str) -> Option<&'static str> {
        self.accepted.values().find(|owner| owner.approved.context.node_id == node).map(|owner| {
            if owner.unknown_reported { "Adoption outcome is unknown; its exact request remains owned and will not be retried" }
            else if owner.retired { "Adoption was accepted in an earlier scene context; waiting for its exact outcome" }
            else { "Adoption was submitted once; waiting for its exact stored outcome" }
        })
    }

    pub fn busy_for(&self, node: &str) -> bool {
        self.accepted
            .values()
            .any(|owner| owner.approved.context.node_id == node)
            || match &self.phase {
                Phase::Reading(read) => read.context.node_id == node,
                Phase::Queued(write) => write.approved.context.node_id == node,
                _ => false,
            }
    }

    pub fn begin(&mut self, node: &str, frame: &Frame<'_>) -> Result<(), String> {
        if self.busy_for(node) {
            return Err(
                "An accepted adoption outcome or review is still pending for this object".into(),
            );
        }
        let context = Context::capture(node, frame.manager, frame.nav, frame.nodes, frame.role)?;
        self.phase = Phase::Reading(Reading {
            context,
            request_id: super::request_id(),
            expected: None,
            queued_at: frame.now,
            sent_at: None,
        });
        Ok(())
    }

    pub fn confirm(&mut self, preview_id: &str, frame: &Frame<'_>) -> Result<(), String> {
        let Phase::Ready(preview) = &self.phase else {
            return Err("Review the local-copy candidate before adopting it".into());
        };
        if preview.id != preview_id || !preview.current(frame) {
            return Err(
                "The reviewed object or context changed; review the local copy again".into(),
            );
        }
        if self.accepted.len() >= MAX_ACCEPTED {
            return Err("Accepted adoption outcomes are full; no adoption was submitted".into());
        }
        let context = preview.context.clone();
        let expected = Some(preview.clone());
        self.phase = Phase::Reading(Reading {
            context,
            request_id: super::request_id(),
            expected,
            queued_at: frame.now,
            sent_at: None,
        });
        Ok(())
    }

    pub fn cancel_review(&mut self) {
        self.phase = Phase::Empty;
    }

    pub fn retire(&mut self) {
        self.phase = Phase::Empty;
        for owner in self.accepted.values_mut() {
            owner.retired = true;
        }
    }

    pub fn route(&mut self, result: &DbResult, frame: &Frame<'_>) -> Option<(String, String)> {
        if let DbResult::SceneAssetAdopted { request_id, result } = result {
            let owner = self.accepted.get(request_id)?;
            let node = owner.approved.context.node_id.clone();
            let message = match result {
                Ok(receipt)
                    if receipt.candidate == owner.approved.candidate
                        && !receipt.operation_id.is_empty()
                        && !receipt.adopter_did.is_empty()
                        && !receipt.adopted_at.is_empty() =>
                {
                    format!(
                        "Local copy adopted for sharing (operation {}). Peer delivery is tracked separately.",
                        receipt.operation_id
                    )
                }
                Ok(_) => {
                    return Some((node, "Adoption reply did not match the accepted candidate. Outcome remains unknown; no write was retried.".into()));
                }
                Err(error) => {
                    format!("Adoption was not confirmed: {error}. No request was retried.")
                }
            };
            self.accepted.remove(request_id);
            return Some((node, message));
        }
        let DbResult::SceneAssetAdoptionCandidate {
            node_id,
            request_id,
            result,
        } = result
        else {
            return None;
        };
        let Phase::Reading(read) = &self.phase else {
            return None;
        };
        if read.request_id != *request_id || read.context.node_id != *node_id {
            return None;
        }
        let node = node_id.clone();
        let Phase::Reading(read) = std::mem::take(&mut self.phase) else {
            unreachable!()
        };
        if read
            .sent_at
            .is_none_or(|sent| frame.now >= sent + READ_SECONDS)
            || !read
                .context
                .matches(frame.manager, frame.nav, frame.nodes, frame.role)
        {
            return Some((
                node,
                "Local-copy review expired or its context changed; no adoption was submitted"
                    .into(),
            ));
        }
        let candidate = match result {
            Ok(candidate) if candidate.node_id == read.context.node_id && candidate.asset_id == read.context.asset_id
                && candidate.scope == read.context.scope && !candidate.asset_fingerprint.is_empty()
                && !candidate.reference_fingerprints.is_empty()
                && candidate.reference_fingerprints.iter().any(|reference| reference.node_id == *node_id) => candidate,
            Ok(_) => return Some((node, "Local-copy review returned a different asset or reference context; no adoption was submitted".into())),
            Err(error) => return Some((node, format!("Local-copy review is unavailable: {error}"))),
        };
        if let Some(approved) = read.expected {
            if approved.candidate == *candidate && approved.current(frame) {
                let id = super::request_id();
                self.phase = Phase::Queued(QueuedWrite {
                    candidate: Some(candidate.clone()),
                    approved,
                    request_id: id,
                    deadline: frame.now + READ_SECONDS,
                });
                return Some((node, "Fresh Owner and reference review matched; adoption is waiting for database submission".into()));
            }
            let preview = Arc::new(Preview {
                id: request_id.clone(),
                candidate: candidate.clone(),
                context: read.context,
                expires: frame.now + REVIEW_SECONDS,
            });
            self.phase = Phase::Ready(preview);
            return Some((node, "The copy or its references changed. Review the updated candidate before confirming again.".into()));
        }
        self.phase = Phase::Ready(Arc::new(Preview {
            id: request_id.clone(),
            candidate: candidate.clone(),
            context: read.context,
            expires: frame.now + REVIEW_SECONDS,
        }));
        Some((
            node,
            "Verified local-copy candidate is ready for review".into(),
        ))
    }

    pub fn pump(
        &mut self,
        frame: &Frame<'_>,
        sender: &crossbeam::channel::Sender<DbCommand>,
    ) -> Option<(String, String)> {
        let phase = std::mem::take(&mut self.phase);
        match phase {
            Phase::Reading(mut read) => {
                let node = read.context.node_id.clone();
                if !read
                    .context
                    .matches(frame.manager, frame.nav, frame.nodes, frame.role)
                    || frame.now >= read.sent_at.unwrap_or(read.queued_at) + READ_SECONDS
                {
                    return Some((
                        node,
                        "Local-copy review timed out or changed; no adoption was submitted".into(),
                    ));
                }
                if read.sent_at.is_none() {
                    match sender.try_send(DbCommand::ReadSceneAssetAdoption {
                        node_id: node.clone(),
                        request_id: read.request_id.clone(),
                    }) {
                        Ok(()) => read.sent_at = Some(frame.now),
                        Err(crossbeam::channel::TrySendError::Full(_)) => {}
                        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                            return Some((
                                node,
                                "Database connection is unavailable; review was not submitted"
                                    .into(),
                            ));
                        }
                    }
                }
                self.phase = Phase::Reading(read);
            }
            Phase::Queued(mut write) => {
                let node = write.approved.context.node_id.clone();
                if frame.now >= write.deadline
                    || !write.approved.current(frame)
                    || !context::owner(frame.role)
                {
                    return Some((node, "Fresh adoption context expired before submission; review again. No write was submitted.".into()));
                }
                let candidate = write
                    .candidate
                    .take()
                    .expect("unsent adoption candidate remains owned");
                let command = DbCommand::AdoptSceneAsset {
                    candidate,
                    request_id: write.request_id.clone(),
                };
                match sender.try_send(command) {
                    Ok(()) => {
                        self.accepted.insert(
                            write.request_id,
                            Accepted {
                                approved: write.approved,
                                sent_at: frame.now,
                                unknown_reported: false,
                                retired: false,
                            },
                        );
                        return Some((
                            node,
                            "Adoption submitted once; waiting for its exact stored outcome".into(),
                        ));
                    }
                    Err(crossbeam::channel::TrySendError::Full(command)) => {
                        let DbCommand::AdoptSceneAsset { candidate, .. } = command else {
                            unreachable!("only the owned adoption was offered")
                        };
                        write.candidate = Some(candidate);
                        self.phase = Phase::Queued(write);
                    }
                    Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                        return Some((
                            node,
                            "Database connection is unavailable; adoption was not submitted".into(),
                        ));
                    }
                }
            }
            Phase::Ready(preview) if preview.current(frame) => self.phase = Phase::Ready(preview),
            Phase::Ready(preview) => {
                return Some((
                    preview.context.node_id.clone(),
                    "Local-copy review context changed or expired; review again".into(),
                ));
            }
            Phase::Empty => {}
        }
        for owner in self.accepted.values_mut() {
            if !owner.approved.current(frame) {
                owner.retired = true;
            }
            if !owner.unknown_reported && frame.now >= owner.sent_at + READ_SECONDS {
                owner.unknown_reported = true;
                return Some((owner.approved.context.node_id.clone(), "Adoption outcome is unknown. Its exact request and candidate remain owned; no write will be retried.".into()));
            }
        }
        None
    }
}
