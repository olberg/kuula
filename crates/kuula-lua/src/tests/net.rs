//! The `net` table through the memory transport: every event kind in
//! order, the bounds, the prices and the denial.

use std::rc::Rc;

use kuula_core::net::{Command, Event, Link, MemoryTransport, NetEnv, Transport, MEMORY_TICKET};
use kuula_core::{Console, FrameInput};

use super::cart;
use crate::api::Price;
use crate::LuaGuest;

const MANIFEST: &[u8] = b"[cart]\nservices = [\"net\"]\n";

fn net_console(src: &str, permitted: bool) -> Console {
    let mut c = Console::new(
        cart(&[("main.lua", src.as_bytes()), ("cart.toml", MANIFEST)]),
        LuaGuest::factory,
    );
    c.set_net_env(NetEnv {
        permitted,
        invite: None,
    });
    c
}

/// A cart that drains its inbox into the log every frame as
/// `<frame>:<kind>[:<field>]`, and runs `body` (a Lua chunk) once on the
/// frame it names.
fn logger(body: &str) -> String {
    format!(
        "\
function drain()
  while true do
    local e = net.recv()
    if not e then break end
    local extra = e.ticket or e.data or e.reason or e.code or (e.peer and ('p' .. e.peer))
    if e.granted ~= nil then extra = tostring(e.granted) end
    print(stat('frame') .. ':' .. e.kind .. (extra and (':' .. extra) or ''))
  end
end
function _update(dt)
  drain()
  {body}
end
"
    )
}

fn logs(c: &Console) -> Vec<String> {
    c.output().log.to_vec()
}

#[test]
fn every_event_kind_reaches_the_cart_in_order() {
    let (a, b) = MemoryTransport::pair();
    let mut host = net_console(
        &logger(
            "if stat('frame') == 2 then net.host() end
  if net.status() == 'connected' and stat('frame') < 6 then net.send('h' .. stat('frame')) end
  if stat('frame') == 8 then net.leave() end",
        ),
        true,
    );
    let mut joiner = net_console(
        &logger(
            "if stat('frame') == 3 then net.join(net.invite() or 'memory:pair') end
  if stat('frame') == 4 then print('peers ' .. #net.peers() .. ' ticket ' .. tostring(net.ticket())) end",
        ),
        true,
    );
    let mut la = Link::over(Box::new(a));
    let mut lb = Link::over(Box::new(b));
    let mut all = Vec::new();
    for _ in 0..10 {
        host.step_linked(&mut la, FrameInput::NONE);
        all.extend(logs(&host).into_iter().map(|l| format!("h{l}")));
        joiner.step_linked(&mut lb, FrameInput::NONE);
        all.extend(logs(&joiner).into_iter().map(|l| format!("j{l}")));
    }
    assert_eq!(host.state().fault(), None, "{:?}", host.state());
    assert_eq!(joiner.state().fault(), None, "{:?}", joiner.state());
    // The memory pair delivers within the frame: the host's send on
    // frame 4 is in the joiner's frame 4 batch.
    assert_eq!(
        all,
        [
            "h3:hosting:memory:pair",
            "h4:connected:p1",
            "j4:connected:p1",
            "j4:message:h4",
            "jpeers 1 ticket nil",
            "j5:message:h5",
            "j8:disconnected:left",
            "h9:disconnected:closed",
        ]
    );
    let ns = host.net_state().unwrap();
    assert_eq!(ns.status.as_str(), "ended");
    assert_eq!((ns.sent, ns.received), (2, 0));
    assert_eq!(joiner.net_state().unwrap().received, 2);
}

#[test]
fn failed_and_permission_events_and_the_denial() {
    let src = logger(
        "if stat('frame') == 2 then net.join('not a ticket') end
  if stat('frame') == 5 then net.host() end
  if stat('frame') == 6 then print('status ' .. net.status() .. ' inbox ' .. stat('net_inbox')) end",
    );
    let (a, _b) = MemoryTransport::pair();
    let mut c = net_console(&src, true);
    let mut link = Link::over(Box::new(a));
    let mut all = Vec::new();
    for frame in 1..=7 {
        if frame == 4 {
            link.set_permitted(false);
        }
        c.step_linked(&mut link, FrameInput::NONE);
        all.extend(logs(&c));
    }
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    assert_eq!(
        all,
        [
            "3:failed:net_ticket",
            "4:permission:false",
            "6:failed:net_denied",
            "status ended inbox 0",
        ]
    );
    assert_eq!(link.constructions(), 1, "the bad join built the transport");

    // Never permitted: denied at once, status stays off, nothing built.
    let mut c = net_console(
        &logger(
            "if stat('frame') == 2 then net.host() end
  if stat('frame') == 3 then print(net.status()) end",
        ),
        false,
    );
    let (a, _b) = MemoryTransport::pair();
    let mut link = Link::over(Box::new(a));
    link.set_permitted(false);
    let mut all = Vec::new();
    for _ in 0..3 {
        c.step_linked(&mut link, FrameInput::NONE);
        all.extend(logs(&c));
    }
    // Frame 2 (the first `_update`) drained the permission event the
    // link sent when it was switched off.
    assert_eq!(all, ["2:permission:false", "3:failed:net_denied", "off"]);
    assert_eq!(link.constructions(), 0);
}

#[test]
fn bounds_are_lua_errors_or_false_and_recv_runs_dry() {
    let src = logger(
        "if stat('frame') == 2 then net.host() end
  if stat('frame') == 4 then
    print(select(2, pcall(net.send, '')))
    print(select(2, pcall(net.send, string.rep('x', 1025))))
    print(select(2, pcall(net.join, string.rep('t', 1025))))
    local ok = 0
    for i = 1, 20 do if net.send('m') then ok = ok + 1 end end
    print('sent ' .. ok .. ' dropped ' .. stat('net_dropped') .. ' total ' .. stat('net_sent'))
    print(tostring(net.recv()))
  end",
    );
    let (a, mut b) = MemoryTransport::pair();
    let mut c = net_console(&src, true);
    let mut link = Link::over(Box::new(a));
    for frame in 1..=4 {
        if frame == 3 {
            b.push(Command::Join {
                ticket: MEMORY_TICKET.into(),
            });
        }
        c.step_linked(&mut link, FrameInput::NONE);
    }
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = logs(&c);
    assert!(log[0].contains("net_data: data of 0 bytes"), "{}", log[0]);
    assert!(
        log[1].contains("net_data: data of 1025 bytes"),
        "{}",
        log[1]
    );
    assert!(
        log[2].contains("net_ticket: ticket of 1025 bytes"),
        "{}",
        log[2]
    );
    assert_eq!(log[3], "sent 16 dropped 4 total 16");
    assert_eq!(log[4], "nil");
    let mut out = Vec::new();
    b.poll(&mut out, 64);
    let messages = out
        .iter()
        .filter(|e| matches!(e, Event::Message { .. }))
        .count();
    assert_eq!(messages, 16, "the outbox never exceeds 16 sends: {out:?}");
}

#[test]
fn send_and_recv_charge_the_documented_formulas() {
    // No drain here: the inbox is read by hand so each `recv` is
    // measured on a known event.
    let src = "\
function _update(dt)
  if stat('frame') == 2 then net.host() end
  if stat('frame') == 4 then
    assert(net.recv().kind == 'hosting')
    assert(net.recv().kind == 'connected')
    local data = string.rep('a', 640)
    local before = stat('cpu_cycles')
    net.send(data)
    local after = stat('cpu_cycles')
    print('send ' .. (after - before - 1))
  end
  if stat('frame') == 5 then
    local before = stat('cpu_cycles')
    local e = net.recv()
    local after = stat('cpu_cycles')
    print('recv ' .. (after - before - 1) .. ' ' .. #e.data)
  end
end
";
    let (a, mut b) = MemoryTransport::pair();
    let mut c = net_console(src, true);
    let mut link = Link::over(Box::new(a));
    for frame in 1..=5 {
        if frame == 3 {
            b.push(Command::Join {
                ticket: MEMORY_TICKET.into(),
            });
        }
        if frame == 4 {
            b.push(Command::Send {
                data: vec![b'z'; 900],
            });
        }
        c.step_linked(&mut link, FrameInput::NONE);
        assert_eq!(c.state().fault(), None, "{:?}", c.state());
        if frame == 4 {
            // The `stat` call after the send costs one cycle, subtracted
            // above; the formula is what the reference renders.
            let expected = Price::Bytes128("data bytes")
                .formula()
                .unwrap()
                .eval(&[640]);
            assert_eq!(logs(&c), [format!("send {expected}")]);
        }
    }
    let expected = Price::Bytes128("data bytes")
        .formula()
        .unwrap()
        .eval(&[900]);
    assert_eq!(logs(&c), [format!("recv {expected} 900")]);
}

#[test]
fn binary_payloads_survive_and_stats_need_the_service() {
    let src = "\
function _update(dt)
  if stat('frame') == 2 then net.host() end
  if stat('frame') == 5 then
    print(net.recv().kind .. ' ' .. net.recv().kind)
    local e = net.recv()
    print(#e.data .. ' ' .. e.data:byte(1) .. ' ' .. e.data:byte(2) .. ' ' .. e.data:byte(3))
  end
end
";
    let (a, mut b) = MemoryTransport::pair();
    let mut c = net_console(src, true);
    let mut link = Link::over(Box::new(a));
    for frame in 1..=5 {
        if frame == 3 {
            b.push(Command::Join {
                ticket: MEMORY_TICKET.into(),
            });
        }
        if frame == 4 {
            b.push(Command::Send {
                data: vec![0, 0xff, 0xfe],
            });
        }
        c.step_linked(&mut link, FrameInput::NONE);
        assert_eq!(c.state().fault(), None, "{:?}", c.state());
    }
    // Frame 5 reads the queued `connected` first, then the message.
    assert_eq!(logs(&c), ["hosting connected", "3 0 255 254"]);

    let mut plain = super::console("function _init() print(pcall(stat, 'net_sent')) end");
    plain.step(FrameInput::NONE);
    let log = logs(&plain);
    assert!(log[0].contains("needs the net service"), "{}", log[0]);
    let mut plain = super::console("function _init() print(net) end");
    plain.step(FrameInput::NONE);
    assert_eq!(logs(&plain), ["nil"]);
    let _ = Rc::new(());
}
