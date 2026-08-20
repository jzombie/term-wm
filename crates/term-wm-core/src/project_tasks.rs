use std::collections::HashMap;
#[cfg(feature = "project-tasks")]
use std::fs;
use std::path::{Path, PathBuf};

use term_wm_config::env::{Environment, parse_environment};
#[cfg(feature = "project-tasks")]
use term_wm_config::env::active_environment;

/// The sole task file path, resolved from the WM launch directory.
pub const TERM_WM_TASKS_PATH: &str = ".term-wm/tasks.json";

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct ProjectTaskConfig {
    pub label: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub environments: Vec<String>,
}

/// Discovery result: the project root (dir where the file was found) + tasks.
#[derive(Debug, Clone)]
pub struct ProjectTasks {
    pub root: PathBuf,
    pub tasks: Vec<ProjectTaskConfig>,
}

impl ProjectTaskConfig {
    /// Build the argument vector from both `command` and `args` sources.
    ///
    /// - Tokenizes the whole `command` string via `shell_words::split` (no
    ///   subcommand truncation).
    /// - Appends `args` entries if present.
    /// - If `command` is omitted or whitespace-only and `args` is present,
    ///   returns `args` directly (args-only form).
    /// - Returns `None` on empty/malformed results.
    pub fn argv(&self) -> Option<Vec<String>> {
        #[cfg(feature = "project-tasks")]
        {
            let command = self.command.as_deref().filter(|s| !s.trim().is_empty());
            let mut argv: Vec<String> = Vec::new();
            if let Some(cmd) = command {
                match shell_words::split(cmd) {
                    Ok(tokens) if !tokens.is_empty() => argv = tokens,
                    _ => return None,
                }
            }
            if let Some(args) = &self.args {
                argv.extend(args.iter().cloned());
            }
            if argv.is_empty() { None } else { Some(argv) }
        }
        #[cfg(not(feature = "project-tasks"))]
        {
            None
        }
    }

    /// Whether this task should be visible in the given runtime environment.
    ///
    /// An empty `environments` list means the task is visible everywhere.
    /// Unknown environment strings never match.
    pub fn visible_in(&self, env: Environment) -> bool {
        self.environments.is_empty()
            || self
                .environments
                .iter()
                .any(|e| parse_environment(e.trim()) == Some(env))
    }
}

/// Walk `cwd` and ancestors looking for a tasks file. Returns `Some(ProjectTasks)`
/// if found and parsed (possibly with zero tasks after environment gating),
/// `None` if not found or malformed.
pub fn load_tasks_for_cwd(cwd: &Path) -> Option<ProjectTasks> {
    #[cfg(feature = "project-tasks")]
    {
        let active = active_environment();
        let mut current = Some(cwd);
        while let Some(dir) = current {
            let path = dir.join(TERM_WM_TASKS_PATH);
            if path.is_file() {
                match parse_tasks_file(&path) {
                    Some(tasks) => {
                        return Some(ProjectTasks {
                            root: dir.to_path_buf(),
                            tasks: tasks.into_iter().filter(|t| t.visible_in(active)).collect(),
                        });
                    }
                    None => {
                        tracing::warn!("Failed to parse tasks file: {}", path.display());
                        return None;
                    }
                }
            }
            current = dir.parent();
        }
        None
    }
    #[cfg(not(feature = "project-tasks"))]
    {
        let _ = cwd;
        None
    }
}

#[allow(dead_code)]
fn parse_tasks_file(path: &Path) -> Option<Vec<ProjectTaskConfig>> {
    #[cfg(feature = "project-tasks")]
    {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to read {}: {e}", path.display());
                return None;
            }
        };
        match serde_json::from_str::<Vec<ProjectTaskConfig>>(&content) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("Failed to parse {}: {e}", path.display());
                None
            }
        }
    }
    #[cfg(not(feature = "project-tasks"))]
    {
        let _ = path;
        None
    }
}

#[cfg(all(test, feature = "project-tasks"))]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn parse_flat_array() {
        let json = r#"[
            {"label": "dev: Run", "command": "cargo", "args": ["run"]},
            {"label": "dev: Help", "command": "cargo", "args": ["run", "--", "--help"]}
        ]"#;
        let tasks = serde_json::from_str::<Vec<ProjectTaskConfig>>(json).expect("should parse");
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].label, "dev: Run");
        assert_eq!(tasks[0].command.as_deref(), Some("cargo"));
        assert_eq!(tasks[0].args.as_deref(), Some(&*vec!["run".to_string()]));
    }

    #[test]
    fn rejects_object_envelope() {
        let json = r#"{"tasks": [
            {"label": "build", "command": "cargo", "args": ["build"]}
        ]}"#;
        let result = serde_json::from_str::<Vec<ProjectTaskConfig>>(json);
        assert!(result.is_err(), "object envelope must not parse");
    }

    #[test]
    fn argv_tokenizes_whole_command() {
        let task = ProjectTaskConfig {
            label: "test".into(),
            command: Some("cargo run --example".into()),
            args: Some(vec!["foo".into()]),
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
        };
        assert_eq!(
            task.argv(),
            Some(vec![
                "cargo".into(),
                "run".into(),
                "--example".into(),
                "foo".into()
            ])
        );
    }

    #[test]
    fn argv_args_only_when_command_omitted() {
        let task = ProjectTaskConfig {
            label: "build".into(),
            command: None,
            args: Some(vec!["cargo".into(), "build".into()]),
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
        };
        assert_eq!(task.argv(), Some(vec!["cargo".into(), "build".into()]));
    }

    #[test]
    fn argv_whitespace_command_treated_as_absent() {
        let task = ProjectTaskConfig {
            label: "build".into(),
            command: Some("   ".into()),
            args: Some(vec!["cargo".into(), "check".into()]),
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
        };
        assert_eq!(task.argv(), Some(vec!["cargo".into(), "check".into()]));
    }

    #[test]
    fn argv_none_on_empty_command_no_args() {
        let task = ProjectTaskConfig {
            label: "empty".into(),
            command: Some("   ".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
        };
        assert_eq!(task.argv(), None);
    }

    #[test]
    fn argv_none_on_split_error() {
        let task = ProjectTaskConfig {
            label: "bad".into(),
            command: Some("unbalanced 'quote".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
        };
        assert_eq!(task.argv(), None);
    }

    // ── Environment gating ──────────────────────────────────────────────

    #[test]
    fn visible_in_always_when_empty() {
        let task = ProjectTaskConfig {
            label: "any".into(),
            command: Some("echo".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
        };
        assert!(task.visible_in(Environment::Dev));
        assert!(task.visible_in(Environment::Prod));
        assert!(task.visible_in(Environment::Test));
    }

    #[test]
    fn visible_in_matches_exact() {
        let task = ProjectTaskConfig {
            label: "dev-only".into(),
            command: Some("echo".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: vec!["dev".into()],
        };
        assert!(task.visible_in(Environment::Dev));
        assert!(!task.visible_in(Environment::Prod));
        assert!(!task.visible_in(Environment::Test));
    }

    #[test]
    fn visible_in_matches_multiple_environments() {
        let task = ProjectTaskConfig {
            label: "prod-or-test".into(),
            command: Some("echo".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: vec!["prod".into(), "test".into()],
        };
        assert!(!task.visible_in(Environment::Dev));
        assert!(task.visible_in(Environment::Prod));
        assert!(task.visible_in(Environment::Test));
    }

    #[test]
    fn visible_in_unknown_value_never_matches() {
        let task = ProjectTaskConfig {
            label: "staging-only".into(),
            command: Some("echo".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: vec!["staging".into()],
        };
        assert!(!task.visible_in(Environment::Dev));
        assert!(!task.visible_in(Environment::Prod));
        assert!(!task.visible_in(Environment::Test));
    }

    #[test]
    #[serial(env)]
    fn gating_filters_at_load_dev() {
        let dir = tempfile::tempdir().expect("tempdir failed");
        let tasks_path = dir.path().join(TERM_WM_TASKS_PATH);
        fs::create_dir_all(tasks_path.parent().expect("has parent")).expect("mkdir");
        fs::write(
            &tasks_path,
            r#"[
                {"label": "dev", "command": "echo", "environments": ["dev"]},
                {"label": "all", "command": "echo"}
            ]"#,
        )
        .expect("write");

        // Simulate dev environment (CARGO_MANIFEST_DIR present → Dev).
        unsafe { std::env::set_var("CARGO_MANIFEST_DIR", "/fake/path") };
        let result = load_tasks_for_cwd(dir.path()).expect("load");
        assert_eq!(result.tasks.len(), 2);
        unsafe { std::env::remove_var("CARGO_MANIFEST_DIR") };
    }

    #[test]
    #[serial(env)]
    fn gating_filters_at_load_prod() {
        let dir = tempfile::tempdir().expect("tempdir failed");
        let tasks_path = dir.path().join(TERM_WM_TASKS_PATH);
        fs::create_dir_all(tasks_path.parent().expect("has parent")).expect("mkdir");
        fs::write(
            &tasks_path,
            r#"[
                {"label": "dev", "command": "echo", "environments": ["dev"]},
                {"label": "prod", "command": "echo", "environments": ["prod"]},
                {"label": "all", "command": "echo"}
            ]"#,
        )
        .expect("write");

        unsafe {
            std::env::set_var("TERM_WM_ENV", "prod");
            std::env::remove_var("CARGO_MANIFEST_DIR");
        }
        let result = load_tasks_for_cwd(dir.path()).expect("load");
        assert_eq!(result.tasks.len(), 2);
        assert!(result.tasks.iter().any(|t| t.label == "prod"));
        assert!(result.tasks.iter().any(|t| t.label == "all"));
        assert!(!result.tasks.iter().any(|t| t.label == "dev"));
        unsafe { std::env::remove_var("TERM_WM_ENV") };
    }

    // ── Discovery ───────────────────────────────────────────────────────

    #[test]
    fn malformed_file_stops_walk() {
        let dir = tempfile::tempdir().expect("tempdir failed");
        let tasks_path = dir.path().join(TERM_WM_TASKS_PATH);
        fs::create_dir_all(tasks_path.parent().expect("has parent")).expect("mkdir");
        fs::write(&tasks_path, "{ invalid json").expect("write");
        assert!(load_tasks_for_cwd(dir.path()).is_none());
    }

    #[test]
    fn walks_up_ancestors() {
        let root = tempfile::tempdir().expect("tempdir failed");
        let child = root.path().join("sub").join("deep");
        fs::create_dir_all(&child).expect("mkdir");

        let tasks_path = root.path().join(".term-wm/tasks.json");
        fs::create_dir_all(tasks_path.parent().expect("has parent")).expect("mkdir");
        fs::write(
            &tasks_path,
            r#"[{"label": "from-parent", "command": "cargo"}]"#,
        )
        .expect("write");

        let result = load_tasks_for_cwd(&child).expect("load");
        assert_eq!(result.root, root.path());
        assert_eq!(result.tasks[0].label, "from-parent");
    }

    #[test]
    fn returns_none_when_no_file_anywhere() {
        let dir = tempfile::tempdir().expect("tempdir failed");
        assert!(load_tasks_for_cwd(dir.path()).is_none());
    }

    #[test]
    fn reads_from_term_wm_dir() {
        let dir = tempfile::tempdir().expect("tempdir failed");
        let tasks_path = dir.path().join(".term-wm/tasks.json");
        fs::create_dir_all(tasks_path.parent().expect("has parent")).expect("mkdir");
        fs::write(&tasks_path, r#"[{"label": "twm", "command": "echo"}]"#).expect("write");

        let result = load_tasks_for_cwd(dir.path()).expect("load");
        assert_eq!(result.root, dir.path());
        assert_eq!(result.tasks.len(), 1);
        assert_eq!(result.tasks[0].label, "twm");
    }
}
