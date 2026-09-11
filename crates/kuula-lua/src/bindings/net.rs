//! The `net` table: installed at a cart's first step, only when its
//! draw state carries a `NetState` (the manifest declared `services =
//! ["net"]`). Every call reads or writes that state and nothing else;
//! the host does the networking between frames. No call here raises a
//! Lua error for a network condition: those are `failed` events. Only
//! a malformed argument (a size, a type) is an error.

use kuula_core::meter::price;
use kuula_core::net::{Event, NetState, Status, MAX_DATA, MAX_TICKET};
use kuula_core::Category;
use mlua::{Error, Lua, Result, Value};

use crate::api::reg::Reg;
use crate::api::{Group, Price, Scope, Sig};
use crate::bindings::with_ctx;
use crate::meter::charge;

fn with_net<R>(lua: &Lua, f: impl FnOnce(&mut NetState) -> R) -> Result<R> {
    with_ctx(lua, |ctx| {
        ctx.state
            .net
            .as_mut()
            .map(f)
            .ok_or_else(|| Error::runtime("net is not available to this cart"))
    })?
}

fn one<R>(lua: &Lua, f: impl FnOnce(&mut NetState) -> R) -> Result<R> {
    charge(lua, Category::Api, 1)?;
    with_net(lua, f)
}

/// An event as the cart sees it: a table with `kind` and the fields
/// that kind carries.
fn event_table(lua: &Lua, e: Event) -> Result<mlua::Table> {
    let t = lua.create_table()?;
    t.set("kind", e.kind())?;
    match e {
        Event::Hosting { ticket } => t.set("ticket", ticket)?,
        Event::Connected { peer } => t.set("peer", peer)?,
        Event::Message { from, data } => {
            t.set("from", from)?;
            t.set("data", lua.create_string(&data)?)?;
        }
        Event::Disconnected { reason } => t.set("reason", reason.as_str())?,
        Event::Failed { code, detail } => {
            t.set("code", code.as_str())?;
            t.set("detail", detail)?;
        }
        Event::Permission { granted } => t.set("granted", granted)?,
    }
    Ok(t)
}

pub(crate) fn install(reg: &mut Reg<'_>) -> Result<()> {
    reg.setup(|lua| lua.globals().set("net", lua.create_table()?))?;

    reg.function(&HOST, |lua, ()| one(lua, |n| n.host()))?;
    reg.function(&JOIN, |lua, ticket: mlua::LuaString| {
        let bytes = ticket.as_bytes();
        if bytes.len() > MAX_TICKET {
            return Err(Error::runtime(format!(
                "net_ticket: ticket of {} bytes, at most {MAX_TICKET}",
                bytes.len()
            )));
        }
        let ticket = std::str::from_utf8(&bytes)
            .map_err(|_| Error::runtime("net_ticket: ticket is not UTF-8"))?
            .to_string();
        one(lua, |n| n.join(ticket))
    })?;
    reg.function(&SEND, |lua, data: mlua::LuaString| {
        let bytes = data.as_bytes();
        if bytes.is_empty() || bytes.len() > MAX_DATA {
            return Err(Error::runtime(format!(
                "net_data: data of {} bytes, must be 1 to {MAX_DATA}",
                bytes.len()
            )));
        }
        charge(lua, Category::Api, price::bytes128(bytes.len() as u64))?;
        let data = bytes.to_vec();
        with_net(lua, |n| n.send(data))
    })?;
    reg.function(&RECV, |lua, ()| {
        let event = with_net(lua, |n| n.recv())?;
        let cost = match &event {
            Some(Event::Message { data, .. }) => price::bytes128(data.len() as u64),
            _ => 1,
        };
        charge(lua, Category::Api, cost)?;
        match event {
            None => Ok(Value::Nil),
            Some(e) => Ok(Value::Table(event_table(lua, e)?)),
        }
    })?;
    reg.function(&STATUS, |lua, ()| one(lua, |n| n.status.as_str()))?;
    reg.function(&TICKET, |lua, ()| {
        one(lua, |n| {
            if n.status == Status::Ended || n.status == Status::Off {
                None
            } else {
                n.ticket.clone()
            }
        })
    })?;
    reg.function(&PEERS, |lua, ()| {
        let peer = one(lua, |n| n.peer)?;
        let t = lua.create_table()?;
        if let Some(p) = peer {
            t.set(1, p)?;
        }
        Ok(t)
    })?;
    reg.function(&INVITE, |lua, ()| one(lua, |n| n.invite.clone()))?;
    reg.function(&LEAVE, |lua, ()| one(lua, |n| n.leave()))?;
    Ok(())
}

binding!(HOST {
    name: "host",
    scope: Scope::Net,
    group: Group::Net,
    sigs: &[Sig::new(
        "net.host()",
        "nothing; asks the host to listen for one peer",
    )],
    price: Price::One,
    doc: "The ticket arrives as a `hosting` event and is readable from \
          `net.ticket()` afterwards. Without permission the answer is a \
          `failed` event with code `net_denied`; while already hosting, \
          joining or connected it is `net_declined`.",
});

binding!(JOIN {
    name: "join",
    scope: Scope::Net,
    group: Group::Net,
    sigs: &[Sig::new(
        "net.join(ticket)",
        "nothing; asks the host to connect to a peer's ticket",
    )],
    price: Price::One,
    errors: &["net_ticket"],
    doc: "A ticket over 1024 bytes or not UTF-8 is a Lua error. A ticket \
          that does not parse or a peer that does not answer is a \
          `failed` event (`net_ticket`, `net_connect`), never an error.",
});

binding!(SEND {
    name: "send",
    scope: Scope::Net,
    group: Group::Net,
    sigs: &[Sig::new(
        "net.send(data)",
        "`true` if queued in this frame's outbox, `false` if there is no session or the outbox (16 per frame) is full",
    )],
    price: Price::Bytes128("data bytes"),
    errors: &["net_data"],
    doc: "`data` is a string of 1 to 1024 bytes, any content; other sizes \
          are a Lua error. Accepted locally does not mean delivered: the \
          message leaves after the frame, and a session that ends first \
          loses it.",
});

binding!(RECV {
    name: "recv",
    scope: Scope::Net,
    group: Group::Net,
    sigs: &[Sig::new(
        "net.recv()",
        "the oldest unread event as a table, or `nil` when there is none",
    )],
    price: Price::Bytes128("data bytes"),
    doc: "The table has `kind` and the fields of that kind: `hosting` \
          (`ticket`), `connected` (`peer`), `message` (`from`, `data`), \
          `disconnected` (`reason`: `left`, `lost` or `closed`), `failed` \
          (`code`, `detail`) and `permission` (`granted`). Events are \
          admitted at the top of the frame, at most 64 per frame, and the \
          inbox holds 256; drain it every frame.",
});

binding!(STATUS {
    name: "status",
    scope: Scope::Net,
    group: Group::Net,
    sigs: &[Sig::new(
        "net.status()",
        "`\"off\"`, `\"hosting\"`, `\"joining\"`, `\"connected\"` or `\"ended\"`",
    )],
    price: Price::One,
});

binding!(TICKET {
    name: "ticket",
    scope: Scope::Net,
    group: Group::Net,
    sigs: &[Sig::new(
        "net.ticket()",
        "the ticket this cart is hosting under, or `nil`",
    )],
    price: Price::One,
});

binding!(PEERS {
    name: "peers",
    scope: Scope::Net,
    group: Group::Net,
    sigs: &[Sig::new(
        "net.peers()",
        "a list of peer numbers: `{}` or `{ 1 }`",
    )],
    price: Price::One,
});

binding!(INVITE {
    name: "invite",
    scope: Scope::Net,
    group: Group::Net,
    sigs: &[Sig::new(
        "net.invite()",
        "the ticket the run was launched with (`--net join`), or `nil`",
    )],
    price: Price::One,
});

binding!(LEAVE {
    name: "leave",
    scope: Scope::Net,
    group: Group::Net,
    sigs: &[Sig::new(
        "net.leave()",
        "nothing; ends the session or stops hosting",
    )],
    price: Price::One,
    doc: "A `disconnected` event with reason `closed` follows.",
});
