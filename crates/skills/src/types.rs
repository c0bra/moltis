use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── Skills manifest ──────────────────────────────────────────────────────────

/// Top-level manifest tracking installed repos and per-skill enabled state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsManifest {
    pub version: u32,
    #[serde(default)]
    pub repos: Vec<RepoEntry>,
}

impl Default for SkillsManifest {
    fn default() -> Self {
        Self {
            version: 1,
            repos: Vec::new(),
        }
    }
}

impl SkillsManifest {
    pub fn add_repo(&mut self, entry: RepoEntry) {
        self.repos.push(entry);
    }

    pub fn remove_repo(&mut self, source: &str) {
        self.repos.retain(|r| r.source != source);
    }

    pub fn find_repo(&self, source: &str) -> Option<&RepoEntry> {
        self.repos.iter().find(|r| r.source == source)
    }

    pub fn find_repo_mut(&mut self, source: &str) -> Option<&mut RepoEntry> {
        self.repos.iter_mut().find(|r| r.source == source)
    }

    pub fn set_skill_enabled(&mut self, source: &str, skill_name: &str, enabled: bool) -> bool {
        if let Some(repo) = self.find_repo_mut(source)
            && let Some(skill) = repo.skills.iter_mut().find(|s| s.name == skill_name)
        {
            skill.enabled = enabled;
            return true;
        }
        false
    }
}

/// A single cloned repository with its discovered skills.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntry {
    pub source: String,
    pub repo_name: String,
    pub installed_at_ms: u64,
    pub skills: Vec<SkillState>,
}

/// Per-skill enabled state within a repo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillState {
    pub name: String,
    pub relative_path: String,
    pub enabled: bool,
}

// ── Skill metadata ───────────────────────────────────────────────────────────

/// Where a skill was discovered from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    /// Project-local: `<cwd>/.moltis/skills/`
    Project,
    /// Personal: `~/.moltis/skills/`
    Personal,
    /// Bundled inside a plugin directory.
    Plugin,
    /// Installed from a registry (e.g. skills.sh).
    Registry,
}

/// Lightweight metadata parsed from SKILL.md frontmatter.
/// Loaded at startup for all discovered skills (cheap).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Skill name — lowercase, hyphens allowed, 1-64 chars.
    pub name: String,
    /// Short human-readable description.
    #[serde(default)]
    pub description: String,
    /// SPDX license identifier.
    #[serde(default)]
    pub license: Option<String>,
    /// Tools this skill is allowed to use.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Filesystem path to the skill directory.
    #[serde(skip)]
    pub path: PathBuf,
    /// Where this skill was discovered.
    #[serde(skip)]
    pub source: Option<SkillSource>,
}

/// Full skill content: metadata + markdown body.
/// Loaded on demand when a skill is activated.
#[derive(Debug, Clone)]
pub struct SkillContent {
    pub metadata: SkillMetadata,
    pub body: String,
}
