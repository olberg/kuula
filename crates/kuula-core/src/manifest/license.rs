//! Exact SPDX identifiers, either cart-wide or split by code and assets.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum License {
    All(String),
    Split { code: String, assets: String },
}

impl License {
    pub fn validate(&self) -> Result<(), String> {
        let ids = match self {
            Self::All(id) => vec![id],
            Self::Split { code, assets } => vec![code, assets],
        };
        for id in ids {
            if !spdx::license_id(id).is_some_and(|known| known.name == id) {
                return Err(format!("unknown SPDX license identifier {id:?}"));
            }
        }
        Ok(())
    }
}

impl std::fmt::Display for License {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All(id) => f.write_str(id),
            Self::Split { code, assets } => write!(f, "code: {code}; assets: {assets}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Manifest;

    #[test]
    fn metadata_accepts_single_and_split_licenses() {
        let m = Manifest::parse("[cart]\nauthor = 'Ada'\nlicense = 'MIT'\n").unwrap();
        assert_eq!(m.author, "Ada");
        assert_eq!(m.license.unwrap().to_string(), "MIT");
        let m = Manifest::parse("[cart]\nlicense.code = 'MIT'\nlicense.assets = 'CC-BY-4.0'\n")
            .unwrap();
        assert_eq!(
            m.license.unwrap().to_string(),
            "code: MIT; assets: CC-BY-4.0"
        );
    }

    #[test]
    fn unknown_and_partial_licenses_fail_readably() {
        let e = Manifest::parse("[cart]\nlicense = 'Made-Up'\n").unwrap_err();
        assert!(e.message.contains("Made-Up"));
        for text in [
            "license = 'MIT+'",
            "license = 'MIT OR Apache-2.0'",
            "license.code = 'MIT'",
            "license = { code = 'MIT', assets = 'MIT', typo = 'MIT' }",
        ] {
            assert!(
                Manifest::parse(&format!("[cart]\n{text}\n")).is_err(),
                "{text}"
            );
        }
    }
}
