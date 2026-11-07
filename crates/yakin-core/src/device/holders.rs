//! Who else has a device node open, from the fd links under `/proc`.

use std::path::Path;
use std::path::PathBuf;

pub const PROC: &str = "/proc";

/// A process with one of the nodes open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Holder {
    pub pid: u32,
    pub comm: String,
}

/// Every process but this one with any of `paths` open, by pid. Processes of other users
/// cannot be read and are skipped.
pub fn holders(
    proc: &Path,
    paths: &[PathBuf],
) -> Vec<Holder> {
    let me = std::process::id();
    let Ok(entries) = std::fs::read_dir(proc) else {
        return Vec::new();
    };
    let mut holders: Vec<Holder> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            if pid == me || !holds(&entry.path(), paths) {
                return None;
            }
            let comm = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
            Some(Holder {
                pid,
                comm: comm.trim_end().to_string(),
            })
        })
        .collect();
    holders.sort_by_key(|holder| holder.pid);
    holders
}

fn holds(
    process: &Path,
    paths: &[PathBuf],
) -> bool {
    let Ok(fds) = std::fs::read_dir(process.join("fd")) else {
        return false;
    };
    fds.filter_map(|fd| std::fs::read_link(fd.ok()?.path()).ok())
        .any(|target| paths.contains(&target))
}

#[cfg(test)]
mod test {
    use std::os::unix::fs::symlink;

    use super::*;

    fn process(
        proc: &Path,
        pid: u32,
        comm: &str,
        fds: &[&str],
    ) {
        let dir = proc.join(pid.to_string());
        std::fs::create_dir_all(dir.join("fd")).unwrap();
        std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
        for (fd, target) in fds.iter().enumerate() {
            symlink(target, dir.join("fd").join(fd.to_string())).unwrap();
        }
    }

    #[test]
    fn test_holders() {
        let proc = std::env::temp_dir().join(format!("yakin-proc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&proc);
        process(&proc, 1234, "steam", &["/dev/null", "/dev/hidraw4"]);
        process(&proc, 99, "steamwebhelper", &["/dev/hidraw7"]);
        process(&proc, 50, "bash", &["/dev/pts/0"]);
        process(&proc, std::process::id(), "yakind", &["/dev/hidraw4"]);
        // Another user's process: no fd directory to read
        std::fs::create_dir_all(proc.join("77")).unwrap();
        std::fs::create_dir_all(proc.join("self")).unwrap();
        let paths = [PathBuf::from("/dev/hidraw4"), PathBuf::from("/dev/hidraw5")];
        let found = holders(&proc, &paths);
        std::fs::remove_dir_all(&proc).unwrap();
        assert_eq!(
            found,
            vec![Holder {
                pid: 1234,
                comm: "steam".to_string()
            }]
        );
        assert!(holders(Path::new("/nonexistent"), &paths).is_empty());
    }
}
