//! Skills system for Zunesha autonomous agents.
//!
//! Specific skills can be installed to specific crew agents (e.g. Zoro, Chopper, Luffy).
//! When an agent runs a task, its installed skills are resolved and injected into its
//! prompt and isolated workspace.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::{Error, Result};

/// A skill with instructions and metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skill {
    /// Unique identifier for this skill (e.g. "tdd-scout", "debt-slayer").
    pub id: String,
    /// Human-friendly name.
    pub name: String,
    /// Short summary of what this skill enables.
    pub description: String,
    /// Detailed instructional prompt or system guidelines.
    pub instructions: String,
    /// Where this skill originated.
    pub source: SkillSource,
}

/// The origin of a skill.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Shipped directly with Zunesha.
    Builtin,
    /// Stored in repository `.zou/skills/` or `.skills/`.
    Project,
    /// Stored in user global configuration.
    User,
}

/// Information about a skill including which agents have it installed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillInfo {
    #[serde(flatten)]
    pub skill: Skill,
    /// Crew members who currently have this skill installed.
    pub installed_to: Vec<String>,
}

/// Canonical built-in skills available out of the box.
pub fn builtin_skills() -> Vec<Skill> {
    vec![
        Skill {
            id: "tdd-scout".to_string(),
            name: "TDD & Edge-Case Scout".to_string(),
            description: "Test-driven development, boundary condition probing, fuzz assertions & zero-regression rigor.".to_string(),
            instructions: "### Skill: TDD & Edge-Case Scout\n\
                - Before writing implementation code, analyze boundary conditions, empty inputs, extreme values, and concurrency hazards.\n\
                - Write comprehensive tests covering both happy paths and error conditions.\n\
                - Ensure all test assertions are deterministic, clear, and failure-descriptive.\n\
                - Never delete or weaken existing tests to make new code pass.".to_string(),
            source: SkillSource::Builtin,
        },
        Skill {
            id: "debt-slayer".to_string(),
            name: "Debt & Complexity Slayer".to_string(),
            description: "Aggressive dead code elimination, AST cleanup, compiler warning eradication & clean abstractions.".to_string(),
            instructions: "### Skill: Debt & Complexity Slayer\n\
                - Ruthlessly eliminate dead code, unused parameters, redundant clones, and deprecated patterns.\n\
                - Slay compiler and linter warnings with zero tolerance.\n\
                - Simplify nested conditionals with guard clauses and early returns.\n\
                - Preserve existing external behaviors and public contracts while simplifying internals.".to_string(),
            source: SkillSource::Builtin,
        },
        Skill {
            id: "system-navigator".to_string(),
            name: "System Architecture Navigator".to_string(),
            description: "DAG route design, clean module boundaries, API contract versioning & non-breaking migrations.".to_string(),
            instructions: "### Skill: System Architecture Navigator\n\
                - Analyze dependencies before proposing changes to prevent cyclic couplings.\n\
                - Design clean, decoupled interface boundaries between modules.\n\
                - Favor backward-compatible migrations and additive API evolution.\n\
                - Document architectural decisions and data flow invariants explicitly.".to_string(),
            source: SkillSource::Builtin,
        },
        Skill {
            id: "ui-artisan".to_string(),
            name: "UI/UX & Design Artisan".to_string(),
            description: "Semantic HTML, responsive Tailwind styling, micro-interactions, accessibility & brutalist aesthetics.".to_string(),
            instructions: "### Skill: UI/UX & Design Artisan\n\
                - Build responsive, accessible, keyboard-navigable interfaces matching the application design system.\n\
                - Ensure crisp visual feedback on hover, focus, active, and disabled states.\n\
                - Optimize typography, contrast ratios, and spacing rhythm.\n\
                - Protect layouts from text overflow and layout shifts.".to_string(),
            source: SkillSource::Builtin,
        },
        Skill {
            id: "crash-medic".to_string(),
            name: "Crash Diagnostics & Medic".to_string(),
            description: "Panic stack trace triage, unwrap eradication, graceful error propagation & state self-healing.".to_string(),
            instructions: "### Skill: Crash Diagnostics & Medic\n\
                - Diagnose crashes and panics down to root cause using logs and traces.\n\
                - Replace brittle unwrap/expect calls with idiomatic error propagation.\n\
                - Ensure corrupt states or interrupted processes recover gracefully to clean baselines.\n\
                - Add regression tests proving the crash condition cannot recur.".to_string(),
            source: SkillSource::Builtin,
        },
        Skill {
            id: "git-archaeologist".to_string(),
            name: "Git Archaeology & Context".to_string(),
            description: "Historical commit archaeology, blame synthesis, Architecture Decision Records (ADRs) & changelogs.".to_string(),
            instructions: "### Skill: Git Archaeology & Context\n\
                - Use git history and blame to understand why code was originally written this way before altering it.\n\
                - Synthesize historical context into clear Architectural Decision Records (ADRs).\n\
                - Produce atomic, beautifully documented commits with explanatory summaries.\n\
                - Maintain changelog integrity and documentation accuracy.".to_string(),
            source: SkillSource::Builtin,
        },
        Skill {
            id: "container-forge".to_string(),
            name: "Container & Superbuild Forge".to_string(),
            description: "Dockerfile multi-stage optimization, CI/CD GitHub Actions workflows, cross-platform build tooling.".to_string(),
            instructions: "### Skill: Container & Superbuild Forge\n\
                - Optimize container build caching, image layers, and minimal runtime footprints.\n\
                - Construct robust CI/CD workflows with automated linting, testing, and artifact publishing.\n\
                - Ensure scripts and toolchains are portable across Linux, macOS, and WSL environments.\n\
                - Pin dependencies and enforce reproducible builds.".to_string(),
            source: SkillSource::Builtin,
        },
        Skill {
            id: "perf-harmonizer".to_string(),
            name: "Performance & Cadence Harmonizer".to_string(),
            description: "Allocation profiling, zero-copy hot-loops, latency reduction & benchmark harness design.".to_string(),
            instructions: "### Skill: Performance & Cadence Harmonizer\n\
                - Profile hot execution paths before and after optimization to measure real improvements.\n\
                - Minimize heap allocations by reusing buffers and taking slices/references where practical.\n\
                - Eliminate unnecessary locks, quadratic loops, and I/O serialization bottlenecks.\n\
                - Add micro-benchmarks to guard against latency regressions.".to_string(),
            source: SkillSource::Builtin,
        },
        Skill {
            id: "rebase-helmsman".to_string(),
            name: "Rebase & Merge Helmsman".to_string(),
            description: "3-way merge conflict disambiguation, semantic git rebasing, clean commit curation & release safety.".to_string(),
            instructions: "### Skill: Rebase & Merge Helmsman\n\
                - Resolve 3-way merge conflicts by reconciling semantic intent rather than blindly keeping one side.\n\
                - Rebase branches cleanly onto updated base commits with zero lost work.\n\
                - Verify build and tests pass after resolving conflicts before committing.\n\
                - Keep branch history clean, logical, and linear.".to_string(),
            source: SkillSource::Builtin,
        },
        Skill {
            id: "security-sentinel".to_string(),
            name: "Security & Secret Sentinel".to_string(),
            description: "Input sanitization, secret leak prevention, path traversal guarding & boundary hardening.".to_string(),
            instructions: "### Skill: Security & Secret Sentinel\n\
                - Audit all external inputs against injection, traversal, and unbounded memory consumption.\n\
                - Never log or persist API keys, tokens, credentials, or sensitive headers.\n\
                - Enforce strict principle of least privilege in processes, file permissions, and environment variables.\n\
                - Validate data boundaries with defensive parsing.".to_string(),
            source: SkillSource::Builtin,
        },
    ]
}

/// Discover all available skills (built-ins + project skills + user skills).
pub fn list_all_skills(repo_root: Option<&Path>) -> Vec<Skill> {
    let mut skills_by_id = BTreeMap::new();

    // 1. Built-in skills
    for skill in builtin_skills() {
        skills_by_id.insert(skill.id.clone(), skill);
    }

    // 2. Project skills (from .zou/skills/<skill-id>/SKILL.md or .skills/<skill-id>/SKILL.md)
    if let Some(root) = repo_root {
        for candidate_dir in &[root.join(".zou").join("skills"), root.join(".skills")] {
            if let Ok(entries) = fs::read_dir(candidate_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let skill_file = path.join("SKILL.md");
                        if skill_file.is_file() {
                            if let Ok(skill) = load_skill_file(&skill_file, SkillSource::Project) {
                                skills_by_id.insert(skill.id.clone(), skill);
                            }
                        }
                    }
                }
            }
        }
    }

    skills_by_id.into_values().collect()
}

/// Load a skill from a `SKILL.md` file.
pub fn load_skill_file(path: &Path, source: SkillSource) -> Result<Skill> {
    let content = fs::read_to_string(path)?;
    let parent_id = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("custom-skill")
        .to_ascii_lowercase();

    parse_skill_content(&parent_id, &content, source)
}

/// Parse skill markdown content (with or without YAML frontmatter).
pub fn parse_skill_content(id: &str, content: &str, source: SkillSource) -> Result<Skill> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(Error::Config("skill content is empty".to_string()));
    }

    let mut name = id.replace(['-', '_'], " ");
    let mut description = format!("Custom skill: {name}");
    let mut instructions = trimmed.to_string();

    // Parse YAML frontmatter if present:
    // ---
    // name: ...
    // description: ...
    // ---
    if trimmed.starts_with("---") {
        if let Some(end_idx) = trimmed[3..].find("---") {
            let frontmatter = &trimmed[3..end_idx + 3];
            let body = trimmed[end_idx + 6..].trim();
            instructions = body.to_string();

            for line in frontmatter.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("name:") {
                    let n = rest.trim().trim_matches('"').trim_matches('\'').trim();
                    if !n.is_empty() {
                        name = n.to_string();
                    }
                } else if let Some(rest) = line.strip_prefix("description:") {
                    let d = rest.trim().trim_matches('"').trim_matches('\'').trim();
                    if !d.is_empty() {
                        description = d.to_string();
                    }
                }
            }
        }
    }

    Ok(Skill {
        id: id.to_ascii_lowercase(),
        name,
        description,
        instructions,
        source,
    })
}

/// Resolve all skills installed to a specific crew member.
pub fn resolve_skills_for_member(
    config: &Config,
    member: &str,
    repo_root: Option<&Path>,
) -> Vec<Skill> {
    let installed_ids = config.resolve_crew_skills(member);
    if installed_ids.is_empty() {
        return Vec::new();
    }

    let all_skills = list_all_skills(repo_root);
    let skill_map: BTreeMap<String, Skill> =
        all_skills.into_iter().map(|s| (s.id.clone(), s)).collect();

    let mut result = Vec::new();
    for id in installed_ids {
        if let Some(skill) = skill_map.get(&id.to_ascii_lowercase()) {
            result.push(skill.clone());
        }
    }
    result
}

/// Create a new project skill in `.zou/skills/<skill-id>/SKILL.md`.
pub fn create_project_skill(
    repo_root: &Path,
    id: &str,
    name: &str,
    description: &str,
    instructions: &str,
) -> Result<PathBuf> {
    let clean_id = id
        .trim()
        .to_ascii_lowercase()
        .replace(|c: char| !c.is_alphanumeric() && c != '-', "-");
    if clean_id.is_empty() {
        return Err(Error::Config("skill id cannot be empty".to_string()));
    }

    let skill_dir = repo_root.join(".zou").join("skills").join(&clean_id);
    fs::create_dir_all(&skill_dir)?;
    let skill_file = skill_dir.join("SKILL.md");

    let content = format!(
        "---\nname: \"{}\"\ndescription: \"{}\"\n---\n\n{}",
        name.trim(),
        description.trim(),
        instructions.trim()
    );

    fs::write(&skill_file, content)?;
    Ok(skill_file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_skills_have_unique_ids_and_instructions() {
        let skills = builtin_skills();
        assert!(skills.len() >= 10);
        let mut ids = std::collections::HashSet::new();
        for skill in skills {
            assert!(!skill.id.is_empty());
            assert!(!skill.name.is_empty());
            assert!(!skill.description.is_empty());
            assert!(!skill.instructions.is_empty());
            assert!(ids.insert(skill.id));
        }
    }

    #[test]
    fn parse_skill_with_frontmatter() {
        let md = "---\nname: Security Auditor\ndescription: Checks for vulnerabilities\n---\n\nAlways sanitize user inputs.";
        let skill = parse_skill_content("sec-audit", md, SkillSource::Project).unwrap();
        assert_eq!(skill.id, "sec-audit");
        assert_eq!(skill.name, "Security Auditor");
        assert_eq!(skill.description, "Checks for vulnerabilities");
        assert_eq!(skill.instructions, "Always sanitize user inputs.");
    }

    #[test]
    fn parse_skill_without_frontmatter() {
        let md = "Direct instructions without frontmatter.";
        let skill = parse_skill_content("quick_fix", md, SkillSource::User).unwrap();
        assert_eq!(skill.id, "quick_fix");
        assert_eq!(skill.name, "quick fix");
        assert_eq!(skill.instructions, "Direct instructions without frontmatter.");
    }
}
