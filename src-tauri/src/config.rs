use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ConfigRoots {
    /// Not read anywhere yet; Phase 2 (session registry) consumes it.
    #[allow(dead_code)]
    pub claude_root: PathBuf,
    pub projects: PathBuf,
    pub sessions: PathBuf,
    pub stats_cache: PathBuf,
}

/// Precedence: explicit setting > CLAUDE_CONFIG_DIR > ~/.claude.
/// The seam exists so tests never depend on the machine's real env or home.
pub fn resolve_with(
    explicit: Option<PathBuf>,
    env_value: Option<&str>,
    home: Option<PathBuf>,
) -> ConfigRoots {
    let claude_root = explicit
        .or_else(|| {
            env_value
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| match home {
            Some(h) => h.join(".claude"),
            None => PathBuf::from(".claude"),
        });

    ConfigRoots {
        projects: claude_root.join("projects"),
        sessions: claude_root.join("sessions"),
        stats_cache: claude_root.join("stats-cache.json"),
        claude_root,
    }
}

pub fn resolve(explicit: Option<PathBuf>) -> ConfigRoots {
    let env_value = std::env::var("CLAUDE_CONFIG_DIR").ok();
    resolve_with(explicit, env_value.as_deref(), dirs::home_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn explicit_override_wins_over_everything() {
        let roots = resolve_with(Some(PathBuf::from("/custom/root")), Some("/env/root"), Some(PathBuf::from("/home/me")));
        assert_eq!(roots.claude_root, PathBuf::from("/custom/root"));
        assert_eq!(roots.projects, PathBuf::from("/custom/root/projects"));
        assert_eq!(roots.sessions, PathBuf::from("/custom/root/sessions"));
        assert_eq!(roots.stats_cache, PathBuf::from("/custom/root/stats-cache.json"));
    }

    #[test]
    fn env_var_used_when_no_explicit_override() {
        let roots = resolve_with(None, Some("/env/root"), Some(PathBuf::from("/home/me")));
        assert_eq!(roots.claude_root, PathBuf::from("/env/root"));
    }

    #[test]
    fn falls_back_to_home_dot_claude() {
        let roots = resolve_with(None, None, Some(PathBuf::from("/home/me")));
        assert_eq!(roots.claude_root, PathBuf::from("/home/me/.claude"));
        assert_eq!(roots.projects, PathBuf::from("/home/me/.claude/projects"));
    }

    #[test]
    fn empty_env_var_is_ignored() {
        let roots = resolve_with(None, Some(""), Some(PathBuf::from("/home/me")));
        assert_eq!(roots.claude_root, PathBuf::from("/home/me/.claude"));
    }

    #[test]
    fn missing_home_yields_relative_fallback_without_panicking() {
        let roots = resolve_with(None, None, None);
        assert_eq!(roots.claude_root, PathBuf::from(".claude"));
    }
}
