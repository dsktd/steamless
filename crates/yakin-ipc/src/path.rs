use std::path::Path;
use std::path::PathBuf;

/// Where the NixOS module's system daemon listens.
pub const SYSTEM_SOCKET: &str = "/run/yakin/yakin.sock";

/// `$XDG_RUNTIME_DIR/yakin.sock`, else `/tmp/yakin-<uid>.sock`: where a daemon run by the user
/// listens.
pub fn default_socket() -> PathBuf {
    match dirs::runtime_dir() {
        Some(dir) => dir.join("yakin.sock"),
        None => PathBuf::from(format!("/tmp/yakin-{}.sock", nix::unistd::getuid())),
    }
}

/// The socket a client should use when none is given: the user's own daemon if one is
/// listening, else the system daemon's, else the user's path (so the error names it).
/// Apps started by the compositor do not see shell variables like `YAKIN_SOCKET`.
pub fn client_socket() -> PathBuf {
    pick(default_socket(), Path::new(SYSTEM_SOCKET))
}

fn pick(
    user: PathBuf,
    system: &Path,
) -> PathBuf {
    if !user.exists() && system.exists() {
        return system.to_path_buf();
    }
    user
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_pick() {
        let dir = std::env::temp_dir().join(format!("yakin-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let user = dir.join("user.sock");
        let system = dir.join("system.sock");
        // Neither: the user's path
        assert_eq!(pick(user.clone(), &system), user);
        // Only the system one
        std::fs::write(&system, "").unwrap();
        assert_eq!(pick(user.clone(), &system), system);
        // Both: the user's own daemon wins
        std::fs::write(&user, "").unwrap();
        assert_eq!(pick(user.clone(), &system), user);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
