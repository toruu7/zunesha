//! Files and images attached to a planning/review prompt.
//!
//! The browser cannot hand a local filesystem path to the daemon, so uploads
//! arrive as base64 in the JSON request. This module writes them under
//! `.zou/attachments/<id>/` and produces a Markdown block containing absolute
//! paths. Agents and planners can then read the original bytes with their
//! normal file tooling; the workspace itself stays a clean git clone.
//!
//! Uploads are deliberately bounded: a prompt is not a file-transfer API, and
//! an unbounded request could exhaust the daemon's memory before the planner
//! even starts.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

/// Maximum number of files in one prompt.
pub const MAX_ATTACHMENTS: usize = 10;
/// Maximum size of one decoded file.
pub const MAX_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
/// Maximum combined decoded size of one prompt's files.
pub const MAX_TOTAL_ATTACHMENT_BYTES: usize = 24 * 1024 * 1024;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

/// A decoded file supplied by a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentUpload {
    pub name: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

/// A file that was written under the project's attachment store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedAttachment {
    pub name: String,
    pub media_type: String,
    pub path: PathBuf,
    pub size: usize,
}

impl SavedAttachment {
    /// True for image media types, used only for the prompt hint.
    pub fn is_image(&self) -> bool {
        self.media_type.starts_with("image/")
    }
}

/// Validate and write uploads into `<repo>/.zou/attachments/<id>/`.
///
/// The returned paths are absolute so they remain valid from an agent working
/// inside `.zou/ws/<task_id>`. Nothing is written until every upload passes
/// validation, so a rejected batch does not leave partial files behind.
pub fn save_attachments(
    repo_root: &Path,
    uploads: &[AttachmentUpload],
) -> Result<Vec<SavedAttachment>> {
    if uploads.is_empty() {
        return Ok(Vec::new());
    }
    // The agent runs inside `.zou/ws/<task_id>`, so every path in the prompt
    // block must be absolute. `discover(".")` returns a relative root, and
    // `zou plan --path .` is a common invocation.
    let repo_root = repo_root.canonicalize().map_err(|error| {
        Error::Attachment(format!(
            "could not resolve repository root {}: {error}",
            repo_root.display()
        ))
    })?;
    if uploads.len() > MAX_ATTACHMENTS {
        return Err(Error::Attachment(format!(
            "too many attachments: {} (maximum {MAX_ATTACHMENTS})",
            uploads.len()
        )));
    }

    let mut total = 0usize;
    for upload in uploads {
        if upload.bytes.is_empty() {
            return Err(Error::Attachment(format!(
                "attachment `{}` is empty",
                upload.name
            )));
        }
        if upload.bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(Error::Attachment(format!(
                "attachment `{}` is too large ({} bytes; maximum {MAX_ATTACHMENT_BYTES})",
                upload.name,
                upload.bytes.len()
            )));
        }
        total = total.saturating_add(upload.bytes.len());
    }
    if total > MAX_TOTAL_ATTACHMENT_BYTES {
        return Err(Error::Attachment(format!(
            "attachments are too large in total ({total} bytes; maximum {MAX_TOTAL_ATTACHMENT_BYTES})"
        )));
    }

    let dir = attachment_dir(&repo_root);
    // A repository can contain a `.zou` symlink pointing outside the project.
    // Resolve before creating anything so uploads cannot write through it.
    crate::sandbox::ensure_within(&repo_root, &dir)?;
    std::fs::create_dir_all(&dir)?;

    let mut used = std::collections::HashSet::new();
    let mut saved = Vec::with_capacity(uploads.len());
    for upload in uploads {
        let name = unique_name(safe_name(&upload.name), &mut used);
        let path = dir.join(&name);
        std::fs::write(&path, &upload.bytes)
            .map_err(|e| Error::Attachment(format!("could not write attachment `{name}`: {e}")))?;
        saved.push(SavedAttachment {
            name,
            media_type: clean_media_type(&upload.media_type),
            path,
            size: upload.bytes.len(),
        });
    }
    Ok(saved)
}

/// Render the canonical prompt section for saved files.
pub fn attachment_block(attachments: &[SavedAttachment]) -> String {
    if attachments.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## Attachments\n\
         The user attached these files to the request. Read them from the absolute paths below \
         with your file/read tool when they are relevant. Images should be inspected directly.\n",
    );
    for attachment in attachments {
        let kind = if attachment.is_image() {
            "image"
        } else {
            "file"
        };
        out.push_str(&format!(
            "- {kind} `{}` ({}, {} bytes) — `{}`\n",
            attachment.name,
            attachment.media_type,
            attachment.size,
            attachment.path.display()
        ));
    }
    out
}

/// Append an attachment section to a prompt, if there is one.
pub fn append_attachments(prompt: &str, attachments: &[SavedAttachment]) -> String {
    let block = attachment_block(attachments);
    if block.is_empty() {
        prompt.to_string()
    } else {
        let prompt = prompt.trim_end();
        if prompt.is_empty() {
            block.trim_end().to_string()
        } else {
            format!("{prompt}\n\n{}", block.trim_end())
        }
    }
}

fn attachment_dir(repo_root: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let sequence = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    repo_root.join(".zou").join("attachments").join(format!(
        "{nanos:032x}-{:x}-{sequence:x}",
        std::process::id()
    ))
}

/// Keep a display name that cannot escape the attachment directory.
fn safe_name(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment");
    let mut safe: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    safe = safe.trim_matches('.').to_string();
    if safe.is_empty() {
        safe = "attachment".to_string();
    }
    if safe.len() > 120 {
        safe.truncate(120);
        safe = safe.trim_end_matches('.').to_string();
    }
    safe
}

fn unique_name(name: String, used: &mut std::collections::HashSet<String>) -> String {
    if used.insert(name.clone()) {
        return name;
    }
    let path = Path::new(&name);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("attachment");
    let extension = path.extension().and_then(|extension| extension.to_str());
    for index in 2.. {
        let candidate = match extension {
            Some(extension) => format!("{stem}-{index}.{extension}"),
            None => format!("{stem}-{index}"),
        };
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("the suffix loop always finds a free name")
}

fn clean_media_type(media_type: &str) -> String {
    let media_type = media_type.trim();
    if media_type.is_empty() || media_type.len() > 120 {
        "application/octet-stream".to_string()
    } else {
        media_type.chars().filter(|c| !c.is_control()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload(name: &str, media_type: &str, bytes: &[u8]) -> AttachmentUpload {
        AttachmentUpload {
            name: name.to_string(),
            media_type: media_type.to_string(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn writes_files_under_the_project_and_keeps_names_safe() {
        let temp = tempfile::tempdir().unwrap();
        let saved = save_attachments(
            temp.path(),
            &[
                upload("../../screenshot.png", "image/png", b"png"),
                upload("notes.txt", "text/plain", b"hello"),
                upload("notes.txt", "text/plain", b"second"),
            ],
        )
        .unwrap();

        assert_eq!(saved.len(), 3);
        assert_eq!(saved[0].name, "screenshot.png");
        assert_eq!(saved[1].name, "notes.txt");
        assert_eq!(saved[2].name, "notes-2.txt");
        for attachment in &saved {
            assert!(attachment
                .path
                .starts_with(temp.path().join(".zou/attachments")));
            assert!(attachment.path.is_file());
        }
    }

    #[test]
    fn empty_and_oversized_batches_are_rejected_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        assert!(save_attachments(temp.path(), &[upload("empty.txt", "text/plain", b"")]).is_err());

        let too_many = (0..=MAX_ATTACHMENTS)
            .map(|index| upload(&format!("{index}.txt"), "text/plain", b"x"))
            .collect::<Vec<_>>();
        assert!(save_attachments(temp.path(), &too_many).is_err());

        let huge = vec![0u8; MAX_ATTACHMENT_BYTES + 1];
        assert!(save_attachments(
            temp.path(),
            &[upload("huge.bin", "application/octet-stream", &huge)]
        )
        .is_err());
        assert!(!temp.path().join(".zou/attachments").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_zou_symlink_that_escapes_the_repository() {
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.path().join(".zou")).unwrap();

        let error = save_attachments(repo.path(), &[upload("notes.txt", "text/plain", b"hello")])
            .unwrap_err();
        assert!(
            matches!(error, Error::SandboxViolation(ref message) if message.contains("escapes")),
            "unexpected error: {error}"
        );
        assert!(!outside.path().join("attachments").exists());
    }

    #[test]
    fn prompt_block_names_images_and_files() {
        let saved = vec![
            SavedAttachment {
                name: "shot.png".into(),
                media_type: "image/png".into(),
                path: PathBuf::from("/repo/.zou/attachments/a/shot.png"),
                size: 42,
            },
            SavedAttachment {
                name: "spec.pdf".into(),
                media_type: "application/pdf".into(),
                path: PathBuf::from("/repo/.zou/attachments/a/spec.pdf"),
                size: 99,
            },
        ];
        let block = attachment_block(&saved);
        assert!(block.contains("image `shot.png`"));
        assert!(block.contains("file `spec.pdf`"));
        assert!(block.contains("/repo/.zou/attachments/a/shot.png"));
        assert!(append_attachments("Do the thing.", &saved).starts_with("Do the thing."));
    }
}
