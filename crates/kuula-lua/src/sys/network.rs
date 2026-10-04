//! Host-only network screens and requests.
use super::{request, with_sys};
use crate::api::{reg::Reg, Group, Price, Scope, Sig};
use kuula_core::shell::{network::Action, SysRequest};
use mlua::{Error, Result};

pub(super) fn install(reg: &mut Reg<'_>) -> Result<()> {
    reg.function(&NETWORK, |lua, ()| {
        let v = with_sys(lua, |s| s.network.clone())?;
        let t = lua.create_table()?;
        t.set("status", v.status)?;
        t.set("ended", v.ended)?;
        t.set("detail", v.detail)?;
        t.set("unavailable", v.unavailable)?;
        t.set("ticket", v.ticket)?;
        t.set("path", v.path)?;
        t.set("relay", v.relay)?;
        t.set("relay_only", v.relay_only)?;
        t.set("discovery", v.discovery)?;
        t.set("discovery_detail", v.discovery_detail)?;
        t.set("editing", v.editing)?;
        t.set("text", v.text)?;
        t.set("network_profile", v.network_profile)?;
        let candidates = lua.create_table()?;
        for (i, c) in v.candidates.into_iter().take(32).enumerate() {
            let entry = lua.create_table()?;
            entry.set("title", c.title)?;
            entry.set("ticket", c.ticket)?;
            candidates.set(i + 1, entry)?;
        }
        t.set("candidates", candidates)?;
        Ok(t)
    })?;
    reg.function(
        &RUN_NETWORK,
        |lua, (name, invite): (String, Option<String>)| {
            if invite
                .as_ref()
                .is_some_and(|t| t.is_empty() || t.len() > kuula_core::net::MAX_TICKET)
            {
                return Err(Error::runtime("ticket must contain 1 to 1024 bytes"));
            }
            request(lua, SysRequest::RunNetwork { name, invite })
        },
    )?;
    reg.function(
        &NET_ACTION,
        |lua, (action, value, on): (String, Option<String>, Option<bool>)| {
            let value = value.unwrap_or_default();
            if value.len() > kuula_core::net::MAX_TICKET
                || (action != "browse"
                    && (!value.is_ascii() || value.bytes().any(|b| b.is_ascii_control())))
            {
                return Err(Error::runtime(
                    "network UI text must be printable ASCII, at most 1024 bytes",
                ));
            }
            let action = match action.as_str() {
                "overlay" => Action::Overlay(on.unwrap_or(false)),
                "browse" => Action::Browse(value),
                "discovery" => Action::Discovery(on.unwrap_or(false)),
                "relay" => Action::Relay {
                    url: value,
                    only: on.unwrap_or(false),
                },
                "edit" if matches!(value.as_str(), "" | "ticket" | "relay") => Action::Edit(value),
                "text" => Action::Text(value),
                "copy" => Action::Copy,
                "paste" => Action::Paste,
                _ => return Err(Error::runtime("unknown network UI action")),
            };
            request(lua, SysRequest::Network(action))
        },
    )?;
    Ok(())
}

binding!(NETWORK {
    name: "network", scope: Scope::Sys, group: Group::Shell,
    sigs: &[Sig::new("sys.network()", "host UI status, ended, detail, unavailable, ticket, path, relay, relay_only, discovery, discovery_detail, candidates, editing, text and network_profile")],
    price: Price::One,
    doc: "Host diagnostics and LAN candidates are shell-only; they never enter cart state or replay. Candidates are untrusted hints, not approval to launch or join.",
});
binding!(RUN_NETWORK {
    name: "run_network", scope: Scope::Sys, group: Group::Shell,
    sigs: &[Sig::new("sys.run_network(name, invite?)", "start the cart with an optional join ticket; permission is a separate setting")],
    price: Price::One,
    doc: "The cart still initiates host/join through net.host/net.join. A supplied ticket becomes net.invite(). A normal sys.run clears the invite.",
});
binding!(NET_ACTION {
    name: "net_action", scope: Scope::Sys, group: Group::Shell,
    sigs: &[Sig::new("sys.net_action(action, value?, on?)", "queue browse(name), discovery(on), relay(url,on), edit(ticket/relay/empty), text(value), copy or paste")],
    price: Price::One,
    doc: "Discovery and relay configuration are explicit host actions. Relay changes require ending the current cart. Copy/paste use the desktop clipboard only after a player action.",
});
