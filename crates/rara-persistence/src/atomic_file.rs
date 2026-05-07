use std::fs;
use std::path::Path;

use anyhow::Result;

pub fn replace_file(src: &Path, dst: &Path) -> Result<()> {
    replace_file_impl(src, dst)?;
    Ok(())
}

#[cfg(not(windows))]
fn replace_file_impl(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::rename(src, dst)
}

#[cfg(windows)]
fn replace_file_impl(src: &Path, dst: &Path) -> std::io::Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && dst.exists() => {
            fs::remove_file(dst)?;
            fs::rename(src, dst)
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use tempfile::tempdir;

    use super::replace_file;

    #[test]
    fn replace_file_replaces_existing_destination() -> Result<()> {
        let temp = tempdir()?;
        let src = temp.path().join("value.tmp");
        let dst = temp.path().join("value.txt");
        std::fs::write(&dst, "old")?;
        std::fs::write(&src, "new")?;

        replace_file(&src, &dst)?;

        assert_eq!(std::fs::read_to_string(&dst)?, "new");
        assert!(!src.exists());
        Ok(())
    }
}
