//! Locating Claude Code's `projects` directories from an injected environment.

use std::path::{Path, PathBuf};

use crate::cache::ClaudeCodeFileCache;
use crate::tokens::{DailyTokens, DayCalendar};

/// The slice of the process environment discovery needs.
///
/// Callers build it from the real environment at the edge
/// (`USERPROFILE` / `HOME` and `CLAUDE_CONFIG_DIR`); tests build it from
/// temp dirs so they never touch the real `~/.claude`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeEnvironment {
    /// User home directory (`%USERPROFILE%` on Windows).
    pub home: PathBuf,
    /// Raw `CLAUDE_CONFIG_DIR` value, if set.
    pub config_dir: Option<String>,
}

impl ClaudeEnvironment {
    /// An environment with no `CLAUDE_CONFIG_DIR`.
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            config_dir: None,
        }
    }

    /// Sets `CLAUDE_CONFIG_DIR`.
    #[must_use]
    pub fn with_config_dir(mut self, value: impl Into<String>) -> Self {
        self.config_dir = Some(value.into());
        self
    }
}

/// Expands a leading `~` (alone, or followed by `/` or `\`) to `home`.
fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => home.to_path_buf(),
        Some(rest) if rest.starts_with(['/', '\\']) => home.join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

/// Candidate `projects` directories, in scan order.
///
/// When `CLAUDE_CONFIG_DIR` is set and non-blank, each comma-separated entry
/// (trimmed, empties dropped, `~` expanded) plus `projects`. Otherwise
/// `<home>/.config/claude/projects` then `<home>/.claude/projects`; on
/// Windows `home` is `%USERPROFILE%`, so these are
/// `%USERPROFILE%\.config\claude\projects` and `%USERPROFILE%\.claude\projects`.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use tokenmon_provider_claude::{projects_directories, ClaudeEnvironment};
/// let env = ClaudeEnvironment::new("/home/me");
/// assert_eq!(projects_directories(&env), vec![
///     PathBuf::from("/home/me/.config/claude/projects"),
///     PathBuf::from("/home/me/.claude/projects"),
/// ]);
/// let env = ClaudeEnvironment::new("/home/me").with_config_dir("/a, /b");
/// assert_eq!(projects_directories(&env), vec![PathBuf::from("/a/projects"), PathBuf::from("/b/projects")]);
/// ```
pub fn projects_directories(env: &ClaudeEnvironment) -> Vec<PathBuf> {
    let configured = env
        .config_dir
        .as_deref()
        .map(|s| s.trim_matches([' ', '\t']))
        .filter(|s| !s.is_empty());
    match configured {
        Some(list) => list
            .split(',')
            .map(|p| p.trim_matches([' ', '\t']))
            .filter(|p| !p.is_empty())
            .map(|p| expand_tilde(p, &env.home).join("projects"))
            .collect(),
        None => vec![
            env.home.join(".config").join("claude").join("projects"),
            env.home.join(".claude").join("projects"),
        ],
    }
}

/// Token counts per day and model for each of `calendars` (same order) from
/// one walk of the transcripts, or `None` when none of `directories` exists
/// (Claude Code is not installed, so the panel hides its section).
pub fn scan_daily_tokens(
    directories: &[PathBuf],
    calendars: &[DayCalendar],
    cache: &ClaudeCodeFileCache,
) -> Option<Vec<DailyTokens>> {
    let existing: Vec<&PathBuf> = directories.iter().filter(|d| d.is_dir()).collect();
    if existing.is_empty() {
        return None;
    }
    for dir in existing {
        cache.update(dir);
    }
    Some(calendars.iter().map(|c| cache.daily_tokens(*c)).collect())
}
