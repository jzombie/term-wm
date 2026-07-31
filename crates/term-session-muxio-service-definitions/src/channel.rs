use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

#[cfg(not(target_os = "windows"))]
use std::os::unix::fs::PermissionsExt;

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

#[derive(Debug, Clone)]
pub struct ChannelResolver {
    base_dir: PathBuf,
}

impl ChannelResolver {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn default_channels_dir() -> PathBuf {
        #[cfg(target_os = "windows")]
        {
            dirs::data_local_dir()
                .unwrap_or_else(|| std::env::temp_dir().join("term-wm"))
                .join("channels")
        }
        #[cfg(not(target_os = "windows"))]
        {
            let uid = unsafe { libc::getuid() };
            let base = dirs::data_dir().unwrap_or_else(|| {
                PathBuf::from(format!("/tmp/term-wm-{}", uid))
            });
            base.join("term-wm").join("channels")
        }
    }

    pub fn resolve(&self, channel: &ChannelName) -> io::Result<PathBuf> {
        let ns_dir = self.base_dir.join(&channel.namespace);
        fs::create_dir_all(&ns_dir)?;
        #[cfg(unix)]
        {
            fs::set_permissions(&ns_dir, fs::Permissions::from_mode(0o700))?;
        }
        let socket_path = ns_dir.join(format!("{}.sock", channel.name));
        let path_len = socket_path.to_string_lossy().as_bytes().len();
        if path_len >= 100 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "resolved path '{}' ({} bytes) exceeds POSIX 100-byte budget",
                    socket_path.display(),
                    path_len
                ),
            ));
        }
        Ok(socket_path)
    }
}
