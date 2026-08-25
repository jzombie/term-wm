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

/// Usage line for the `copy` utility (the only utility taking positionals).
const COPY_USAGE: &str = "usage: term-wm --util copy [FILE]";

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

/// Dispatch a `--util <UTIL>` invocation and return the process exit code.
///
/// `args` is the utility argument vector (the CLI positional slot after
/// `--`). Each utility validates its own arity; excess arguments are a usage
/// error printed to stderr with [`COPY_EXIT_FAILURE`].
pub fn run_util(action: UtilAction, args: &[String]) -> i32 {
    match action {
        UtilAction::Copy => match copy_file_arg(args) {
            Ok(file) => term_clipboard::run_copy_util(file, UTIL_LABEL),
            Err(code) => code,
        },
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
        assert_eq!(
            copy_file_arg(&one),
            Ok(Some(PathBuf::from("diff.patch")))
        );
    }
}
