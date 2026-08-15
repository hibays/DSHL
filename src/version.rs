//! Minimal semantic-version parsing and comparison.
//!
//! Kept dependency-free on purpose: the only thing we need is to compare
//! `major.minor.patch` tuples (e.g. `node >= 24.15.0`, `bun >= 1.3.14`).

use std::cmp::Ordering;
use std::fmt;

/// A `major.minor.patch` version tuple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parse a version from a loose string.
    ///
    /// Accepts leading `v`/`V`, arbitrary prefixes like `node v26.7.0`,
    /// `cargo 1.97.1`, `fnm 1.38.1`, and ignores trailing pre-release/build
    /// metadata (`1.0.0-rc.6`, `1.0.0+build`).
    pub fn parse(input: &str) -> Option<Self> {
        // Find the first `digit.digit.digit` triple anywhere in the string.
        let bytes = input.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i].is_ascii_digit() {
                // Try to parse three dot-separated numeric groups.
                let mut parts = [0u64; 3];
                let mut group = 0;
                let mut ok = true;
                let mut j = i;
                while group < 3 {
                    let mut value = 0u64;
                    let mut any = false;
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        value = value
                            .saturating_mul(10)
                            .saturating_add((bytes[j] - b'0') as u64);
                        any = true;
                        j += 1;
                    }
                    if !any {
                        ok = false;
                        break;
                    }
                    parts[group] = value;
                    group += 1;
                    if group < 3 {
                        if j < bytes.len() && bytes[j] == b'.' {
                            j += 1;
                        } else {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok && group == 3 {
                    // Make sure the triple is a whole component (next char is a
                    // separator/end, not another digit, e.g. `1.0.0rc` is fine,
                    // `1.0.012` is not a valid triple boundary but is harmless).
                    return Some(Self::new(parts[0], parts[1], parts[2]));
                }
            }
            i += 1;
        }
        None
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// One dot-separated pre-release identifier, e.g. `rc` or `6` in `1.0.0-rc.6`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreId {
    Num(u64),
    Str(String),
}

/// A full semantic version: `major.minor.patch` plus an optional pre-release
/// (`0.1.0-rc.6`).
///
/// Unlike `Version` — which intentionally drops pre-release metadata so
/// `0.1.0-rc.5` and `0.1.0-rc.6` collapse to the same tuple — `FullVersion`
/// keeps it and orders per the semver rules: `0.1.0-rc.6 < 0.1.0`,
/// `0.1.0-rc.5 < 0.1.0-rc.6`. Build metadata (`+build`) is parsed but
/// ignored, exactly like `Version` ignores trailing metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<PreId>,
}

impl FullVersion {
    /// Parse a version from a loose string, keeping the pre-release suffix.
    ///
    /// Finds the first `digit.digit.digit` triple anywhere in the string (the
    /// same scan as `Version::parse`) and reads an optional `-pre.release`
    /// and `+build` suffix after it. Returns `None` when no version triple is
    /// present.
    pub fn parse(input: &str) -> Option<Self> {
        let bytes = input.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i].is_ascii_digit() {
                // Try to parse three dot-separated numeric groups.
                let mut parts = [0u64; 3];
                let mut group = 0;
                let mut ok = true;
                let mut j = i;
                while group < 3 {
                    let mut value = 0u64;
                    let mut any = false;
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        value = value
                            .saturating_mul(10)
                            .saturating_add((bytes[j] - b'0') as u64);
                        any = true;
                        j += 1;
                    }
                    if !any {
                        ok = false;
                        break;
                    }
                    parts[group] = value;
                    group += 1;
                    if group < 3 {
                        if j < bytes.len() && bytes[j] == b'.' {
                            j += 1;
                        } else {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok && group == 3 {
                    // Optional pre-release: `-id(.id)*`.
                    let mut pre = Vec::new();
                    if j < bytes.len() && bytes[j] == b'-' {
                        j += 1;
                        let mut id = String::new();
                        let mut has_id = false;
                        while j < bytes.len()
                            && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-')
                        {
                            id.push(bytes[j] as char);
                            has_id = true;
                            j += 1;
                        }
                        if !has_id {
                            pre.clear(); // `0.1.0-` — no pre-release after all
                        } else {
                            push_pre(&mut pre, &id);
                            while j < bytes.len() && bytes[j] == b'.' {
                                j += 1;
                                id.clear();
                                has_id = false;
                                while j < bytes.len()
                                    && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-')
                                {
                                    id.push(bytes[j] as char);
                                    has_id = true;
                                    j += 1;
                                }
                                if !has_id {
                                    break; // trailing dot: stop the pre-release
                                }
                                push_pre(&mut pre, &id);
                            }
                        }
                    }
                    // Optional `+build` metadata: parsed but ignored.
                    if j < bytes.len() && bytes[j] == b'+' {
                        while j < bytes.len()
                            && (bytes[j].is_ascii_alphanumeric()
                                || bytes[j] == b'-'
                                || bytes[j] == b'.')
                        {
                            j += 1;
                        }
                    }
                    return Some(Self {
                        major: parts[0],
                        minor: parts[1],
                        patch: parts[2],
                        pre,
                    });
                }
            }
            i += 1;
        }
        None
    }
}

fn push_pre(pre: &mut Vec<PreId>, id: &str) {
    if let Ok(n) = id.parse::<u64>() {
        pre.push(PreId::Num(n));
    } else {
        pre.push(PreId::Str(id.to_string()));
    }
}

/// Semver pre-release precedence: a version *without* a pre-release is
/// greater than the same version *with* one; identifiers compare numerically
/// when both are numeric, lexically (ASCII) when both are strings, and a
/// numeric identifier is always lower than a string identifier. When one
/// list is a prefix of the other, the shorter one is lower.
fn pre_cmp(a: &[PreId], b: &[PreId]) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            for (x, y) in a.iter().zip(b.iter()) {
                let ord = match (x, y) {
                    (PreId::Num(n), PreId::Num(m)) => n.cmp(m),
                    (PreId::Num(_), PreId::Str(_)) => Ordering::Less,
                    (PreId::Str(_), PreId::Num(_)) => Ordering::Greater,
                    (PreId::Str(s), PreId::Str(t)) => s.cmp(t),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            a.len().cmp(&b.len())
        }
    }
}

impl Ord for FullVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| pre_cmp(&self.pre, &other.pre))
    }
}

impl PartialOrd for FullVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for FullVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (idx, id) in self.pre.iter().enumerate() {
            let sep = if idx == 0 { "-" } else { "." };
            match id {
                PreId::Num(n) => write!(f, "{sep}{n}")?,
                PreId::Str(s) => write!(f, "{sep}{s}")?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_formats() {
        assert_eq!(Version::parse("v26.7.0"), Some(Version::new(26, 7, 0)));
        assert_eq!(Version::parse("26.7.0"), Some(Version::new(26, 7, 0)));
        assert_eq!(Version::parse("1.3.14"), Some(Version::new(1, 3, 14)));
        assert_eq!(Version::parse("node v26.7.0"), Some(Version::new(26, 7, 0)));
        assert_eq!(
            Version::parse("cargo 1.97.1 (x)"),
            Some(Version::new(1, 97, 1))
        );
        assert_eq!(Version::parse("0.1.0-rc.6"), Some(Version::new(0, 1, 0)));
        assert_eq!(Version::parse("fnm 1.38.1"), Some(Version::new(1, 38, 1)));
    }

    #[test]
    fn rejects_non_versions() {
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("hello"), None);
        assert_eq!(Version::parse("v26"), None);
    }

    #[test]
    fn orders_correctly() {
        assert!(Version::new(26, 7, 0) >= Version::new(24, 15, 0));
        assert!(Version::new(1, 3, 14) >= Version::new(1, 3, 14));
        assert!(Version::new(24, 15, 0) < Version::new(24, 15, 1));
    }

    #[test]
    fn full_version_parses_prerelease() {
        let rc6 = FullVersion {
            major: 0,
            minor: 1,
            patch: 0,
            pre: vec![PreId::Str("rc".into()), PreId::Num(6)],
        };
        assert_eq!(FullVersion::parse("0.1.0-rc.6"), Some(rc6.clone()));
        assert_eq!(FullVersion::parse("dsh 0.1.0-rc.6"), Some(rc6.clone()));
        assert_eq!(FullVersion::parse("v0.1.0-rc.6"), Some(rc6.clone()));
        // Build metadata is ignored.
        assert_eq!(FullVersion::parse("0.1.0-rc.6+build.5"), Some(rc6.clone()));
        assert_eq!(
            FullVersion::parse("1.2.3"),
            Some(FullVersion {
                major: 1,
                minor: 2,
                patch: 3,
                pre: vec![]
            })
        );
        assert_eq!(
            FullVersion::parse("0.1.0-rc.6").unwrap().to_string(),
            "0.1.0-rc.6"
        );
        assert_eq!(
            FullVersion::parse("0.1.0-rc.6").unwrap().to_string(),
            rc6.to_string()
        );
        assert_eq!(FullVersion::parse("hello"), None);
    }

    #[test]
    fn full_version_orders_prereleases() {
        let v = |s: &str| FullVersion::parse(s).unwrap();
        assert!(v("0.1.0-rc.5") < v("0.1.0-rc.6"));
        assert!(v("0.1.0-rc.6") < v("0.1.0"));
        assert!(v("0.1.0-rc.6") == v("0.1.0-rc.6"));
        assert!(v("0.1.0-rc.10") > v("0.1.0-rc.9"));
        assert!(v("1.0.0-1") < v("1.0.0-alpha"));
        assert!(v("1.0.0-alpha.1") < v("1.0.0-alpha.2"));
        assert!(v("1.0.0-alpha") < v("1.0.0-alpha.1"));
        assert!(v("1.0.0-beta") > v("1.0.0-alpha"));
    }
}
