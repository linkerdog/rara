//! Pure RARA core APIs intended for browser and worker integration.

use std::collections::BTreeMap;

use rara_apply_patch::{
    PatchActionStats as ApplyPatchActionStats, PatchChange as ApplyPatchChange,
    PatchError as ApplyPatchError, build_patch_action,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WasmCoreError {
    #[error("patch preview failed: {0}")]
    PatchPreview(String),
}

impl From<ApplyPatchError> for WasmCoreError {
    fn from(error: ApplyPatchError) -> Self {
        Self::PatchPreview(error.to_string())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct VirtualFileSet {
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

impl VirtualFileSet {
    pub fn get(&self, path: &str) -> Option<&String> {
        self.files.get(path)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PatchPreviewRequest {
    pub patch: String,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PatchPreview {
    pub summary: Vec<String>,
    pub stats: PatchActionStats,
    pub preview: PatchTextPreview,
    pub delta: VirtualPatchDelta,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct VirtualPatchDelta {
    pub exact: bool,
    pub changes: Vec<VirtualPatchChange>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct VirtualPatchChange {
    pub path: String,
    #[serde(flatten)]
    pub change: VirtualPatchFileChange,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VirtualPatchFileChange {
    Add {
        content: String,
        overwritten_content: Option<String>,
    },
    Delete {
        content: String,
    },
    Update {
        move_to: Option<String>,
        original_content: String,
        overwritten_move_content: Option<String>,
        new_content: String,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PatchActionStats {
    pub files_changed: usize,
    pub hunks_applied: usize,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub created_files: Vec<String>,
    pub deleted_files: Vec<String>,
    pub moved_files: Vec<PatchMove>,
    pub updated_files: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PatchMove {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PatchTextPreview {
    pub text: String,
    pub truncated: bool,
}

pub fn preview_patch(request: PatchPreviewRequest) -> Result<PatchPreview, WasmCoreError> {
    preview_patch_with_files(request.patch, request.files)
}

pub fn preview_patch_with_files(
    patch: impl Into<String>,
    files: BTreeMap<String, String>,
) -> Result<PatchPreview, WasmCoreError> {
    let patch = patch.into();
    let action = build_patch_action(&patch, |path| Ok(files.get(path).cloned()))?;
    let delta = VirtualPatchDelta {
        exact: true,
        changes: action
            .changes
            .iter()
            .map(|change| virtual_patch_change(change, &files))
            .collect(),
    };

    Ok(PatchPreview {
        summary: action.summary(),
        stats: patch_action_stats(action.stats),
        preview: PatchTextPreview {
            text: action.preview.text,
            truncated: action.preview.truncated,
        },
        delta,
    })
}

fn virtual_patch_change(
    change: &ApplyPatchChange,
    files: &BTreeMap<String, String>,
) -> VirtualPatchChange {
    match change {
        ApplyPatchChange::Add { path, content, .. } => VirtualPatchChange {
            path: path.clone(),
            change: VirtualPatchFileChange::Add {
                content: content.clone(),
                overwritten_content: files.get(path).cloned(),
            },
        },
        ApplyPatchChange::Delete { path, content, .. } => VirtualPatchChange {
            path: path.clone(),
            change: VirtualPatchFileChange::Delete {
                content: content.clone(),
            },
        },
        ApplyPatchChange::Update {
            path,
            move_to,
            original_content,
            new_content,
            ..
        } => VirtualPatchChange {
            path: path.clone(),
            change: VirtualPatchFileChange::Update {
                move_to: move_to.clone(),
                original_content: original_content.clone(),
                overwritten_move_content: move_to
                    .as_ref()
                    .and_then(|target| files.get(target))
                    .cloned(),
                new_content: new_content.clone(),
            },
        },
    }
}

fn patch_action_stats(stats: ApplyPatchActionStats) -> PatchActionStats {
    PatchActionStats {
        files_changed: stats.files_changed,
        hunks_applied: stats.hunks_applied,
        added_lines: stats.added_lines,
        removed_lines: stats.removed_lines,
        created_files: stats.created_files,
        deleted_files: stats.deleted_files,
        moved_files: stats
            .moved_files
            .into_iter()
            .map(|move_entry| PatchMove {
                from: move_entry.from,
                to: move_entry.to,
            })
            .collect(),
        updated_files: stats.updated_files,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{
        PatchPreviewRequest, VirtualPatchFileChange, WasmCoreError, preview_patch,
        preview_patch_with_files,
    };

    #[test]
    fn previews_patch_as_serializable_virtual_delta() {
        let files = BTreeMap::from([
            ("old.txt".to_string(), "obsolete\n".to_string()),
            ("src.txt".to_string(), "before\n".to_string()),
            ("dst.txt".to_string(), "existing target\n".to_string()),
        ]);
        let preview = preview_patch_with_files(
            "*** Begin Patch\n\
             *** Add File: new.txt\n\
             +created\n\
             *** Delete File: old.txt\n\
             *** Update File: src.txt\n\
             *** Move to: dst.txt\n\
             @@\n\
             -before\n\
             +after\n\
             *** End Patch",
            files,
        )
        .expect("patch previews");

        assert_eq!(
            preview.summary,
            vec![
                "Add file new.txt",
                "Delete file old.txt",
                "Update file src.txt -> dst.txt",
            ]
        );
        assert_eq!(preview.stats.files_changed, 3);
        assert_eq!(preview.stats.added_lines, 2);
        assert_eq!(preview.stats.removed_lines, 2);
        assert!(preview.delta.exact);
        assert_eq!(preview.delta.changes.len(), 3);
        assert!(matches!(
            &preview.delta.changes[2].change,
            VirtualPatchFileChange::Update {
                move_to: Some(target),
                original_content,
                overwritten_move_content: Some(overwritten),
                new_content,
            } if target == "dst.txt"
                && original_content == "before\n"
                && overwritten == "existing target\n"
                && new_content == "after\n"
        ));

        let value = serde_json::to_value(&preview.delta.changes[2]).expect("serializes");
        assert_eq!(
            value,
            json!({
                "path": "src.txt",
                "kind": "update",
                "move_to": "dst.txt",
                "original_content": "before\n",
                "overwritten_move_content": "existing target\n",
                "new_content": "after\n",
            })
        );
    }

    #[test]
    fn previews_request_payload() {
        let preview = preview_patch(PatchPreviewRequest {
            patch: "*** Begin Patch\n*** Add File: note.txt\n+hello\n*** End Patch".to_string(),
            files: BTreeMap::new(),
        })
        .expect("patch previews");

        assert_eq!(preview.delta.changes.len(), 1);
        assert_eq!(preview.stats.created_files, vec!["note.txt"]);
    }

    #[test]
    fn rejects_missing_update_target() {
        let error = preview_patch_with_files(
            "*** Begin Patch\n\
             *** Update File: missing.txt\n\
             @@\n\
             -old\n\
             +new\n\
             *** End Patch",
            BTreeMap::new(),
        )
        .expect_err("missing update target fails");

        assert_eq!(
            error,
            WasmCoreError::PatchPreview("Cannot update missing file missing.txt".to_string())
        );
    }
}
