//! Local socket transport: the same protocol as stdio over a Unix domain socket.
//!
//! The socket file is restricted to its owner. Every connection gets its own MCP session; all
//! sessions share the loaded project.

use std::path::Path;
use std::sync::Arc;

use crate::server::SharedState;

#[cfg(unix)]
mod imp {
    use std::io::{self, BufReader};
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::sync::Arc;
    use std::thread;

    use crate::protocol::serve;
    use crate::server::SharedState;

    /// Bind the socket at `path`. A stale socket file is replaced; a live one, or any other
    /// kind of file, is an error.
    pub fn bind(path: &Path) -> io::Result<UnixListener> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_socket() => {
                if UnixStream::connect(path).is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "another server is listening on this socket",
                    ));
                }
                std::fs::remove_file(path)?;
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "the path exists and is not a socket",
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    /// Accept connections until `max_connections` have been served (forever for `None`).
    pub fn serve_listener(
        listener: UnixListener,
        shared: Arc<SharedState>,
        max_connections: Option<usize>,
    ) -> io::Result<()> {
        let mut handles = Vec::new();
        for (served, stream) in listener.incoming().enumerate() {
            let stream = stream?;
            let shared = shared.clone();
            let handle = thread::Builder::new()
                .name("jadx-mcp-connection".into())
                .spawn(move || {
                    let Ok(reader) = stream.try_clone() else {
                        return;
                    };
                    let _ = serve(BufReader::new(reader), stream, shared);
                })?;
            handles.push(handle);
            if max_connections.is_some_and(|max| served + 1 >= max) {
                break;
            }
        }
        for handle in handles {
            let _ = handle.join();
        }
        Ok(())
    }
}

#[cfg(unix)]
pub use imp::{bind, serve_listener};

/// Listen on a Unix domain socket and serve connections until the process ends.
#[cfg(unix)]
pub fn serve_unix(path: &Path, shared: Arc<SharedState>) -> std::io::Result<()> {
    let listener = bind(path)?;
    serve_listener(listener, shared, None)
}

#[cfg(not(unix))]
pub fn serve_unix(_path: &Path, _shared: Arc<SharedState>) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "local sockets are only supported on Unix platforms",
    ))
}
