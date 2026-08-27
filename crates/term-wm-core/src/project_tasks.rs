use std::collections::HashMap;
#[cfg(feature = "project-tasks")]
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(feature = "project-tasks")]
use term_wm_config::env::active_environment;
use term_wm_config::env::{Environment, parse_environment};

/// The sole task file path, resolved from the WM launch directory.
pub const TERM_WM_TASKS_PATH: &str = ".term-wm/tasks.json";

/// Placeholder substituted with the PID of the term-wm process that spawns
/// the task (UI spawner or CLI runner). Lets tasks.json target the WM itself,
/// e.g. `xcrun xctrace record ... --attach {wm.pid}`.
pub const WM_PID_PLACEHOLDER: &str = "{wm.pid}";

/// Placeholder substituted with the full path of the term-wm executable that
/// spawns the task (resolved via `std::env::current_exe()` of the spawning
/// process). Lets tasks.json invoke the binary itself, e.g. piping into
/// `{wm.exe} --util copy`. Prefer passing it through a task `env` entry and
/// referencing `$VAR` inside shell scripts: inline use inside quoted shell
/// text breaks when the resolved path contains quote characters.
pub const WM_EXE_PLACEHOLDER: &str = "{wm.exe}";

/// Canonical platform string for macOS as compared against
/// [`std::env::consts::OS`].
pub const PLATFORM_MACOS: &str = "macos";

/// Accepted alias for [`PLATFORM_MACOS`] in tasks.json `platforms` lists so
/// authors may use either the OS-reported spelling (`macos`) or the classic
/// Darwin spelling (`darwin`).
pub const PLATFORM_DARWIN_ALIAS: &str = "darwin";

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
    /// Platform guard: `None` (or empty) means every platform. Entries are
    /// matched case-insensitively against `std::env::consts::OS`, with
    /// `darwin` accepted as an alias for `macos`.
    #[serde(default)]
    pub platforms: Option<Vec<String>>,
}

/// Variables available during `{...}` placeholder substitution in task strings.
///
/// `pid` is the OS PID of the term-wm process performing the spawn — the UI
/// (WM) process for palette-launched tasks, the CLI process for
/// `--task`-launched ones. `exe` is the path of that same process's
/// executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskVarContext {
    pub pid: u32,
    pub exe: PathBuf,
}

impl Default for TaskVarContext {
    fn default() -> Self {
        Self {
            pid: std::process::id(),
            exe: current_exe_fallback(),
        }
    }
}

/// Resolve this process's executable path, degrading gracefully when the OS
/// cannot supply it (no unwraps): `std::env::current_exe()` first, then
/// argv[0] as reported by the OS, then an empty path (substitution yields an
/// empty string rather than failing the spawn).
fn current_exe_fallback() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| {
        std::env::args_os()
            .next()
            .map_or_else(PathBuf::new, PathBuf::from)
    })
}

/// Substitute registered placeholders in a task string.
///
/// Currently registered: [`WM_PID_PLACEHOLDER`] and [`WM_EXE_PLACEHOLDER`].
/// Unknown `{...}` sequences are left verbatim so future placeholders and
/// literal braces in user commands survive unchanged.
pub fn substitute_vars(input: &str, ctx: &TaskVarContext) -> String {
    input
        .replace(WM_PID_PLACEHOLDER, &ctx.pid.to_string())
        .replace(WM_EXE_PLACEHOLDER, &ctx.exe.to_string_lossy())
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
    ///
    /// Uses a default [`TaskVarContext`] (current process PID and executable);
    /// prefer [`ProjectTaskConfig::argv_resolved`] when the substitution
    /// context is known by the caller.
    pub fn argv(&self) -> Option<Vec<String>> {
        self.argv_resolved(&TaskVarContext::default())
    }

    /// Like [`ProjectTaskConfig::argv`], but substitutes `{...}` placeholders
    /// (`{wm.pid}`, `{wm.exe}`) BEFORE shell-words tokenization so a
    /// substituted value can never be split into stray tokens, even inside
    /// quoted segments.
    pub fn argv_resolved(&self, ctx: &TaskVarContext) -> Option<Vec<String>> {
        #[cfg(feature = "project-tasks")]
        {
            let command = self
                .command
                .as_deref()
                .map(|c| substitute_vars(c, ctx))
                .filter(|s| !s.trim().is_empty());
            let mut argv: Vec<String> = Vec::new();
            if let Some(cmd) = command {
                match shell_words::split(&cmd) {
                    Ok(tokens) if !tokens.is_empty() => argv = tokens,
                    _ => return None,
                }
            }
            if let Some(args) = &self.args {
                argv.extend(args.iter().map(|a| substitute_vars(a, ctx)));
            }
            if argv.is_empty() { None } else { Some(argv) }
        }
        #[cfg(not(feature = "project-tasks"))]
        {
            let (_ctx, _task) = (ctx, self);
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

    /// Whether this task should run on the current platform.
    ///
    /// A missing or empty `platforms` list means every platform. Entries are
    /// matched case-insensitively against [`std::env::consts::OS`], with
    /// `darwin` accepted as an alias for `macos`. Unknown platform strings
    /// simply never match.
    pub fn visible_on_platform(&self) -> bool {
        match &self.platforms {
            None => true,
            Some(list) if list.is_empty() => true,
            Some(list) => list
                .iter()
                .any(|p| normalize_platform(p) == std::env::consts::OS),
        }
    }
}

/// Normalize a tasks.json platform entry: trim, lowercase, and map the
/// `darwin` alias to the canonical `macos` spelling.
fn normalize_platform(raw: &str) -> String {
    let trimmed = raw.trim().to_ascii_lowercase();
    if trimmed == PLATFORM_DARWIN_ALIAS {
        PLATFORM_MACOS.to_string()
    } else {
        trimmed
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
                            tasks: tasks
                                .into_iter()
                                .filter(|t| t.visible_in(active))
                                .filter(|t| t.visible_on_platform())
                                .collect(),
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

#[cfg(feature = "project-tasks")]
fn parse_tasks_str(content: &str) -> Result<Vec<ProjectTaskConfig>, serde_json::Error> {
    use json_comments::StripComments;
    let stripped = StripComments::new(content.as_bytes());
    serde_json::from_reader::<_, Vec<ProjectTaskConfig>>(stripped)
}

/// Fully-resolved execution parameters for a task, shared by the UI spawner
/// (`CommandBuilder`) and the CLI runner (stdio-inheriting `std::process`).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTask {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
}

/// Resolve a task into concrete argv/cwd/env values.
///
/// - `argv`: placeholder-substituted and shell-words-tokenized.
/// - `cwd`: the task's `cwd` when absolute; joined under `root` when relative
///   (substituted first); `base.to_path_buf()` when absent.
/// - `env`: per-task overrides with placeholders substituted in values.
///
/// Returns `None` when the task has no usable command. Mirrors the historical
/// `TermWmApp::command_builder_for_task` semantics with
/// `base = root.unwrap_or(launch_cwd)` — here the caller passes that base
/// explicitly so both entry points share one implementation.
pub fn resolve_task(
    task: &ProjectTaskConfig,
    base: &Path,
    vars: &TaskVarContext,
) -> Option<ResolvedTask> {
    #[cfg(feature = "project-tasks")]
    {
        let argv = task.argv_resolved(vars)?;
        let cwd = match task.cwd.as_deref() {
            Some(c) => {
                let substituted = substitute_vars(c, vars);
                let p = Path::new(&substituted);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    base.join(p)
                }
            }
            None => base.to_path_buf(),
        };
        let env = task
            .env
            .iter()
            .map(|(k, v)| (k.clone(), substitute_vars(v, vars)))
            .collect();
        Some(ResolvedTask { argv, cwd, env })
    }
    #[cfg(not(feature = "project-tasks"))]
    {
        let (_task, _base, _vars) = (task, base, vars);
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
        match parse_tasks_str(&content) {
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
            platforms: None,
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
            platforms: None,
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
            platforms: None,
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
            platforms: None,
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
            platforms: None,
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
            platforms: None,
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
            platforms: None,
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
            platforms: None,
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
            platforms: None,
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
        let _manifest = term_test_support::EnvVarGuard::set("CARGO_MANIFEST_DIR", "/fake/path");
        let result = load_tasks_for_cwd(dir.path()).expect("load");
        assert_eq!(result.tasks.len(), 2);
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

        let _env = term_test_support::EnvVarGuard::set("TERM_WM_ENV", "prod");
        let _manifest_absent = term_test_support::EnvVarGuard::removed("CARGO_MANIFEST_DIR");
        let result = load_tasks_for_cwd(dir.path()).expect("load");
        assert_eq!(result.tasks.len(), 2);
        assert!(result.tasks.iter().any(|t| t.label == "prod"));
        assert!(result.tasks.iter().any(|t| t.label == "all"));
        assert!(!result.tasks.iter().any(|t| t.label == "dev"));
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

    // ── JSON with comments (JSONC) ─────────────────────────────────────

    #[test]
    fn strips_line_comments() {
        let json = r#"
            // header comment
            [
                // task comment
                {"label": "a", "command": "echo hi"} // trailing comment
            ]
            // footer comment
        "#;
        let tasks = parse_tasks_str(json).expect("should parse with line comments");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].label, "a");
    }

    #[test]
    fn strips_block_comments() {
        let json = r#"
            /* header block */
            [
                /* inline */ {"label": "b", "command": "echo hi"} /* trailing */
            ]
        "#;
        let tasks = parse_tasks_str(json).expect("should parse with block comments");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].label, "b");
    }

    #[test]
    fn preserves_comment_like_content_inside_strings() {
        let json = r#"[
            {"label": "url", "command": "echo \"https://example.com\""},
            {"label": "slash", "command": "echo // not a comment"},
            {"label": "block", "command": "echo /* not a comment */"}
        ]"#;
        // Wrapping is unnecessary here; parse_tasks_str with these values
        // should preserve the string contents exactly.
        let tasks = parse_tasks_str(json).expect("strings with // or /* */ must survive");
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[1].command.as_deref(), Some("echo // not a comment"));
        assert_eq!(
            tasks[2].command.as_deref(),
            Some("echo /* not a comment */")
        );
    }

    #[test]
    fn strips_comments_at_load() {
        let dir = tempfile::tempdir().expect("tempdir failed");
        let tasks_path = dir.path().join(TERM_WM_TASKS_PATH);
        fs::create_dir_all(tasks_path.parent().expect("has parent")).expect("mkdir");
        fs::write(
            &tasks_path,
            r#"
            // Project tasks with comments
            [
                {"label": "commented", "command": "echo hello"} // inline
                /* block comment */
            ]
            "#,
        )
        .expect("write");
        let result = load_tasks_for_cwd(dir.path()).expect("load with comments");
        assert_eq!(result.tasks.len(), 1);
        assert_eq!(result.tasks[0].label, "commented");
    }

    #[test]
    fn single_string_command_with_shell_words() {
        // The `command` field is shell-words tokenized, so the full invocation
        // can live in one string without a separate `args` array.
        let task = ProjectTaskConfig {
            label: "run".into(),
            command: Some("cargo run -- --help".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
            platforms: None,
        };
        assert_eq!(
            task.argv(),
            Some(vec![
                "cargo".into(),
                "run".into(),
                "--".into(),
                "--help".into()
            ])
        );
    }

    // ── String substitution ({wm.pid}, {wm.exe}) ────────────────────────

    /// Fixed exe path used by the shared test context; contains a space so
    /// tokenization-sensitive behavior is exercised by default.
    const TEST_EXE: &str = "/opt/my wm/bin/term-wm";

    fn ctx_with_pid(pid: u32) -> TaskVarContext {
        TaskVarContext {
            pid,
            exe: PathBuf::from(TEST_EXE),
        }
    }

    #[test]
    fn substitute_replaces_wm_pid_placeholder() {
        let ctx = ctx_with_pid(4242);
        assert_eq!(substitute_vars("attach {wm.pid}", &ctx), "attach 4242");
        assert_eq!(substitute_vars("{wm.pid}", &ctx), "4242");
        assert_eq!(substitute_vars("no placeholder", &ctx), "no placeholder");
    }

    #[test]
    fn substitute_replaces_wm_exe_placeholder() {
        let ctx = ctx_with_pid(1);
        assert_eq!(
            substitute_vars("git diff | '{wm.exe}' --util copy", &ctx),
            format!("git diff | '{TEST_EXE}' --util copy")
        );
        // Both placeholders coexist in one string.
        assert_eq!(
            substitute_vars("{wm.exe} attach {wm.pid}", &ctx),
            format!("{TEST_EXE} attach 1")
        );
    }

    #[test]
    fn default_ctx_resolves_current_exe() {
        let ctx = TaskVarContext::default();
        let expected = std::env::current_exe().unwrap_or_else(|_| PathBuf::new());
        assert_eq!(ctx.exe, expected);
    }

    #[test]
    fn argv_resolved_substitutes_wm_exe_in_args_without_retok() {
        // `args` elements are substituted AFTER tokenization and never
        // re-split, so an exe path containing spaces stays one argv element.
        let task = ProjectTaskConfig {
            label: "pipe".into(),
            command: Some("sh".into()),
            args: Some(vec!["-c".into(), "git diff | \"{wm.exe}\" --util copy".into()]),
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
            platforms: None,
        };
        let argv = task.argv_resolved(&ctx_with_pid(1)).expect("argv");
        assert_eq!(argv.len(), 3);
        assert_eq!(
            argv[2],
            format!("git diff | \"{TEST_EXE}\" --util copy")
        );
    }

    #[test]
    fn argv_resolved_substitutes_quoted_wm_exe_command_as_one_token() {
        // In `command` form, substitution precedes shell-words splitting, so
        // a quoted '{wm.exe}' stays ONE token even with spaces in the path.
        let task = ProjectTaskConfig {
            label: "direct".into(),
            command: Some("'{wm.exe}' --util copy".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
            platforms: None,
        };
        let argv = task.argv_resolved(&ctx_with_pid(7)).expect("argv");
        assert_eq!(argv, vec![TEST_EXE.to_string(), "--util".into(), "copy".into()]);
    }

    #[test]
    fn resolve_task_substitutes_wm_exe_in_env_values() {
        let task = ProjectTaskConfig {
            label: "env-exe".into(),
            command: Some("sh".into()),
            args: Some(vec!["-c".into(), "git diff | \"$TERM_WM_EXE\" --util copy".into()]),
            cwd: None,
            env: [("TERM_WM_EXE".to_string(), "{wm.exe}".to_string())].into(),
            environments: Vec::new(),
            platforms: None,
        };
        let resolved =
            resolve_task(&task, Path::new("/project"), &ctx_with_pid(3)).expect("resolved");
        assert_eq!(
            resolved.env.get("TERM_WM_EXE").map(String::as_str),
            Some(TEST_EXE)
        );
    }

    #[test]
    fn substitute_leaves_unknown_placeholders() {
        let ctx = ctx_with_pid(1);
        assert_eq!(substitute_vars("{other} {wm.pid}", &ctx), "{other} 1");
        assert_eq!(substitute_vars("{wm.pidX}", &ctx), "{wm.pidX}");
        assert_eq!(substitute_vars("{} {wm.pid", &ctx), "{} {wm.pid");
    }

    #[test]
    fn argv_resolved_substitutes_command_and_args() {
        let task = ProjectTaskConfig {
            label: "profile".into(),
            command: Some("xcrun xctrace record --attach {wm.pid}".into()),
            args: Some(vec!["--output".into(), "/tmp/{wm.pid}.trace".into()]),
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
            platforms: None,
        };
        let argv = task.argv_resolved(&ctx_with_pid(77)).expect("argv");
        assert_eq!(
            argv,
            vec![
                "xcrun".to_string(),
                "xctrace".to_string(),
                "record".to_string(),
                "--attach".to_string(),
                "77".to_string(),
                "--output".to_string(),
                "/tmp/77.trace".to_string()
            ]
        );
    }

    #[test]
    fn substitution_runs_before_shell_words_split() {
        // A quoted segment containing the placeholder must stay ONE token
        // after tokenization — proving substitution precedes splitting.
        let task = ProjectTaskConfig {
            label: "quoted".into(),
            command: Some("trace --template 'Time Profiler {wm.pid}'".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
            platforms: None,
        };
        let argv = task.argv_resolved(&ctx_with_pid(9)).expect("argv");
        assert_eq!(argv.len(), 3);
        assert_eq!(argv[2], "Time Profiler 9");
    }

    #[test]
    fn argv_default_ctx_uses_process_pid_or_none_placeholder_match() {
        // Default-context argv() must behave like argv_resolved with the
        // current process PID when placeholders are absent.
        let task = ProjectTaskConfig {
            label: "plain".into(),
            command: Some("echo hi".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
            platforms: None,
        };
        assert_eq!(task.argv(), task.argv_resolved(&TaskVarContext::default()));
    }

    // ── Platform gating ─────────────────────────────────────────────────

    fn os_str() -> String {
        std::env::consts::OS.to_string()
    }

    #[test]
    fn visible_on_platform_when_list_missing_or_empty() {
        let mk = |platforms: Option<Vec<String>>| ProjectTaskConfig {
            label: "p".into(),
            command: Some("echo".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
            platforms,
        };
        assert!(mk(None).visible_on_platform());
        assert!(mk(Some(Vec::new())).visible_on_platform());
    }

    #[test]
    fn visible_on_platform_matches_current_os_case_insensitively() {
        let task = ProjectTaskConfig {
            label: "native".into(),
            command: Some("echo".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
            platforms: Some(vec![os_str().to_uppercase()]),
        };
        assert!(task.visible_on_platform());
    }

    #[test]
    fn visible_on_platform_hides_foreign_os() {
        let foreign = if os_str() == "linux" {
            "macos"
        } else {
            "linux"
        };
        let task = ProjectTaskConfig {
            label: "foreign".into(),
            command: Some("echo".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
            platforms: Some(vec![foreign.into()]),
        };
        assert!(!task.visible_on_platform());
    }

    #[test]
    fn visible_on_platform_darwin_alias_matches_only_macos() {
        let task = ProjectTaskConfig {
            label: "apple".into(),
            command: Some("echo".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
            platforms: Some(vec!["Darwin".into()]),
        };
        assert_eq!(task.visible_on_platform(), os_str() == PLATFORM_MACOS);
    }

    #[test]
    fn platform_filter_applies_at_load() {
        let dir = tempfile::tempdir().expect("tempdir failed");
        let tasks_path = dir.path().join(TERM_WM_TASKS_PATH);
        fs::create_dir_all(tasks_path.parent().expect("has parent")).expect("mkdir");
        fs::write(
            &tasks_path,
            format!(
                r#"[
                    {{"label": "native", "command": "echo", "platforms": ["{}"]}},
                    {{"label": "foreign", "command": "echo", "platforms": ["not-an-os"]}},
                    {{"label": "all", "command": "echo"}}
                ]"#,
                os_str()
            ),
        )
        .expect("write");

        let result = load_tasks_for_cwd(dir.path()).expect("load");
        let labels: Vec<&str> = result.tasks.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, vec!["native", "all"]);
    }

    // ── resolve_task ────────────────────────────────────────────────────

    #[test]
    fn resolve_task_builds_argv_cwd_env() {
        let task = ProjectTaskConfig {
            label: "t".into(),
            command: Some("attach {wm.pid}".into()),
            args: Some(vec!["{wm.pid}".into()]),
            cwd: Some("logs".into()),
            env: [("OUT".to_string(), "/tmp/{wm.pid}.txt".to_string())].into(),
            environments: Vec::new(),
            platforms: None,
        };
        let base = Path::new("/project");
        let resolved = resolve_task(&task, base, &ctx_with_pid(55)).expect("resolved");
        assert_eq!(
            resolved.argv,
            vec!["attach".to_string(), "55".to_string(), "55".to_string()]
        );
        assert_eq!(resolved.cwd, Path::new("/project/logs"));
        assert_eq!(
            resolved.env.get("OUT").map(String::as_str),
            Some("/tmp/55.txt")
        );
    }

    #[test]
    fn resolve_task_keeps_absolute_cwd() {
        let task = ProjectTaskConfig {
            label: "t".into(),
            command: Some("echo".into()),
            args: None,
            cwd: Some("/abs/dir".into()),
            env: HashMap::new(),
            environments: Vec::new(),
            platforms: None,
        };
        let resolved =
            resolve_task(&task, Path::new("/project"), &ctx_with_pid(1)).expect("resolved");
        assert_eq!(resolved.cwd, Path::new("/abs/dir"));
    }

    #[test]
    fn resolve_task_defaults_cwd_to_base() {
        let task = ProjectTaskConfig {
            label: "t".into(),
            command: Some("echo".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
            platforms: None,
        };
        let resolved = resolve_task(&task, Path::new("/base"), &ctx_with_pid(1)).expect("resolved");
        assert_eq!(resolved.cwd, Path::new("/base"));
    }

    #[test]
    fn resolve_task_none_on_empty_command() {
        let task = ProjectTaskConfig {
            label: "t".into(),
            command: Some("   ".into()),
            args: None,
            cwd: None,
            env: HashMap::new(),
            environments: Vec::new(),
            platforms: None,
        };
        assert!(resolve_task(&task, Path::new("/b"), &ctx_with_pid(1)).is_none());
    }
}

#[cfg(all(test, not(feature = "project-tasks")))]
mod tests_disabled {
    use super::*;

    #[test]
    fn argv_returns_none_when_feature_disabled() {
        let task = ProjectTaskConfig {
            label: "any".into(),
            command: Some("echo hello".into()),
            args: None,
            cwd: None,
            env: std::collections::HashMap::new(),
            environments: Vec::new(),
            platforms: None,
        };
        assert_eq!(
            task.argv(),
            None,
            "argv() must be None when project-tasks feature is disabled"
        );
    }

    #[test]
    fn load_returns_none_when_feature_disabled_even_with_valid_file() {
        let dir = tempfile::tempdir().expect("tempdir failed");
        let tasks_path = dir.path().join(TERM_WM_TASKS_PATH);
        std::fs::create_dir_all(tasks_path.parent().expect("has parent")).expect("mkdir");
        std::fs::write(&tasks_path, r#"[{"label": "x", "command": "echo"}]"#).expect("write");
        assert!(
            load_tasks_for_cwd(dir.path()).is_none(),
            "load_tasks_for_cwd must be None when feature disabled, even with valid file on disk"
        );
    }
}
