//! In-process transport and deterministic delivery queues.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use super::{Command, Event, FailCode, Reason, Transport, INBOX_CAP};

/// The ticket the memory pair's host hands out.
pub const MEMORY_TICKET: &str = "memory:pair";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SideStatus {
    Off,
    Hosting,
    Connected,
}

#[derive(Default)]
struct Side {
    status: Option<SideStatus>,
    /// Whether this side called `host`: a host goes back to listening
    /// when its peer leaves, a joiner goes back to off.
    host: bool,
    /// Events for this side, each with the poll count it becomes
    /// deliverable at.
    queue: VecDeque<(u64, Event)>,
    /// How many of the queued events are messages, the count the
    /// capacity applies to.
    messages: usize,
    polls: u64,
    alive: bool,
}

impl Side {
    /// The status after the peer of a connected side goes away.
    fn after_peer(&self) -> SideStatus {
        if self.host {
            SideStatus::Hosting
        } else {
            SideStatus::Off
        }
    }

    fn clear(&mut self) {
        self.queue.clear();
        self.messages = 0;
    }
}

#[derive(Default)]
struct Hub {
    sides: [Side; 2],
    /// Polls an event waits before it is deliverable.
    delay: u64,
    /// Queued messages at which a send is refused as full.
    capacity: usize,
}

/// Two transports joined in process. `Host` on either side yields
/// `Hosting` with [`MEMORY_TICKET`]; `Join` with that ticket connects
/// them; `Send` delivers to the other side; `Leave` tells the other side
/// `Left`; dropping a side tells the other `Lost`. Knobs: a delivery
/// delay in polls and a queue capacity, so a harness can stall and
/// pressure a cart.
pub struct MemoryTransport {
    hub: Rc<RefCell<Hub>>,
    me: usize,
}

impl MemoryTransport {
    pub fn pair() -> (MemoryTransport, MemoryTransport) {
        let hub = Rc::new(RefCell::new(Hub {
            sides: [
                Side {
                    status: Some(SideStatus::Off),
                    alive: true,
                    ..Side::default()
                },
                Side {
                    status: Some(SideStatus::Off),
                    alive: true,
                    ..Side::default()
                },
            ],
            delay: 0,
            capacity: INBOX_CAP,
        }));
        (
            MemoryTransport {
                hub: hub.clone(),
                me: 0,
            },
            MemoryTransport { hub, me: 1 },
        )
    }

    /// Polls an event waits before it is delivered (default 0).
    pub fn set_delay(&self, polls: u64) {
        self.hub.borrow_mut().delay = polls;
    }

    /// Queued messages (control events do not count) at which the
    /// other side's send is refused as `net_queue_full` (default
    /// [`INBOX_CAP`], which also caps the queue as a whole).
    pub fn set_capacity(&self, n: usize) {
        self.hub.borrow_mut().capacity = n;
    }

    fn other(&self) -> usize {
        1 - self.me
    }
}

impl Hub {
    fn deliver(&mut self, to: usize, e: Event) {
        // A stalled receiver cannot accumulate unlimited failed-send reports.
        // Preserve terminal transitions by discarding obsolete queued traffic.
        if self.sides[to].queue.len() >= INBOX_CAP {
            if matches!(e, Event::Disconnected { .. }) {
                self.sides[to].clear();
            } else {
                return;
            }
        }
        if matches!(e, Event::Message { .. }) {
            self.sides[to].messages += 1;
        }
        let due = self.sides[to].polls + self.delay;
        self.sides[to].queue.push_back((due, e));
    }
}

impl Transport for MemoryTransport {
    fn push(&mut self, cmd: Command) {
        let (me, other) = (self.me, self.other());
        let mut hub = self.hub.borrow_mut();
        let status = hub.sides[me].status.unwrap_or(SideStatus::Off);
        match cmd {
            Command::Host => {
                if status != SideStatus::Off {
                    hub.deliver(me, Event::failed(FailCode::Declined, "already hosting"));
                } else {
                    hub.sides[me].status = Some(SideStatus::Hosting);
                    hub.sides[me].host = true;
                    hub.deliver(
                        me,
                        Event::Hosting {
                            ticket: MEMORY_TICKET.to_string(),
                        },
                    );
                }
            }
            Command::Join { ticket } => {
                if ticket != MEMORY_TICKET {
                    hub.deliver(me, Event::failed(FailCode::Ticket, "not a memory ticket"));
                } else if !hub.sides[other].alive
                    || hub.sides[other].status != Some(SideStatus::Hosting)
                {
                    hub.deliver(me, Event::failed(FailCode::Connect, "no host is listening"));
                } else {
                    hub.sides[me].status = Some(SideStatus::Connected);
                    hub.sides[other].status = Some(SideStatus::Connected);
                    hub.deliver(me, Event::Connected { peer: 1 });
                    hub.deliver(other, Event::Connected { peer: 1 });
                }
            }
            Command::Send { data } => {
                if status != SideStatus::Connected {
                    hub.deliver(me, Event::failed(FailCode::Declined, "no session"));
                } else if hub.sides[other].queue.len() >= INBOX_CAP
                    || hub.sides[other].messages >= hub.capacity
                {
                    hub.deliver(
                        me,
                        Event::failed(FailCode::QueueFull, "the peer's queue is full"),
                    );
                } else {
                    hub.deliver(other, Event::Message { from: 1, data });
                }
            }
            Command::Leave => {
                if status == SideStatus::Connected {
                    hub.deliver(
                        other,
                        Event::Disconnected {
                            reason: Reason::Left,
                        },
                    );
                    // A host keeps listening after its peer leaves.
                    hub.sides[other].status = Some(hub.sides[other].after_peer());
                }
                hub.sides[me].status = Some(SideStatus::Off);
                hub.sides[me].host = false;
                hub.sides[me].clear();
            }
        }
    }

    fn poll(&mut self, out: &mut Vec<Event>, max: usize) {
        let mut hub = self.hub.borrow_mut();
        let side = &mut hub.sides[self.me];
        side.polls += 1;
        let mut n = 0;
        while n < max {
            match side.queue.front() {
                Some((due, _)) if *due < side.polls => {
                    let (_, e) = side.queue.pop_front().expect("checked");
                    if matches!(e, Event::Message { .. }) {
                        side.messages -= 1;
                    }
                    out.push(e);
                    n += 1;
                }
                _ => break,
            }
        }
    }
}

impl Drop for MemoryTransport {
    fn drop(&mut self) {
        let (me, other) = (self.me, self.other());
        let mut hub = self.hub.borrow_mut();
        hub.sides[me].alive = false;
        if hub.sides[me].status == Some(SideStatus::Connected) {
            hub.sides[other].status = Some(hub.sides[other].after_peer());
            hub.deliver(
                other,
                Event::Disconnected {
                    reason: Reason::Lost,
                },
            );
        }
        hub.sides[me].status = Some(SideStatus::Off);
    }
}
