use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub fn ensure_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)
            .with_context(|| format!("Impossible de créer le dossier {:?}", path))?;
    }
    Ok(())
}
