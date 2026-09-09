//! Built-in utility dispatch for `--util <UTIL>`.
//!
//! Utilities are small headless helpers that run and exit before any window
//! manager, session, or gateway machinery starts. They exist so scripts and
//! project-task pipelines (`tasks.json`) can drive the term-wm binary itself.
//! The positional arguments captured by the CLI's trailing `--` var-arg slot
//! are forwarded here as each utility's argument vector.

use std::path::PathBuf;

use term_clipboard::COPY_EXIT_FAILURE;

use crate::cli::UtilAction;

/// Program label used in utility usage/error messages.
const UTIL_LABEL: &str = env!("CARGO_PKG_NAME");

/// Usage line for the `copy` utility.
const COPY_USAGE: &str = "usage: term-wm --util copy [--force-osc52] [FILE]";

/// Flag opting into OSC 52 emission even when stdout is not a terminal.
/// For copies whose stdout is captured by a framework that forwards bytes
/// verbatim to a real terminal (e.g. an OxDock `RUN` step), restoring the
/// terminal-emulator clipboard path that the TTY gate would otherwise skip.
const COPY_FORCE_OSC52_ARG: &str = "--force-osc52";

/// Usage line for the `oxdock` utility (exactly one script PATH in v1).
#[cfg(feature = "oxdock")]
const OXDOCK_USAGE: &str = "usage: term-wm --util oxdock <PATH>";

/// Error reported when `--util oxdock` is invoked without the `oxdock`
/// cargo feature. The enum variant stays unconditional so every feature
/// combination parses identically; gating lives here at runtime.
#[cfg(not(feature = "oxdock"))]
const OXDOCK_REQUIRES_FEATURE: &str = "error: '--util oxdock' requires the 'oxdock' feature";

/// Number of positional arguments accepted by the `oxdock` utility: the
/// script path only. OxDock scripts take no argv of their own, so trailing
/// extras are a usage error rather than silent dead input.
#[cfg(feature = "oxdock")]
const OXDOCK_POSITIONAL_ARGS: usize = 1;

/// Number of positional arguments accepted by the `copy` utility.
const COPY_MAX_POSITIONAL_ARGS: usize = 1;

/// Map the `copy` utility's argument vector to its optional FILE argument.
///
/// Zero positionals means piped stdin; exactly one names a file. Anything
/// beyond [`COPY_MAX_POSITIONAL_ARGS`] is a usage error (reported on stderr,
/// mapped to [`COPY_EXIT_FAILURE`]).
fn copy_file_arg(args: &[String]) -> Result<Option<PathBuf>, i32> {
    match args.len() {
        0 => Ok(None),
        COPY_MAX_POSITIONAL_ARGS => Ok(Some(PathBuf::from(&args[0]))),
        _ => {
            eprintln!("{UTIL_LABEL}: {COPY_USAGE}");
            Err(COPY_EXIT_FAILURE)
        }
    }
}

/// Split the `copy` argument vector into its `--force-osc52` flag and the
/// remaining positional arguments (the optional FILE).
fn copy_force_osc52(args: &[String]) -> (bool, Vec<String>) {
    let mut rest = Vec::with_capacity(args.len());
    let mut force = false;
    for arg in args {
        if arg == COPY_FORCE_OSC52_ARG {
            force = true;
        } else {
            rest.push(arg.clone());
        }
    }
    (force, rest)
}

/// Dispatch a `--util <UTIL>` invocation and return the process exit code.
///
/// `args` is the utility argument vector (the CLI positional slot after
/// `--`). Each utility validates its own arity; excess arguments are a usage
/// error printed to stderr with [`COPY_EXIT_FAILURE`].
pub fn run_util(action: UtilAction, args: &[String]) -> i32 {
    match action {
        UtilAction::Copy => {
            let (force, rest) = copy_force_osc52(args);
            match copy_file_arg(&rest) {
                Ok(file) => {
                    // `#[non_exhaustive]` forbids struct expressions outside
                    // `term-clipboard`; configure via default + assignment.
                    let mut config = term_clipboard::ClipboardConfig::default();
                    config.osc52_force = force;
                    term_clipboard::run_copy_util_with_config(file, UTIL_LABEL, config)
                }
                Err(code) => code,
            }
        }
        UtilAction::Oxdock => run_oxdock_util(args),
    }
}

/// Dispatch a `--util oxdock <PATH>` invocation.
///
/// Feature-off builds print the requires-feature error (the variant parses
/// unconditionally); otherwise exactly one script path is accepted and any
/// evaluated script code flows back as the process exit code.
fn run_oxdock_util(args: &[String]) -> i32 {
    #[cfg(not(feature = "oxdock"))]
    {
        let _ = args;
        eprintln!("{UTIL_LABEL}: {OXDOCK_REQUIRES_FEATURE}");
        COPY_EXIT_FAILURE
    }
    #[cfg(feature = "oxdock")]
    {
        if args.len() != OXDOCK_POSITIONAL_ARGS {
            eprintln!("{UTIL_LABEL}: {OXDOCK_USAGE}");
            return COPY_EXIT_FAILURE;
        }
        // Resolve `{wm.pid}`/`{wm.exe}` in the path itself only; the script
        // body keeps OxDock's own `{{ env:KEY }}` spelling (see oxdock_runner).
        let vars = term_wm_core::project_tasks::TaskVarContext::default();
        let resolved = term_wm_core::project_tasks::substitute_vars(&args[0], &vars);
        match crate::oxdock_runner::run_oxdock_script(std::path::Path::new(&resolved)) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("{UTIL_LABEL}: {e}");
                COPY_EXIT_FAILURE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extra positionals are rejected as a usage failure before any clipboard
    /// work is attempted.
    #[test]
    fn copy_rejects_more_than_one_positional() {
        let extra = vec!["a.txt".to_string(), "b.txt".to_string()];
        assert_eq!(copy_file_arg(&extra), Err(COPY_EXIT_FAILURE));
    }

    #[test]
    fn copy_accepts_zero_positionals_as_stdin_mode() {
        assert_eq!(copy_file_arg(&[]), Ok(None));
    }

    #[test]
    fn copy_maps_single_positional_to_file() {
        let one = vec!["diff.patch".to_string()];
        assert_eq!(copy_file_arg(&one), Ok(Some(PathBuf::from("diff.patch"))));
    }

    /// The force flag separates from positionals in any order, leaving the
    /// FILE mapping untouched.
    #[test]
    fn copy_force_flag_splits_from_positionals() {
        let (force, rest) = copy_force_osc52(&[]);
        assert!(!force);
        assert!(rest.is_empty());
        let (force, rest) = copy_force_osc52(&["--force-osc52".to_string()]);
        assert!(force);
        assert!(rest.is_empty());
        let (force, rest) = copy_force_osc52(&["a.txt".to_string(), "--force-osc52".to_string()]);
        assert!(force);
        assert_eq!(rest, vec!["a.txt".to_string()]);
    }

    /// Flag plus file still dispatches (missing file fails at ingestion).
    #[test]
    fn copy_force_flag_with_file_runs() {
        let args = vec![
            "--force-osc52".to_string(),
            "testdata/does-not-exist".to_string(),
        ];
        assert_eq!(run_util(UtilAction::Copy, &args), COPY_EXIT_FAILURE);
    }

    /// Zero or multiple positionals are usage errors, mirroring `copy`'s
    /// arity discipline (OxDock scripts take no argv of their own in v1).
    #[test]
    fn oxdock_rejects_wrong_positional_count() {
        assert_eq!(run_oxdock_util(&[]), COPY_EXIT_FAILURE);
        let two = vec!["a.oxfile".to_string(), "b.oxfile".to_string()];
        assert_eq!(run_oxdock_util(&two), COPY_EXIT_FAILURE);
    }

    /// A missing script file surfaces the failure code without panicking.
    #[test]
    fn oxdock_missing_script_fails() {
        let one = vec!["testdata/does-not-exist.oxfile".to_string()];
        assert_eq!(run_oxdock_util(&one), COPY_EXIT_FAILURE);
    }

    /// Feature-off builds reject `--util oxdock` with the explicit
    /// requires-feature error instead of a generic clap rejection.
    #[test]
    #[cfg(not(feature = "oxdock"))]
    fn oxdock_requires_feature_when_disabled() {
        let one = vec!["demo.oxfile".to_string()];
        assert_eq!(run_oxdock_util(&one), COPY_EXIT_FAILURE);
    }
}
