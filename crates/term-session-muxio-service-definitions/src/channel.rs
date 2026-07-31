use std::fmt;

use interprocess::local_socket::{GenericNamespaced, Stream, prelude::*};

#[derive(Debug, Clone)]
pub struct ChannelName {
    pub namespace: String,
    pub name: String,
}

impl ChannelName {
    pub fn parse(input: &str) -> Result<Self, String> {
        let input = input.trim();
        let parts: Vec<&str> = input.split('/').collect();
        let (ns, name) = match parts.as_slice() {
            [name] => ("default", *name),
            [ns, name] => (*ns, *name),
            _ => return Err(format!(
                "invalid channel format '{input}': expected 'name' or 'namespace/name'"
            )),
        };
        let is_valid = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        if !is_valid(ns) {
            return Err(format!(
                "invalid namespace '{ns}': must be non-empty alphanumeric, hyphen, or underscore"
            ));
        }
        if !is_valid(name) {
            return Err(format!(
                "invalid name '{name}': must be non-empty alphanumeric, hyphen, or underscore"
            ));
        }
        Ok(Self {
            namespace: ns.to_string(),
            name: name.to_string(),
        })
    }
}

impl fmt::Display for ChannelName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.namespace, self.name)
    }
}

// ── IPC endpoint probing ──────────────────────────────────────────────

/// Returns `true` if a session server is reachable on the given channel.
///
/// The probe uses the exact same `GenericNamespaced` mapping as muxio's
/// `RpcIpcServer::serve` and `RpcIpcClient`, so it always targets the socket
/// location the library chose on the current platform (Linux abstract
/// namespace, macOS `/tmp`, Windows named pipes).
pub fn probe_ipc_endpoint(channel: &ChannelName) -> bool {
    let Ok(name) = channel.to_string().to_ns_name::<GenericNamespaced>() else {
        return false;
    };
    Stream::connect(name).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_name_parses_name_and_namespace() {
        let single = ChannelName::parse("main").unwrap();
        assert_eq!(single.namespace, "default");
        assert_eq!(single.name, "main");

        let both = ChannelName::parse("workspace/dev").unwrap();
        assert_eq!(both.namespace, "workspace");
        assert_eq!(both.name, "dev");
    }

    #[test]
    fn channel_name_rejects_invalid_input() {
        assert!(ChannelName::parse("").is_err());
        assert!(ChannelName::parse("a/b/c").is_err());
        assert!(ChannelName::parse("has space/name").is_err());
        assert!(ChannelName::parse("/bare").is_err());
    }

    #[test]
    fn channel_name_display_round_trips() {
        let ch = ChannelName::parse("default/main").unwrap();
        assert_eq!(ch.to_string(), "default/main");
    }

    #[test]
    fn probe_is_false_when_nothing_is_bound() {
        let channel = ChannelName::parse("probe/not_listening").unwrap();
        assert!(!probe_ipc_endpoint(&channel));
    }
}
