//! Filesystem helpers for config drivers — the `AgentConfig` walkers every
//! adapter needs (`*.md` dirs, `<name>/SKILL.md` packages). Sync std::fs:
//! driver processes are single-purpose, these reads are small.

use std::path::{Path, PathBuf};

use sepia_core::agent_config::{
    ConfigFile, ConfigSkill, ConfigWriteAction, WriteActionKind, safe_file_stem, skill_attributes,
};
use sepia_core::domain::StorageError;
use sepia_core::frontmatter;
use serde_json::{Map, Value};

fn fs_error(prefix: &str) -> impl Fn(std::io::Error) -> StorageError + '_ {
    move |cause| StorageError::new(format!("{prefix}: {cause}"))
}

/// Read a file as UTF-8, or `None` when absent; other errors propagate.
pub fn read_file_if_exists(path: &Path) -> Result<Option<String>, StorageError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => Ok(None),
        Err(e) => Err(fs_error(&format!("Failed to read {}", path.display()))(e)),
    }
}

/// Read + JSON.parse a file, or `None` when absent/unparseable/non-object.
pub fn read_json_if_exists(path: &Path) -> Result<Option<Map<String, Value>>, StorageError> {
    let Some(text) = read_file_if_exists(path)? else {
        return Ok(None);
    };
    Ok(serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.as_object().cloned()))
}

/// List directory entries, or `[]` when the directory is absent.
pub fn list_dir_if_exists(path: &Path) -> Result<Vec<String>, StorageError> {
    match std::fs::read_dir(path) {
        Ok(entries) => {
            let mut names = Vec::new();
            for entry in entries {
                let entry =
                    entry.map_err(fs_error(&format!("Failed to list {}", path.display())))?;
                names.push(entry.file_name().to_string_lossy().to_string());
            }
            Ok(names)
        }
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                || e.kind() == std::io::ErrorKind::NotADirectory =>
        {
            Ok(Vec::new())
        }
        Err(e) => Err(fs_error(&format!("Failed to list {}", path.display()))(e)),
    }
}

/// Write `content` to `path`, creating parents — returns the action the
/// write resolved to (`Unchanged` when the file already held it).
pub fn write_file_action(path: &Path, content: &str) -> Result<ConfigWriteAction, StorageError> {
    let existing = read_file_if_exists(path)?;
    if existing.as_deref() == Some(content) {
        return Ok(ConfigWriteAction {
            path: path.display().to_string(),
            action: WriteActionKind::Unchanged,
            detail: None,
        });
    }
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .map_err(fs_error(&format!("Failed to create {}", dir.display())))?;
        }
    }
    std::fs::write(path, content)
        .map_err(fs_error(&format!("Failed to write {}", path.display())))?;
    Ok(ConfigWriteAction {
        path: path.display().to_string(),
        action: if existing.is_none() {
            WriteActionKind::Wrote
        } else {
            WriteActionKind::Updated
        },
        detail: None,
    })
}

/// A parsed `*.md`/`*.mdc` doc: file stem, frontmatter attributes, body.
pub struct MarkdownDoc {
    pub stem: String,
    pub file_name: String,
    pub path: PathBuf,
    pub attributes: Map<String, Value>,
    pub body: String,
}

/// Read every top-level file in `dir` whose extension is in `exts`, sorted.
pub fn read_markdown_dir(dir: &Path, exts: &[&str]) -> Result<Vec<MarkdownDoc>, StorageError> {
    let names = list_dir_if_exists(dir)?;
    let mut docs = Vec::new();
    for name in {
        let mut n = names;
        n.sort();
        n
    } {
        let Some(ext) = exts.iter().find(|e| name.to_lowercase().ends_with(**e)) else {
            continue;
        };
        let path = dir.join(&name);
        let Some(text) = read_file_if_exists(&path)? else {
            continue;
        };
        let doc = frontmatter::parse(&text);
        docs.push(MarkdownDoc {
            stem: name[..name.len() - ext.len()].to_string(),
            file_name: name,
            path,
            attributes: doc.attributes,
            body: doc.body,
        });
    }
    Ok(docs)
}

fn walk_files(dir: &Path, prefix: &str) -> Result<Vec<String>, StorageError> {
    let names = list_dir_if_exists(dir)?;
    let mut out = Vec::new();
    for name in names {
        let path = dir.join(&name);
        let meta = std::fs::metadata(&path)
            .map_err(fs_error(&format!("Failed to stat {}", path.display())))?;
        if meta.is_dir() {
            out.extend(walk_files(&path, &format!("{prefix}{name}/"))?);
        } else if meta.is_file() {
            out.push(format!("{prefix}{name}"));
        }
    }
    Ok(out)
}

/// Read a skills root (`<dir>/<name>/SKILL.md` plus sibling files). A dir
/// without a `SKILL.md` is skipped; frontmatter `name` wins over the dir
/// name; leftover attributes land in `metadata`.
pub fn read_skills_dir(skills_dir: &Path) -> Result<Vec<ConfigSkill>, StorageError> {
    let names = list_dir_if_exists(skills_dir)?;
    let mut skills = Vec::new();
    for name in {
        let mut n = names;
        n.sort();
        n
    } {
        let dir = skills_dir.join(&name);
        let skill_path = dir.join("SKILL.md");
        let Some(text) = read_file_if_exists(&skill_path)? else {
            continue;
        };
        let doc = frontmatter::parse(&text);
        let mut attributes = doc.attributes;
        let attr_name = attributes.remove("name");
        let description = attributes.remove("description");
        let rel_files: Vec<String> = walk_files(&dir, "")?
            .into_iter()
            .filter(|p| p.to_uppercase() != "SKILL.MD")
            .collect();
        let mut files = Vec::new();
        for rel in {
            let mut r = rel_files;
            r.sort();
            r
        } {
            if let Some(content) = read_file_if_exists(&dir.join(&rel))? {
                files.push(ConfigFile { path: rel, content });
            }
        }
        attributes.insert(
            "sourcePath".into(),
            Value::String(skill_path.display().to_string()),
        );
        skills.push(ConfigSkill {
            name: attr_name
                .as_ref()
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map_or_else(|| name.clone(), str::to_string),
            description: description
                .as_ref()
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            body: doc.body,
            files,
            metadata: Value::Object(attributes),
        });
    }
    Ok(skills)
}

/// Write `skills` under `skills_dir` — `<name>/SKILL.md` plus sibling files.
pub fn write_skills_dir(
    skills_dir: &Path,
    skills: &[ConfigSkill],
) -> Result<Vec<ConfigWriteAction>, StorageError> {
    let mut actions = Vec::new();
    for skill in skills {
        let Some(stem) = safe_file_stem(&skill.name) else {
            actions.push(ConfigWriteAction {
                path: skills_dir.display().to_string(),
                action: WriteActionKind::Skipped,
                detail: Some(format!(
                    "skill {} has no usable file stem",
                    serde_json::to_string(&skill.name).unwrap_or_default()
                )),
            });
            continue;
        };
        let dir = skills_dir.join(&stem);
        let attrs = skill_attributes(skill);
        actions.push(write_file_action(
            &dir.join("SKILL.md"),
            &frontmatter::render(&attrs, &skill.body),
        )?);
        for file in &skill.files {
            // Preserve subpaths; only refuse genuinely escaping names.
            if file.path.contains("..") || file.path.starts_with('/') {
                actions.push(ConfigWriteAction {
                    path: dir.join(&file.path).display().to_string(),
                    action: WriteActionKind::Skipped,
                    detail: Some("skill file path escapes the skill dir".into()),
                });
                continue;
            }
            actions.push(write_file_action(&dir.join(&file.path), &file.content)?);
        }
    }
    Ok(actions)
}
