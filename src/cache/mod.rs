use std::collections::hash_map::DefaultHasher;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn resolve_git_dir(workspace_dir: &Path) -> Option<PathBuf> {
    let dot_git = workspace_dir.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }

    if dot_git.is_file() {
        let contents = std::fs::read_to_string(&dot_git).ok()?;
        let line = contents.trim();
        let gitdir = line.strip_prefix("gitdir:")?.trim();
        let gitdir_path = Path::new(gitdir);
        if gitdir_path.is_absolute() {
            return Some(gitdir_path.to_path_buf());
        }
        return Some(workspace_dir.join(gitdir_path));
    }

    None
}

pub fn ensure_git_excludes_forge_dir(
    workspace_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let git_dir = match resolve_git_dir(workspace_dir) {
        Some(git_dir) => git_dir,
        None => return Ok(()),
    };

    let exclude_path = git_dir.join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let existing = std::fs::read_to_string(&exclude_path).unwrap_or_default();
    let already_present = existing
        .lines()
        .any(|l| matches!(l.trim(), ".forge/" | ".forge"));
    if already_present {
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_path)?;

    if !existing.is_empty() && !existing.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file, ".forge/")?;

    Ok(())
}

pub fn default_cache_dir(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(".forge").join("cache")
}

pub fn collect_lockfiles(workspace_dir: &Path) -> Vec<PathBuf> {
    let candidates = [
        "Cargo.lock",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "go.sum",
        "poetry.lock",
        "Pipfile.lock",
        "composer.lock",
        "Gemfile.lock",
        "uv.lock",
    ];

    let mut out = Vec::new();
    for name in candidates {
        let p = workspace_dir.join(name);
        if p.is_file() {
            out.push(p);
        }
    }

    if let Ok(entries) = std::fs::read_dir(workspace_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name.starts_with("requirements") && name.ends_with(".txt") {
                out.push(p);
            }
        }
    }

    out.sort();
    out
}

pub fn compute_cache_key(workspace_dir: &Path) -> String {
    let lockfiles = collect_lockfiles(workspace_dir);
    if lockfiles.is_empty() {
        return "default".to_string();
    }

    let mut hasher = DefaultHasher::new();
    "forge-cache-key-v1".hash(&mut hasher);
    for path in &lockfiles {
        if let Ok(rel) = path.strip_prefix(workspace_dir) {
            rel.to_string_lossy().hash(&mut hasher);
        } else {
            path.to_string_lossy().hash(&mut hasher);
        }

        if let Ok(bytes) = std::fs::read(path) {
            bytes.hash(&mut hasher);
        }
    }

    format!("{:016x}", hasher.finish())
}
