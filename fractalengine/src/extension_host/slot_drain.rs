//! Drains other packages' slot commands onto the integration bus as `CallerKind::Ui`, with their args (R-HOOKS-33, R-UI-04).
//! Pinned seam for I-BIN: build a [`SlotDrain`] over `Arc<Bus>` (via [`BusSlotCaller`]) and call
//! [`SlotDrain::run`] each frame with the commands taken from `ExtensionSlotState`; [`SlotCommand`] is built from
//! `ExtensionCommandRequest` (I-SLOTS adds its `args`). The terrain package stays with `terrain_commands`.
//! See `src/AGENTS.md` §extension-host.

use std::sync::Arc;

use fe_plugin::bus::{Bus, BusRefusal, Caller, CallerKind, CommandOutcome, CommandTicket};
use fe_terrain::extension::TERRAIN_EXTENSION_ID;
use serde_json::Value;
use tokio::sync::oneshot::error::TryRecvError;

/// Pending calls kept while their answers are awaited; past this the oldest are dropped (the guest still runs them).
const MAX_PENDING: usize = 64;

/// One slot command a user pressed: the contributing package, its command id and the call args.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotCommand {
    pub package_id: String,
    pub command: String,
    /// `Null` when the slot passes none; a context-menu press carries `{"node_id": ..}`.
    pub args: Value,
}

/// How a call ended, for the status line or a notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotReport {
    /// The guest answered.
    Done { package_id: String, command: String },
    /// The bus refused it at admission (stable reason code and text).
    Refused { package_id: String, command: String, code: &'static str, message: String },
    /// The guest refused, failed, or the host ended the call.
    Failed { package_id: String, command: String, message: String },
}

/// What the drain needs from the bus (the real one is [`BusSlotCaller`]).
pub trait SlotCaller {
    /// Admit one local-UI command call at `petal_id`.
    fn call(&self, petal_id: &str, package_id: &str, command: &str, args: &Value) -> Result<CommandTicket, BusRefusal>;
}

/// The host bus as a [`SlotCaller`]: the local user, kind `ui`, no ceiling.
pub struct BusSlotCaller {
    bus: Arc<Bus>,
}

impl BusSlotCaller {
    pub fn new(bus: Arc<Bus>) -> Self {
        Self { bus }
    }
}

impl SlotCaller for BusSlotCaller {
    fn call(&self, petal_id: &str, package_id: &str, command: &str, args: &Value) -> Result<CommandTicket, BusRefusal> {
        let caller = Caller {
            did: self.bus.local_did().to_owned(),
            kind: CallerKind::Ui,
            package_id: None,
            local: true,
            role_ceiling: None,
        };
        self.bus.call_command(&caller, petal_id, package_id, command, args)
    }
}

/// True for commands another dispatcher owns (the terrain package's editor actions).
pub fn is_host_command(package_id: &str) -> bool {
    package_id == TERRAIN_EXTENSION_ID
}

/// Splits taken commands into those this drain sends and those left for another dispatcher.
pub fn split_foreign(commands: Vec<SlotCommand>) -> (Vec<SlotCommand>, Vec<SlotCommand>) {
    commands.into_iter().partition(|command| !is_host_command(&command.package_id))
}

struct Pending {
    package_id: String,
    command: String,
    ticket: CommandTicket,
}

/// Sends slot commands to the bus and collects their outcomes without blocking the frame.
pub struct SlotDrain<C: SlotCaller> {
    caller: C,
    pending: Vec<Pending>,
}

impl<C: SlotCaller> SlotDrain<C> {
    pub fn new(caller: C) -> Self {
        Self { caller, pending: Vec::new() }
    }

    /// Calls still waiting for a guest answer.
    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// Send `commands` at `petal_id` (`None`: no active petal, each is reported refused) and return every call that
    /// finished since the last run, including admission refusals.
    pub fn run(&mut self, petal_id: Option<&str>, commands: Vec<SlotCommand>) -> Vec<SlotReport> {
        let mut reports = Vec::new();
        for command in commands {
            let SlotCommand { package_id, command, args } = command;
            let Some(petal_id) = petal_id else {
                reports.push(SlotReport::Refused {
                    package_id,
                    command,
                    code: "not-available",
                    message: "open a petal first".into(),
                });
                continue;
            };
            match self.caller.call(petal_id, &package_id, &command, &args) {
                Ok(ticket) => self.pending.push(Pending { package_id, command, ticket }),
                Err(refusal) => {
                    tracing::debug!(%package_id, %command, code = refusal.code(), "slot command refused");
                    reports.push(SlotReport::Refused { package_id, command, code: refusal.code(), message: format!("{refusal:?}") });
                }
            }
        }
        if self.pending.len() > MAX_PENDING {
            let excess = self.pending.len() - MAX_PENDING;
            self.pending.drain(..excess);
        }
        let mut waiting = Vec::with_capacity(self.pending.len());
        for mut pending in self.pending.drain(..) {
            match pending.ticket.reply.try_recv() {
                Ok(CommandOutcome::Ok(_)) => reports.push(SlotReport::Done { package_id: pending.package_id, command: pending.command }),
                Ok(CommandOutcome::Guest(error)) => reports.push(SlotReport::Failed {
                    package_id: pending.package_id,
                    command: pending.command,
                    message: format!("{error:?}"),
                }),
                Ok(CommandOutcome::Host(error)) => reports.push(SlotReport::Failed {
                    package_id: pending.package_id,
                    command: pending.command,
                    message: format!("{error:?}"),
                }),
                Err(TryRecvError::Empty) => waiting.push(pending),
                Err(TryRecvError::Closed) => reports.push(SlotReport::Failed {
                    package_id: pending.package_id,
                    command: pending.command,
                    message: "the extension stopped before answering".into(),
                }),
            }
        }
        self.pending = waiting;
        reports
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;
    use tokio::sync::oneshot;

    /// Records every admitted call and keeps the reply senders so the test answers them later.
    struct FakeCaller {
        seen: RefCell<Vec<(String, String, String, Value)>>,
        replies: RefCell<Vec<oneshot::Sender<CommandOutcome>>>,
    }

    impl SlotCaller for &FakeCaller {
        fn call(&self, petal_id: &str, package_id: &str, command: &str, args: &Value) -> Result<CommandTicket, BusRefusal> {
            if command == "forbidden" {
                return Err(BusRefusal::CapabilityDenied("not declared".into()));
            }
            self.seen.borrow_mut().push((petal_id.into(), package_id.into(), command.into(), args.clone()));
            let (reply, receiver) = oneshot::channel();
            self.replies.borrow_mut().push(reply);
            Ok(CommandTicket { call: 1, reply: receiver })
        }
    }

    fn press(package: &str, command: &str, args: Value) -> SlotCommand {
        SlotCommand { package_id: package.into(), command: command.into(), args }
    }

    /// Journey: terrain stays out, a context-menu press carries its node, refusal and answer both surface.
    #[test]
    fn slot_drain_journey_args_reach_the_bus_and_outcomes_come_back() {
        let (mine, terrain) = split_foreign(vec![
            press("org.example.fleet", "start", json!({ "node_id": "n1" })),
            press(TERRAIN_EXTENSION_ID, "terrain-toggle", Value::Null),
            press("org.example.fleet", "forbidden", Value::Null),
        ]);
        assert_eq!(terrain.len(), 1, "the terrain dispatcher keeps its own commands");

        let fake = FakeCaller { seen: RefCell::new(Vec::new()), replies: RefCell::new(Vec::new()) };
        let mut drain = SlotDrain::new(&fake);
        let first = drain.run(Some("p1"), mine);
        assert_eq!(first.len(), 1, "the refused call reports at once");
        assert!(matches!(&first[0], SlotReport::Refused { code: "capability-denied", .. }));
        assert_eq!(fake.seen.borrow()[0], ("p1".into(), "org.example.fleet".into(), "start".into(), json!({ "node_id": "n1" })));
        assert_eq!(drain.in_flight(), 1);

        assert!(drain.run(Some("p1"), Vec::new()).is_empty(), "no answer yet: nothing reported");
        let _ = fake.replies.borrow_mut().remove(0).send(CommandOutcome::Ok("{}".into()));
        let done = drain.run(Some("p1"), Vec::new());
        assert_eq!(done, vec![SlotReport::Done { package_id: "org.example.fleet".into(), command: "start".into() }]);
        assert_eq!(drain.in_flight(), 0);

        let none = drain.run(None, vec![press("org.example.fleet", "start", Value::Null)]);
        assert!(matches!(&none[0], SlotReport::Refused { code: "not-available", .. }), "no active petal: refused, never sent");
    }
}
