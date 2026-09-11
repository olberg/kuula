//! The console's networking half: admission of the host's events at
//! the top of a cart step, the frame's commands out after it, and the
//! `Leave` a torn-down cart owes its peer. Kept beside `console.rs` so
//! that file stays under its limit; nothing here is reachable from a
//! cart except through `DrawState.net`.

use super::Console;
use crate::input::FrameInput;
use crate::net::{Command, Event, Link, NetEnv, NetState, MAX_BATCH};

impl Console {
    /// Queue the host's events for admission. They are admitted, at
    /// most [`MAX_BATCH`] per step, when the cart actually steps; a
    /// paused cart keeps them waiting, and the host is expected to
    /// offer no more than [`Console::net_room`] at a time.
    pub(super) fn offer_net_events(&mut self, events: Vec<Event>) {
        if events.is_empty() {
            return;
        }
        if self.cart.state.net.is_none() {
            // A cart without the service can be offered nothing; a host
            // that does so is confused, and the events go nowhere.
            return;
        }
        self.cart.net_pending.extend(events);
    }

    /// How many events the host may offer now: the inbox's room less
    /// what is already waiting, never more than the batch bound.
    pub fn net_room(&self) -> usize {
        match &self.cart.state.net {
            Some(net) => net
                .room()
                .saturating_sub(self.cart.net_pending.len())
                .min(MAX_BATCH),
            None => 0,
        }
    }

    /// The commands the cart issued in the last step, or a single
    /// `Leave` when the cart was torn down (fault, quit, restart or a
    /// new cart) with a session live. Drained. The torn-down cart's
    /// own commands go with it: a faulted cart does not step again, so
    /// what its last frame queued would otherwise be drained on the
    /// next host tick and build a transport for a stopped cart.
    pub fn take_net_commands(&mut self) -> Vec<Command> {
        if self.cart.net_teardown {
            self.cart.net_teardown = false;
            if let Some(net) = self.cart.state.net.as_mut() {
                net.outbox.clear();
            }
            return vec![Command::Leave];
        }
        match &mut self.cart.state.net {
            Some(net) => std::mem::take(&mut net.outbox),
            None => Vec::new(),
        }
    }

    /// Set the initial environment for carts that declare the service:
    /// the permission of the run and the invite ticket. Applies to the
    /// current cart if it has not started, and to every cart the shell
    /// loads afterwards.
    pub fn set_net_env(&mut self, env: NetEnv) {
        if self.cart.frame == 0 {
            if let Some(net) = self.cart.state.net.as_mut() {
                net.permitted = env.permitted;
                net.invite = env.invite.clone();
            }
        }
        self.net_env = env;
    }

    /// The cart's networking state, if it declares the service.
    pub fn net_state(&self) -> Option<&NetState> {
        self.cart.state.net.as_ref()
    }

    /// One frame through a [`Link`]: poll it for what fits, step, hand
    /// the commands back. What every host that runs a networked cart
    /// does.
    pub fn step_linked(&mut self, link: &mut Link, input: FrameInput) {
        let mut events = Vec::new();
        link.poll(&mut events, self.net_room());
        self.step_with(input, events);
        let commands = self.take_net_commands();
        if !commands.is_empty() {
            link.push(commands);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::console::{CartOpener, Guest, GuestFactory, Preload};
    use crate::draw::DrawState;
    use crate::fault::Fault;
    use crate::input::FrameInput;
    use crate::net::{
        Command, Event, FailCode, Link, MemoryTransport, NetEnv, Reason, Status, Transport,
        MEMORY_TICKET,
    };
    use crate::save::{MemoryStore, SaveStore};
    use crate::shell::{Settings, SysRequest, SysState};
    use crate::snapshot::{Snapshot, SnapshotLimits};
    use crate::source::CartSource;
    use crate::Console;

    const NET_MANIFEST: &[u8] = b"[cart]\nservices = [\"net\"]\n";

    fn cart(entries: Vec<(&str, &[u8])>) -> Rc<dyn CartSource> {
        Rc::new(
            Snapshot::from_entries(
                entries.into_iter().map(|(k, v)| (k, v.to_vec())),
                SnapshotLimits::default(),
            )
            .unwrap(),
        )
    }

    /// A guest scripted by frame: hosts on 1, sends on every frame with
    /// a session, drains its inbox into the log, faults on `fail_on`.
    struct Scripted {
        fail_on: Option<u64>,
        join: bool,
    }

    impl Guest for Scripted {
        fn step(&mut self, state: &mut DrawState, _: FrameInput, frame: u64) -> Result<(), Fault> {
            let net = state.net.as_mut().expect("the cart declares net");
            while let Some(e) = net.recv() {
                state.log.push(format!("{frame}:{}", e.kind()));
            }
            if frame == 1 {
                if self.join {
                    net.join(MEMORY_TICKET.into());
                } else {
                    net.host();
                }
            }
            if net.status == Status::Connected {
                net.send(vec![frame as u8]);
            }
            if self.fail_on == Some(frame) {
                return Err(Fault::new(
                    Fault::RUNTIME_ERROR,
                    "main.lua",
                    Some(1),
                    "boom",
                ));
            }
            Ok(())
        }
    }

    fn permitted() -> NetEnv {
        NetEnv {
            permitted: true,
            invite: None,
        }
    }

    struct Plain;
    impl Guest for Plain {
        fn step(&mut self, state: &mut DrawState, _: FrameInput, _: u64) -> Result<(), Fault> {
            assert!(state.net.is_none(), "a plain cart has no net state");
            Ok(())
        }
    }

    #[test]
    fn a_plain_cart_has_no_net_state_and_room_zero() {
        let c = Console::new(cart(vec![("main.lua", b"")]), |_, _| {
            Ok(Box::new(Plain) as Box<dyn Guest>)
        });
        assert!(c.net_state().is_none());
        assert_eq!(c.net_room(), 0);
        let mut c = c;
        c.step_with(FrameInput::NONE, vec![Event::Permission { granted: true }]);
        assert!(c.take_net_commands().is_empty());
    }

    #[test]
    fn two_consoles_over_the_memory_pair_exchange_messages() {
        let (a, b) = MemoryTransport::pair();
        let src = cart(vec![("main.lua", b""), ("cart.toml", NET_MANIFEST)]);
        let mut host = Console::new(src.clone(), |_, _| {
            Ok(Box::new(Scripted {
                fail_on: None,
                join: false,
            }) as Box<dyn Guest>)
        });
        let mut joiner = Console::new(src, |_, _| {
            Ok(Box::new(Scripted {
                fail_on: None,
                join: true,
            }) as Box<dyn Guest>)
        });
        host.set_net_env(permitted());
        joiner.set_net_env(permitted());
        let mut la = Link::over(Box::new(a));
        let mut lb = Link::over(Box::new(b));
        let mut log = Vec::new();
        for _ in 0..6 {
            host.step_linked(&mut la, FrameInput::NONE);
            log.extend(host.output().log.iter().map(|l| format!("h{l}")));
            joiner.step_linked(&mut lb, FrameInput::NONE);
            log.extend(joiner.output().log.iter().map(|l| format!("j{l}")));
        }
        // The memory pair delivers within the frame, so the host sees
        // its ticket and the joiner together on frame 2.
        assert_eq!(
            log,
            [
                "h2:hosting",
                "h2:connected",
                "j2:connected",
                "j2:message",
                "h3:message",
                "j3:message",
                "h4:message",
                "j4:message",
                "h5:message",
                "j5:message",
                "h6:message",
                "j6:message",
            ]
        );
        let ns = host.net_state().unwrap();
        assert_eq!(ns.status, Status::Connected);
        assert_eq!(ns.ticket.as_deref(), Some(MEMORY_TICKET));
        assert_eq!(ns.sent, 5);
        assert_eq!(ns.received, 4);
    }

    #[test]
    fn a_fault_with_a_session_open_says_bye_once() {
        let (a, mut b) = MemoryTransport::pair();
        let src = cart(vec![("main.lua", b""), ("cart.toml", NET_MANIFEST)]);
        let mut host = Console::new(src, |_, _| {
            Ok(Box::new(Scripted {
                fail_on: Some(3),
                join: false,
            }) as Box<dyn Guest>)
        });
        host.set_net_env(permitted());
        let mut la = Link::over(Box::new(a));
        host.step_linked(&mut la, FrameInput::NONE);
        host.step_linked(&mut la, FrameInput::NONE);
        b.push(Command::Join {
            ticket: MEMORY_TICKET.into(),
        });
        host.step_linked(&mut la, FrameInput::NONE);
        assert!(host.state().fault().is_some());
        assert!(!la.is_live(), "the link closed on the fault");
        let mut out = Vec::new();
        b.poll(&mut out, 64);
        assert_eq!(
            out,
            [
                Event::Connected { peer: 1 },
                Event::Disconnected {
                    reason: Reason::Left
                }
            ]
        );
        // The faulting frame's sends were discarded: the only command
        // after the fault was the leave, and nothing follows.
        host.step_linked(&mut la, FrameInput::NONE);
        assert!(host.take_net_commands().is_empty());
    }

    #[test]
    fn a_cart_that_hosts_and_faults_in_one_frame_leaves_nothing_behind() {
        // `host()` then the fault, in frame 1: the teardown's `Leave`
        // is the only command, and the queued `Host` is gone with the
        // cart, so a second host tick after the fault drains nothing.
        let src = cart(vec![("main.lua", b""), ("cart.toml", NET_MANIFEST)]);
        let mut c = Console::new(src, |_, _| {
            Ok(Box::new(Scripted {
                fail_on: Some(1),
                join: false,
            }) as Box<dyn Guest>)
        });
        c.set_net_env(permitted());
        c.step_with(FrameInput::NONE, Vec::new());
        assert!(c.state().fault().is_some());
        assert_eq!(c.take_net_commands(), [Command::Leave]);
        c.step_with(FrameInput::NONE, Vec::new());
        assert!(c.take_net_commands().is_empty());
    }

    #[test]
    fn events_wait_while_paused_and_a_restart_leaves() {
        struct StubShell;
        impl Guest for StubShell {
            fn step(
                &mut self,
                state: &mut DrawState,
                _: FrameInput,
                frame: u64,
            ) -> Result<(), Fault> {
                let sys = state.sys.as_mut().expect("the shell has sys");
                match frame {
                    1 => sys.request(SysRequest::Run("net".into())),
                    4 => sys.request(SysRequest::Paused(true)),
                    7 => sys.request(SysRequest::Paused(false)),
                    9 => sys.request(SysRequest::Restart),
                    _ => {}
                }
                Ok(())
            }
        }
        let source = cart(vec![("main.lua", b""), ("cart.toml", NET_MANIFEST)]);
        let opener: CartOpener = Rc::new(move |_: &str| {
            Ok((
                source.clone(),
                Box::new(MemoryStore::new()) as Box<dyn SaveStore>,
            ))
        });
        let factory: GuestFactory = Rc::new(|_: &str, _: &str| {
            Ok(Box::new(Scripted {
                fail_on: None,
                join: false,
            }) as Box<dyn Guest>)
        });
        let mut c = Console::with_shell(
            Box::new(StubShell),
            Vec::new(),
            Settings::default(),
            opener,
            factory,
            Preload::Decode,
        );
        c.set_net_env(permitted());
        // Every host builds a fresh pair; the peer sides are kept so the
        // test can drive them.
        let peers: Rc<std::cell::RefCell<Vec<MemoryTransport>>> = Default::default();
        let stash = peers.clone();
        let mut link = Link::new(Box::new(move || {
            let (a, b) = MemoryTransport::pair();
            stash.borrow_mut().push(b);
            Box::new(a)
        }));
        link.set_permitted(true);
        let mut seen = Vec::new();
        for shell_frame in 1..=10 {
            if shell_frame == 3 {
                peers.borrow_mut()[0].push(Command::Join {
                    ticket: MEMORY_TICKET.into(),
                });
            }
            if shell_frame == 5 {
                // Offered while paused: waits, and the room shrinks.
                peers.borrow_mut()[0].push(Command::Send { data: vec![9] });
            }
            c.step_linked(&mut link, FrameInput::NONE);
            seen.extend(c.output().log.iter().map(|l| format!("{shell_frame}/{l}")));
            if shell_frame == 6 {
                assert!(c.is_paused());
                assert_eq!(c.net_room(), 63, "one event waits: {seen:?}");
            }
        }
        assert!(
            seen.contains(&"8/4:message".to_string()),
            "the waiting message was admitted on resume: {seen:?}"
        );
        // The restart replaced the cart: its peer heard bye, and the
        // fresh cart hosts anew on its own frame 1, over a new transport.
        assert_eq!(link.constructions(), 2);
        let mut out = Vec::new();
        peers.borrow_mut()[0].poll(&mut out, 64);
        assert!(
            out.contains(&Event::Disconnected {
                reason: Reason::Left
            }),
            "{out:?}"
        );
        let state = c.net_state().unwrap();
        assert_eq!(state.status, Status::Hosting, "{state:?}");
        assert!(state.permitted, "the shell's carts inherit the env");
        let _ = SysState::new(Vec::new(), Settings::default());
    }

    #[test]
    fn without_permission_host_is_denied_and_no_transport_is_built() {
        let src = cart(vec![("main.lua", b""), ("cart.toml", NET_MANIFEST)]);
        let mut c = Console::new(src, |_, _| {
            Ok(Box::new(Scripted {
                fail_on: None,
                join: false,
            }) as Box<dyn Guest>)
        });
        let (a, _b) = MemoryTransport::pair();
        let mut link = Link::over(Box::new(a));
        link.set_permitted(false);
        c.step_linked(&mut link, FrameInput::NONE);
        // Frame 1 drained the permission event the link sent when it was
        // switched off, then had its host denied locally.
        assert_eq!(c.output().log.to_vec(), ["1:permission"]);
        c.step_linked(&mut link, FrameInput::NONE);
        assert_eq!(link.constructions(), 0);
        assert_eq!(c.output().log.to_vec(), ["2:failed"]);
        let state = c.net_state().unwrap();
        assert_eq!(state.status, Status::Off);
        assert!(state.inbox.front().is_none());
        let _ = FailCode::Denied;
    }
}
