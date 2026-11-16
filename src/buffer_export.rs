//! Stream raw replay buffer ranges to a newly created file.
use crate::replay::BUFFER_PAGE_LIMIT;
use std::{io::Write, path::Path};

pub fn write_new<F, C>(
    path: &Path,
    offset: u64,
    length: u64,
    mut read: F,
    check: C,
) -> Result<(), String>
where
    F: FnMut(u64, u32) -> Result<Vec<u8>, String>,
    C: Fn() -> Result<(), String>,
{
    let end = offset.checked_add(length).ok_or("Invalid buffer range")?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(".renderdocx-buffer-")
        .tempfile_in(parent)
        .map_err(|e| format!("Cannot create temporary export: {e}"))?;
    let mut cursor = offset;
    while cursor < end {
        check()?;
        let count = (end - cursor).min(u64::from(BUFFER_PAGE_LIMIT)) as u32;
        let bytes = read(cursor, count)?;
        if bytes.len() != count as usize {
            return Err("Buffer read returned incomplete data".into());
        }
        temporary
            .write_all(&bytes)
            .map_err(|e| format!("Cannot write buffer export: {e}"))?;
        cursor += u64::from(count);
    }
    temporary
        .flush()
        .map_err(|e| format!("Cannot finish buffer export: {e}"))?;
    check()?;
    temporary
        .persist_noclobber(path)
        .map_err(|e| format!("Cannot publish buffer export: {}", e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streams_ranges_and_preserves_existing_files_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("数据.bin");
        let mut pages = 0;
        write_new(
            &path,
            7,
            u64::from(BUFFER_PAGE_LIMIT) + 3,
            |offset, count| {
                pages += 1;
                Ok((offset..offset + u64::from(count))
                    .map(|n| n as u8)
                    .collect())
            },
            || Ok(()),
        )
        .unwrap();
        let data = std::fs::read(&path).unwrap();
        assert_eq!(pages, 2);
        assert_eq!(data.len(), BUFFER_PAGE_LIMIT as usize + 3);
        assert_eq!(data[0], 7);
        assert_eq!(*data.last().unwrap(), 9);
        assert!(write_new(&path, 0, 1, |_, _| Ok(vec![1]), || Ok(())).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), data);
        let short = dir.path().join("short.bin");
        assert!(write_new(&short, 0, 2, |_, _| Ok(vec![1]), || Ok(())).is_err());
        assert!(!short.exists());
        let cancelled = dir.path().join("cancelled.bin");
        assert!(
            write_new(
                &cancelled,
                0,
                0,
                |_, _| unreachable!(),
                || Err("changed".into())
            )
            .is_err()
        );
        assert!(!cancelled.exists());
        let changed = dir.path().join("changed.bin");
        let checks = std::cell::Cell::new(0);
        assert!(
            write_new(
                &changed,
                0,
                u64::from(BUFFER_PAGE_LIMIT) + 1,
                |_, count| Ok(vec![0; count as usize]),
                || {
                    let n = checks.get();
                    checks.set(n + 1);
                    if n == 0 {
                        Ok(())
                    } else {
                        Err("changed".into())
                    }
                }
            )
            .is_err()
        );
        assert!(!changed.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
