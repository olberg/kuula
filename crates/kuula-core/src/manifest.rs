//! `cart.toml`: the minimal manifest. Every field is optional; unknown
//! keys are errors so a typo cannot silently do nothing.

use std::fmt;

use serde::Deserialize;

pub const MANIFEST_FILE: &str = "cart.toml";

/// The one screen mode a cart declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub enum ScreenMode {
    #[serde(rename = "320x240")]
    Low,
    /// The system's primary resolution and the default.
    #[default]
    #[serde(rename = "640x480")]
    High,
}

impl ScreenMode {
    pub fn size(self) -> (u32, u32) {
        match self {
            ScreenMode::Low => (320, 240),
            ScreenMode::High => (640, 480),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ScreenMode::Low => "320x240",
            ScreenMode::High => "640x480",
        }
    }

    pub fn parse(name: &str) -> Option<ScreenMode> {
        match name {
            "320x240" => Some(ScreenMode::Low),
            "640x480" => Some(ScreenMode::High),
            _ => None,
        }
    }
}

/// A host service a cart declares under `[cart] services`. Only `net`
/// exists; an unknown name is a manifest error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Service {
    #[serde(rename = "net")]
    Net,
}

impl Service {
    pub fn as_str(self) -> &'static str {
        match self {
            Service::Net => "net",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Manifest {
    pub title: String,
    pub screen_mode: ScreenMode,
    /// Sheet names decoded at boot, `gfx/<name>.png`.
    pub preload_sheets: Vec<String>,
    /// Map names decoded at boot, `map/<name>.json`.
    pub preload_maps: Vec<String>,
    /// Host services the cart asked for; `net` gives it the `net` table.
    pub services: Vec<Service>,
}

impl Manifest {
    pub fn has_service(&self, s: Service) -> bool {
        self.services.contains(&s)
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    cart: CartSection,
    #[serde(default)]
    preload: PreloadSection,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CartSection {
    #[serde(default)]
    title: String,
    #[serde(default)]
    screen_mode: ScreenMode,
    #[serde(default)]
    services: Vec<Service>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PreloadSection {
    #[serde(default)]
    sheets: Vec<String>,
    #[serde(default)]
    maps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub message: String,
    pub line: Option<u32>,
}

impl ManifestError {
    pub const CODE: &'static str = "manifest_error";
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ManifestError {}

/// Largest manifest accepted, well above anything reasonable.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
        if text.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError {
                message: format!("cart.toml is larger than {MAX_MANIFEST_BYTES} bytes"),
                line: None,
            });
        }
        let file: File = toml::from_str(text).map_err(|e| {
            let line = e
                .span()
                .map(|s| text[..s.start].matches('\n').count() as u32 + 1);
            ManifestError {
                message: e.message().to_string(),
                line,
            }
        })?;
        let names = |list: &[String], what: &str| -> Result<(), ManifestError> {
            for n in list {
                if !valid_asset_name(n) {
                    return Err(ManifestError {
                        message: format!(
                            "preload.{what} entry {n:?} must be letters, digits, '_' or '-'"
                        ),
                        line: None,
                    });
                }
            }
            Ok(())
        };
        names(&file.preload.sheets, "sheets")?;
        names(&file.preload.maps, "maps")?;
        let mut services = file.cart.services;
        services.dedup();
        Ok(Manifest {
            title: file.cart.title,
            screen_mode: file.cart.screen_mode,
            preload_sheets: file.preload.sheets,
            preload_maps: file.preload.maps,
            services,
        })
    }
}

/// Asset names are a single path component without an extension.
pub fn valid_asset_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_manifest_is_all_defaults() {
        let m = Manifest::parse("").unwrap();
        assert_eq!(m, Manifest::default());
        assert_eq!(m.screen_mode, ScreenMode::High);
        assert_eq!(m.screen_mode.size(), (640, 480));
    }

    #[test]
    fn full_manifest_parses() {
        let m = Manifest::parse(
            "[cart]\ntitle = \"Tiles\"\nscreen_mode = \"320x240\"\n\n[preload]\nsheets = [\"tiles\", \"hero-2\"]\nmaps = [\"overworld\"]\n",
        )
        .unwrap();
        assert_eq!(m.title, "Tiles");
        assert_eq!(m.screen_mode, ScreenMode::Low);
        assert_eq!(m.preload_sheets, ["tiles", "hero-2"]);
        assert_eq!(m.preload_maps, ["overworld"]);
    }

    #[test]
    fn unknown_keys_and_bad_modes_are_errors_with_lines() {
        let e = Manifest::parse("[cart]\nscreen_mode = \"800x600\"\n").unwrap_err();
        assert!(e.message.contains("800x600"), "{}", e.message);
        assert_eq!(e.line, Some(2));
        let e = Manifest::parse("[cart]\ntitle = 1\n\n[cart2]\nx = 1\n").unwrap_err();
        assert!(e.line.is_some());
        let e = Manifest::parse("[cart]\nscreenmode = \"320x240\"\n").unwrap_err();
        assert!(e.message.contains("screenmode"), "{}", e.message);
        let e = Manifest::parse("[preload]\nsheets = [\"../x\"]\n").unwrap_err();
        assert!(e.message.contains("../x"), "{}", e.message);
    }

    #[test]
    fn services_are_a_closed_list() {
        let m = Manifest::parse("[cart]\nservices = [\"net\", \"net\"]\n").unwrap();
        assert_eq!(m.services, [Service::Net]);
        assert!(m.has_service(Service::Net));
        assert!(!Manifest::default().has_service(Service::Net));
        let e = Manifest::parse("[cart]\nservices = [\"gossip\"]\n").unwrap_err();
        assert!(e.message.contains("gossip"), "{}", e.message);
        assert_eq!(e.line, Some(2));
        let e = Manifest::parse("[cart]\nservice = [\"net\"]\n").unwrap_err();
        assert!(e.message.contains("service"), "{}", e.message);
    }

    #[test]
    fn asset_names_are_one_plain_component() {
        assert!(valid_asset_name("tiles"));
        assert!(valid_asset_name("hero_2-b"));
        assert!(!valid_asset_name(""));
        assert!(!valid_asset_name("a/b"));
        assert!(!valid_asset_name("a.png"));
        assert!(!valid_asset_name(&"x".repeat(65)));
    }
}
