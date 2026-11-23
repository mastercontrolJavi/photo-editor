//! Non-destructive edit storage.
//!
//! Each photo's edit is a small JSON file beside it:
//! `IMG_0001.DNG` → `IMG_0001.DNG.edit.json`. The photo itself is never
//! written to.
//!
//! Saves are crash-safe: the new JSON goes to a temporary file, is flushed to
//! disk, and then renamed over the old one. Renaming within a folder is atomic
//! on NTFS, so after a crash or power cut you have either the old edit or the
//! new one, never half a file.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::adjust::Adjustments;
use crate::error::{PipelineError, Result};

pub const SCHEMA: &str = "photo-editor/edit";
/// Bump when the file layout changes; add a migration in [`load`].
pub const CURRENT_VERSION: u32 = 1;
pub const SUFFIX: &str = ".edit.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditFile {
    pub schema: String,
    pub version: u32,
    /// Which photo this edit belongs to (name + size, as a sanity check).
    pub source: SourceRef,
    pub adjustments: Adjustments,
    /// Seconds since 1970-01-01 UTC.
    pub saved_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceRef {
    pub file_name: String,
    pub byte_len: u64,
}

pub fn sidecar_path(source: &Path) -> PathBuf {
    let mut name = source.file_name().unwrap_or_default().to_os_string();
    name.push(SUFFIX);
    source.with_file_name(name)
}

fn sidecar_err(path: &Path, detail: impl Into<String>) -> PipelineError {
    PipelineError::Sidecar { path: path.to_owned(), detail: detail.into() }
}

/// Save the edit for `source`. Returns the sidecar path.
pub fn save(source: &Path, adjustments: &Adjustments) -> Result<PathBuf> {
    let path = sidecar_path(source);
    let byte_len = fs::metadata(source).map(|m| m.len()).unwrap_or(0);
    let doc = EditFile {
        schema: SCHEMA.into(),
        version: CURRENT_VERSION,
        source: SourceRef { file_name: source.file_name().unwrap_or_default().to_string_lossy().into_owned(), byte_len },
        adjustments: adjustments.sanitized(),
        saved_at: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    };
    let json = serde_json::to_vec_pretty(&doc).expect("edit file always serialises");

    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let write = |p: &Path| -> std::io::Result<()> {
        let mut f = File::create(p)?;
        f.write_all(&json)?;
        f.sync_all()
    };
    write(&tmp).map_err(|source| {
        let _ = fs::remove_file(&tmp);
        PipelineError::Write { path: tmp.clone(), source }
    })?;
    fs::rename(&tmp, &path).map_err(|source| {
        let _ = fs::remove_file(&tmp);
        PipelineError::Write { path: path.clone(), source }
    })?;
    Ok(path)
}

/// Load the saved edit for `source`. `Ok(None)` means there is no edit yet.
pub fn load(source: &Path) -> Result<Option<Adjustments>> {
    let path = sidecar_path(source);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(PipelineError::Read { path, source }),
    };
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| sidecar_err(&path, format!("not valid JSON ({e})")))?;
    if value.get("schema").and_then(|s| s.as_str()) != Some(SCHEMA) {
        return Err(sidecar_err(&path, "not a photo-editor edit file"));
    }
    let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if version > CURRENT_VERSION {
        return Err(sidecar_err(&path, format!("saved by a newer version of the app (format v{version})")));
    }
    // Migrations from older versions go here, e.g. `if version == 1 { ... }`.
    let doc: EditFile = serde_json::from_value(value).map_err(|e| sidecar_err(&path, e.to_string()))?;
    Ok(Some(doc.adjustments.sanitized()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(dir: &Path) -> PathBuf {
        let p = dir.join("IMG_0001.DNG");
        fs::write(&p, b"pretend raw bytes").unwrap();
        p
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let src = photo(dir.path());
        assert_eq!(load(&src).unwrap(), None);
        let adj = Adjustments { exposure: 0.75, temperature: -12.5, tint: 4.0 };
        let path = save(&src, &adj).unwrap();
        assert_eq!(path.file_name().unwrap(), "IMG_0001.DNG.edit.json");
        assert_eq!(load(&src).unwrap(), Some(adj));
        // Saving again replaces, doesn't append, and leaves no temp file.
        let adj2 = Adjustments { exposure: -1.0, ..adj };
        save(&src, &adj2).unwrap();
        assert_eq!(load(&src).unwrap(), Some(adj2));
        assert!(!dir.path().join("IMG_0001.DNG.edit.json.tmp").exists());
    }

    #[test]
    fn interrupted_save_keeps_previous_edit() {
        let dir = tempfile::tempdir().unwrap();
        let src = photo(dir.path());
        let adj = Adjustments { exposure: 1.0, ..Default::default() };
        save(&src, &adj).unwrap();
        // Simulate a crash mid-save: a half-written temp file is left behind.
        fs::write(dir.path().join("IMG_0001.DNG.edit.json.tmp"), b"{\"schema\": \"photo-ed").unwrap();
        assert_eq!(load(&src).unwrap(), Some(adj));
        // The next save still works and overwrites the stale temp file.
        save(&src, &Adjustments::default()).unwrap();
        assert_eq!(load(&src).unwrap(), Some(Adjustments::default()));
    }

    #[test]
    fn rejects_corrupt_foreign_and_future_files() {
        let dir = tempfile::tempdir().unwrap();
        let src = photo(dir.path());
        let side = sidecar_path(&src);
        fs::write(&side, b"not json").unwrap();
        assert!(matches!(load(&src), Err(PipelineError::Sidecar { .. })));
        fs::write(&side, br#"{"schema":"something-else","version":1}"#).unwrap();
        assert!(matches!(load(&src), Err(PipelineError::Sidecar { .. })));
        fs::write(&side, br#"{"schema":"photo-editor/edit","version":99}"#).unwrap();
        let err = load(&src).unwrap_err().to_string();
        assert!(err.contains("newer version"), "{err}");
    }

    #[test]
    fn out_of_range_values_are_clamped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let src = photo(dir.path());
        fs::write(
            sidecar_path(&src),
            br#"{"schema":"photo-editor/edit","version":1,"source":{"file_name":"IMG_0001.DNG","byte_len":1},
                "adjustments":{"exposure":40,"temperature":0,"tint":0},"saved_at":0}"#,
        )
        .unwrap();
        assert_eq!(load(&src).unwrap().unwrap().exposure, 5.0);
    }
}
