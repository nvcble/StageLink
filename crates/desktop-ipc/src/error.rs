use thiserror::Error;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("pipe I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("could not build the pipe security descriptor: {0}")]
    Security(String),

    #[error("message exceeded the {limit}-byte envelope limit")]
    EnvelopeTooLarge { limit: usize },

    #[error("malformed envelope: {0}")]
    Malformed(String),

    #[error("handshake failed: {0}")]
    Handshake(&'static str),

    #[error("peer speaks control protocol {peer}, this build speaks {ours}")]
    ProtocolMismatch { peer: String, ours: String },

    #[error("the client disconnected")]
    Disconnected,

    #[error("the server is shutting down")]
    ShuttingDown,
}

pub type Result<T> = core::result::Result<T, IpcError>;

impl IpcError {
    /// Whether the server should keep listening for a new client after this.
    ///
    /// A client that disconnects or fails a handshake is routine — Electron may
    /// be restarting. An I/O or security failure on the listener itself is not,
    /// and the service should report it rather than spin.
    #[must_use]
    pub fn is_client_scoped(&self) -> bool {
        matches!(
            self,
            Self::Disconnected
                | Self::Handshake(_)
                | Self::Malformed(_)
                | Self::EnvelopeTooLarge { .. }
                | Self::ProtocolMismatch { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_failures_do_not_take_the_listener_down() {
        assert!(IpcError::Disconnected.is_client_scoped());
        assert!(IpcError::Handshake("bad token").is_client_scoped());
        assert!(IpcError::Malformed("junk".into()).is_client_scoped());
    }

    #[test]
    fn listener_failures_are_not_client_scoped() {
        assert!(!IpcError::Security("no SID".into()).is_client_scoped());
        assert!(!IpcError::ShuttingDown.is_client_scoped());
        assert!(
            !IpcError::Io(std::io::Error::other("disk gone")).is_client_scoped()
        );
    }
}
