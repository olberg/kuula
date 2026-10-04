//! The real-Iroh adapter check behind `net_sim --loopback`.

use kuula_core::net::{Command, Event, Transport};
use kuula_core::{FrameInput, Snapshot};
use kuula_host_headless::net_sim::{self, Config, Guest, Run};
use kuula_net::IrohTransport;
use std::time::{Duration, Instant};

pub fn simulate(
    guest: &Guest,
    snapshots: [Snapshot; 2],
    config: Config,
    inputs: [Vec<FrameInput>; 2],
) -> Result<Run, String> {
    let (host_transport, ticket) = listen()?;
    let join_transport = IrohTransport::new(Some("127.0.0.1:0".parse().unwrap()));
    let start = Instant::now();
    net_sim::simulate_with(
        guest,
        snapshots,
        config,
        inputs,
        [Box::new(host_transport), Box::new(join_transport)],
        ticket,
        "iroh-loopback",
        |frame| {
            let due = start + Duration::from_nanos((frame - 1) * (1_000_000_000 / 60));
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        },
    )
}

/// Reserve the listener's ticket before booting the joining cart. Its first
/// Host request consumes this reservation and re-emits Hosting at a frame.
struct ReservedHost {
    inner: IrohTransport,
    ticket: Option<String>,
    hosting: Option<String>,
}

impl Transport for ReservedHost {
    fn push(&mut self, command: Command) {
        if matches!(command, Command::Host) {
            if let Some(ticket) = self.ticket.take() {
                self.hosting = Some(ticket);
                return;
            }
        }
        self.inner.push(command);
    }
    fn poll(&mut self, out: &mut Vec<Event>, max: usize) {
        if max == 0 {
            return;
        }
        let mut room = max;
        if let Some(ticket) = self.hosting.take() {
            out.push(Event::Hosting { ticket });
            room -= 1;
        }
        self.inner.poll(out, room);
    }
}

fn listen() -> Result<(ReservedHost, String), String> {
    let mut inner = IrohTransport::new(Some("127.0.0.1:0".parse().unwrap()));
    inner.push(Command::Host);
    let start = Instant::now();
    loop {
        let mut events = Vec::new();
        inner.poll(&mut events, 64);
        for event in events {
            match event {
                Event::Hosting { ticket } => {
                    return Ok((
                        ReservedHost {
                            inner,
                            ticket: Some(ticket.clone()),
                            hosting: None,
                        },
                        ticket,
                    ))
                }
                Event::Failed { code, detail } => {
                    return Err(format!("{}: {detail}", code.as_str()))
                }
                _ => {}
            }
        }
        if start.elapsed() > Duration::from_secs(10) {
            return Err("loopback listener timed out".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
