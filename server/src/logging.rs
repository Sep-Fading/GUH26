// Chooses which of MiniClaw's own diagnostic events reach stderr.
use tracing_subscriber::{Layer, filter::{LevelFilter, Targets}};

/// Names the variable that sets the diagnostic level.
pub const VARIABLE: &str = "MINICLAW_LOG";

/// Returns the filter for `MINICLAW_LOG`, or `None` when nothing should be printed.
/// The value is one level word. Unset means warnings and errors on Windows, whose sandbox
/// cleanup reports problems only this way, and nothing on macOS, which printed none before.
/// Only MiniClaw's own targets pass: dependency crates (HTTP, TLS) can log headers or bodies.
pub fn filter(value: Option<&str>, windows: bool) -> Result<Option<Targets>, String> {
    let level = match value.map(|value| value.trim().to_ascii_lowercase()).as_deref() {
        None if windows => LevelFilter::WARN,
        None | Some("off") => return Ok(None),
        Some("error") => LevelFilter::ERROR,
        Some("warn") => LevelFilter::WARN,
        Some("info") => LevelFilter::INFO,
        Some("debug") => LevelFilter::DEBUG,
        Some("trace") => LevelFilter::TRACE,
        Some(_) => return Err(format!("{VARIABLE} must be one of off, error, warn, info, debug, trace")),
    };
    Ok(Some(Targets::new().with_target("miniclaw_server", level)))
}

/// Installs the stderr writer for MiniClaw's diagnostic events from `MINICLAW_LOG`.
/// Returns an error naming the variable when its value is not a level word.
pub fn install() -> Result<(), String> {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    let value = std::env::var(VARIABLE).ok();
    let Some(targets) = filter(value.as_deref(), cfg!(windows))? else { return Ok(()) };
    // Plain text: on Windows stderr is usually a launcher pipe or a redirected file.
    let writer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr).with_ansi(false);
    tracing_subscriber::registry().with(writer.with_filter(targets)).try_init()
        .map_err(|_| "Diagnostic logging could not be installed".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::Level;

    // Reports whether a filter lets one event of a target and level through.
    fn passes(filter: &Option<Targets>, target: &str, level: Level) -> bool {
        filter.as_ref().is_some_and(|filter| filter.would_enable(target, &level))
    }

    #[test]
    fn default_is_warnings_on_windows_and_silent_on_macos() {
        let windows = filter(None, true).unwrap();
        assert!(passes(&windows, "miniclaw_server::attachments", Level::WARN));
        assert!(passes(&windows, "miniclaw_server", Level::ERROR));
        assert!(!passes(&windows, "miniclaw_server::attachments", Level::INFO));
        assert!(filter(None, false).unwrap().is_none());
    }

    #[test]
    fn a_level_word_applies_only_to_miniclaw() {
        let debug = filter(Some("debug"), false).unwrap();
        assert!(passes(&debug, "miniclaw_server::windows_sandbox_journal", Level::DEBUG));
        assert!(!passes(&debug, "miniclaw_server", Level::TRACE));
        for dependency in ["hyper", "reqwest", "rustls", "h2", "tokio"] {
            assert!(!passes(&debug, dependency, Level::ERROR), "{dependency}");
        }
        assert!(filter(Some("off"), true).unwrap().is_none());
        assert!(passes(&filter(Some(" Info "), true).unwrap(), "miniclaw_server", Level::INFO));
    }

    #[test]
    fn other_values_are_refused() {
        for value in ["", "verbose", "miniclaw_server=debug", "debug,hyper=trace", "warn,"] {
            let error = filter(Some(value), true).unwrap_err();
            assert!(error.contains(VARIABLE) && error.contains("off, error, warn, info, debug, trace"), "{value}: {error}");
        }
    }
}
