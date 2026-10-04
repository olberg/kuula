//! Desktop shell network services: configuration, LAN catalog and diagnostics.
use super::Listed;
use kuula_core::{
    net::identity::Identity,
    shell::{
        network::{Action, Candidate},
        SysRequest,
    },
};
use kuula_net::{
    config::RelayConfig,
    discovery::{Advertisement, Discovery},
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const DISCOVERY_RETRY: Duration = Duration::from_secs(5);

#[derive(Clone, Default)]
pub struct Config {
    pub relay: RelayConfig,
    pub discovery: bool,
}
impl Config {
    fn path() -> Option<std::path::PathBuf> {
        Some(crate::settings::path()?.with_file_name("network.kv"))
    }
    fn parse(text: &str, mut config: Self) -> Self {
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "relay" => config.relay.url = (!value.is_empty()).then(|| value.into()),
                "relay_only" => config.relay.only = value == "on",
                "discovery" => config.discovery = value == "on",
                _ => {}
            }
        }
        config
    }
    pub fn load() -> Self {
        let mut config = match Self::path() {
            Some(path) => crate::settings::load_file(&path, Self::default(), Self::parse),
            None => Self::default(),
        };
        if let Err(e) = config.relay.validate() {
            eprintln!("network settings: {e}; relay disabled");
            config.relay = RelayConfig::default();
        }
        config
    }
    fn render(&self) -> String {
        format!(
            "relay = {}\nrelay_only = {}\ndiscovery = {}\n",
            self.relay.url.as_deref().unwrap_or(""),
            if self.relay.only { "on" } else { "off" },
            if self.discovery { "on" } else { "off" }
        )
    }
    fn save(&self) -> std::io::Result<()> {
        match Self::path() {
            Some(path) => crate::settings::save_file(&path, &self.render()),
            None => Ok(()),
        }
    }
}

pub fn service(
    listed: Rc<Vec<Listed>>,
    config: Rc<RefCell<Config>>,
    diagnostics: Rc<RefCell<Arc<Mutex<String>>>>,
    current_identity: Rc<RefCell<Identity>>,
) -> kuula_host_sdl::ShellService {
    let mut selected: Option<(Identity, String)> = None;
    let mut discovery: Option<Discovery> = None;
    // After a socket error the catalog is reopened once this passes, so a
    // transient fault (a roaming adapter, a VPN coming up) recovers.
    let mut discovery_retry_at: Option<Instant> = None;
    let mut was_running = false;
    let profile = Arc::new(Mutex::new(String::from("not checked")));
    let mut profile_started = false;
    Box::new(move |console, requests| {
        let permitted = console.settings().net;
        let running = console.net_state().is_some();
        if was_running != running {
            discovery = None;
        }
        was_running = running;
        for request in requests {
            let SysRequest::Network(action) = request else {
                continue;
            };
            match action {
                Action::Browse(name) => {
                    selected = listed.iter().find(|c| c.entry.name == *name).and_then(|c| {
                        super::open_cart(&c.path)
                            .ok()
                            .map(|s| (Identity::new(&s), c.entry.title.clone()))
                    });
                    if !profile_started {
                        profile_started = true;
                        let profile = profile.clone();
                        let _ = std::thread::Builder::new()
                            .name("kuula-network-profile".into())
                            .spawn(move || {
                                *profile.lock().unwrap() = network_profile();
                            });
                    }
                }
                Action::Discovery(on) => {
                    config.borrow_mut().discovery = *on;
                    discovery = None;
                    discovery_retry_at = None;
                    if let Some(v) = console.network_view_mut() {
                        v.discovery_detail.clear();
                    }
                }
                Action::Relay { url, only } => {
                    let candidate = RelayConfig {
                        url: (!url.is_empty()).then(|| url.clone()),
                        only: *only,
                    };
                    let result = if running {
                        Err("End the current session before changing relay settings.".into())
                    } else {
                        candidate.validate().map_err(|e| e.to_string())
                    };
                    match result {
                        Ok(()) => {
                            config.borrow_mut().relay = candidate;
                            if let Some(v) = console.network_view_mut() {
                                v.detail = "Relay configuration saved.".into();
                            }
                        }
                        Err(e) => {
                            if let Some(v) = console.network_view_mut() {
                                v.detail = e;
                            }
                            continue;
                        }
                    }
                }
                _ => continue,
            }
            if matches!(action, Action::Discovery(_) | Action::Relay { .. }) {
                if let Err(e) = config.borrow().save() {
                    if let Some(v) = console.network_view_mut() {
                        v.detail = format!("Cannot save network settings: {e}");
                    }
                }
            }
        }
        let config = config.borrow();
        let hosting = console
            .net_state()
            .is_some_and(|n| n.status == kuula_core::net::Status::Hosting && n.ticket.is_some());
        let Some(v) = console.network_view_mut() else {
            discovery = None;
            return;
        };
        v.relay = config.relay.url.clone().unwrap_or_default();
        v.relay_only = config.relay.only;
        v.discovery = config.discovery;
        v.network_profile = profile.lock().unwrap().clone();
        v.path = if permitted && running {
            diagnostics.borrow().lock().unwrap().clone()
        } else {
            String::new()
        };
        if !permitted {
            v.path.clear();
        }
        if !permitted || !config.discovery || selected.is_none() {
            discovery = None;
            v.candidates.clear();
            discovery_retry_at = None;
            return;
        }
        let now = Instant::now();
        if discovery.is_none() && discovery_retry_at.is_none_or(|t| now >= t) {
            match Discovery::open() {
                Ok(d) => {
                    discovery = Some(d);
                    discovery_retry_at = None;
                    v.discovery_detail = "Searching on this LAN (IPv4 multicast).".into();
                }
                Err(e) => {
                    v.discovery_detail = format!("LAN discovery unavailable: {e}; retrying");
                    discovery_retry_at = Some(now + DISCOVERY_RETRY);
                }
            }
        }
        if let (Some(d), Some((identity, title))) = (discovery.as_mut(), selected.as_ref()) {
            let advert =
                hosting.then(|| Advertisement::new(&current_identity.borrow(), title, &v.ticket));
            match d.poll(advert.as_ref(), identity, &v.ticket) {
                Ok(candidates) => {
                    v.candidates = candidates
                        .into_iter()
                        .map(|a| Candidate {
                            title: a.title,
                            ticket: a.ticket,
                        })
                        .collect()
                }
                Err(e) => {
                    v.discovery_detail = format!("LAN discovery stopped: {e}; retrying");
                    v.candidates.clear();
                    discovery = None;
                    discovery_retry_at = Some(now + DISCOVERY_RETRY);
                }
            }
        }
    })
}

#[cfg(windows)]
fn network_profile() -> String {
    use std::os::windows::process::CommandExt;
    let mut child = match std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "Get-NetConnectionProfile | ForEach-Object { $_.InterfaceAlias + ': ' + $_.NetworkCategory }"])
        .creation_flags(0x08000000).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn() {
            Ok(c) => c, Err(_) => return "unavailable".into(),
        };
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while child.try_wait().ok().flatten().is_none() {
        if std::time::Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            return "check Windows network profile".into();
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    child
        .wait_with_output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .chars()
                .take(160)
                .collect()
        })
        .filter(|s: &String| !s.is_empty())
        .unwrap_or_else(|| "unavailable".into())
}
#[cfg(not(windows))]
fn network_profile() -> String {
    "Windows network categories do not apply".into()
}
