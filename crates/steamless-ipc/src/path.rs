use std::path::PathBuf;

/// `$XDG_RUNTIME_DIR/steamless.sock`, else `/tmp/steamless-<uid>.sock`.
pub fn default_socket() -> PathBuf {
    match dirs::runtime_dir() {
        Some(dir) => dir.join("steamless.sock"),
        None => PathBuf::from(format!("/tmp/steamless-{}.sock", nix::unistd::getuid())),
    }
}
