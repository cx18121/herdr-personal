use std::{env, fs, io::ErrorKind, path::PathBuf, process};

use serde::{Deserialize, Serialize};

use crate::{
    error::{AppError, AppResult},
    layout::{ComparableLayoutNode, LayoutNode},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UndoRecord {
    pub workspace_id: String,
    pub tab_id: String,
    pub focused_pane_id: String,
    pub before: LayoutNode,
    pub after: ComparableLayoutNode,
}

pub fn save(record: UndoRecord) -> AppResult<()> {
    let path = state_path(&record.tab_id)?;
    let temporary = path.with_extension(format!("{}.tmp", process::id()));
    fs::write(&temporary, serde_json::to_vec(&record)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

pub fn get(tab_id: &str) -> AppResult<Option<UndoRecord>> {
    let path = state_path(tab_id)?;
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(serde_json::from_str(&contents)?)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn remove(tab_id: &str) -> AppResult<()> {
    match fs::remove_file(state_path(tab_id)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn state_path(tab_id: &str) -> AppResult<PathBuf> {
    let directory = env::var_os("HERDR_PLUGIN_STATE_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Message("HERDR_PLUGIN_STATE_DIR is missing".into()))?;
    fs::create_dir_all(&directory)?;
    Ok(directory.join(format!("undo-{tab_id}.json")))
}
