//! Workspace-name derivation shared by the CLI launcher and app branding.
//!
//! #284: an unnamed `-w` workspace defaults to the sanitized launch-directory
//! basename so each project lands in a self-named workspace instead of a
//! generic one. Kept in the library so the bundled binary and embedders share
//! one derivation path.

/// Mirrors `term_session::DEFAULT_WORKSPACE`; duplicated as a literal because
/// the `term-session` crate is only linked when session persistence is
/// compiled in.
#[cfg(not(feature = "session-persistence"))]
pub const FALLBACK_WORKSPACE: &str = "default";

/// Replacement character for bytes invalid in a workspace (ChannelName)
/// namespace segment.
const WORKSPACE_NAME_FILL_CHAR: char = '_';

/// Sanitize a raw name into a `ChannelName`-safe namespace segment: keep
/// `[A-Za-z0-9_-]`, map everything else to `WORKSPACE_NAME_FILL_CHAR`, and
/// trim fill characters from both ends. Returns `None` when nothing usable
/// remains so callers can apply their own fallback (#284).
pub fn sanitize_workspace_name_opt(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                WORKSPACE_NAME_FILL_CHAR
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(WORKSPACE_NAME_FILL_CHAR);
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The launch directory's basename, when resolvable.
pub fn cwd_basename() -> Option<String> {
    std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
}

#[cfg(feature = "session-persistence")]
pub fn resolve_workspace_arg(arg: &Option<String>) -> String {
    arg.clone().unwrap_or_else(derive_default_workspace)
}

/// #284: default the initial workspace to the sanitized launch-directory
/// basename so each project lands in a self-named workspace instead of a
/// generic one.
#[cfg(feature = "session-persistence")]
pub fn derive_default_workspace() -> String {
    sanitize_workspace_name_opt(&cwd_basename().unwrap_or_default())
        .unwrap_or_else(term_session_default_workspace)
}

#[cfg(feature = "session-persistence")]
fn term_session_default_workspace() -> String {
    term_session::DEFAULT_WORKSPACE.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_workspace_name_keeps_channel_safe_characters() {
        assert_eq!(
            sanitize_workspace_name_opt("my-app_2"),
            Some("my-app_2".to_string()),
            "valid characters must pass through"
        );
        assert_eq!(
            sanitize_workspace_name_opt("test workspace name"),
            Some("test_workspace_name".to_string()),
            "spaces become fill characters"
        );
        assert_eq!(
            sanitize_workspace_name_opt("sample-dir-123"),
            Some("sample-dir-123".to_string()),
            "already-safe names pass through unchanged"
        );
        assert_eq!(
            sanitize_workspace_name_opt("my.project"),
            Some("my_project".to_string())
        );
        assert_eq!(
            sanitize_workspace_name_opt("  padded  "),
            Some("padded".to_string()),
            "outer whitespace is trimmed before sanitizing"
        );
    }

    #[test]
    fn sanitize_workspace_name_trims_edge_fills_and_rejects_empty_results() {
        assert_eq!(
            sanitize_workspace_name_opt("...proj..."),
            Some("proj".to_string()),
            "edge fill characters are trimmed"
        );
        assert_eq!(sanitize_workspace_name_opt(""), None);
        assert_eq!(sanitize_workspace_name_opt("   "), None);
        assert_eq!(
            sanitize_workspace_name_opt("///"),
            None,
            "names that sanitize to nothing yield None so callers can fall back"
        );
    }

    /// #284: `-w` absent derives the workspace name from the launch directory.
    #[cfg(feature = "session-persistence")]
    #[test]
    #[serial_test::serial(cwd)]
    fn derive_default_workspace_uses_cwd_basename_sanitized() {
        let dir = tempfile::tempdir().expect("tempdir failed");
        let project_dir = dir.path().join("My.Project");
        std::fs::create_dir_all(&project_dir).expect("mkdir");

        let prev = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&project_dir).expect("chdir");
        let derived = derive_default_workspace();
        std::env::set_current_dir(prev).expect("restore cwd");

        assert_eq!(derived, "My_Project");
    }

    /// #284: an explicit `-w` value always wins over cwd derivation.
    #[cfg(feature = "session-persistence")]
    #[test]
    fn resolve_workspace_arg_explicit_value_wins() {
        assert_eq!(
            resolve_workspace_arg(&Some("custom-ws".to_string())),
            "custom-ws"
        );
    }
}
