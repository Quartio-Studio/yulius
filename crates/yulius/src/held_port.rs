//! A loopback TCP port for tests of an unreachable server (SPEC-493 FLK-001): bound and not
//! listening, so a connection to it is refused and nothing else can come to listen on it while
//! it is held. A port let go and hoped free can be taken by any process meanwhile.

use std::net::TcpListener;
use std::os::fd::{AsRawFd, OwnedFd};

use nix::sys::socket::{
    AddressFamily, Backlog, SockFlag, SockType, SockaddrIn, bind, getsockname, listen, socket,
};

/// A held port; see the module.
pub(crate) struct HeldPort {
    socket: OwnedFd,
    port: u16,
}

impl HeldPort {
    /// Binds a loopback port the system chooses, without listening.
    pub(crate) fn new() -> std::io::Result<Self> {
        let socket = socket(
            AddressFamily::Inet,
            SockType::Stream,
            SockFlag::empty(),
            None,
        )?;
        bind(socket.as_raw_fd(), &SockaddrIn::new(127, 0, 0, 1, 0))?;
        let port = getsockname::<SockaddrIn>(socket.as_raw_fd())?.port();
        Ok(Self { socket, port })
    }

    /// The held port.
    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// Starts listening on the held port itself: the server comes back where it was missed.
    pub(crate) fn listen(self) -> std::io::Result<TcpListener> {
        listen(&self.socket, Backlog::new(16)?)?;
        Ok(TcpListener::from(self.socket))
    }
}
