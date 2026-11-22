//! Coordinate the completion and administrative cancellation of the enrolled proxy and relay.
use std::io;
use tokio::sync::{oneshot, watch};

#[derive(Clone, Copy, Default)]
struct Progress {
    enrolled: bool,
    relay_done: bool,
    proxy_done: bool,
    cancelled: bool,
}

/// Keep a completed copy selectable while its original peer still owes buffered bytes.
#[derive(Clone, Default)]
pub struct CopyDrain {
    proxy: bool,
    signal: Option<watch::Sender<Progress>>,
}

impl CopyDrain {
    /// Wait for the other enrolled copy after both local write halves have closed.
    pub async fn finish(&self) {
        if let Some(signal) = &self.signal {
            signal.send_modify(|p| {
                if self.proxy {
                    p.proxy_done = true;
                } else {
                    p.relay_done = true;
                }
            });
            let mut receiver = signal.subscribe();
            let _ = receiver
                .wait_for(|p| !p.enrolled || (p.relay_done && p.proxy_done) || p.cancelled)
                .await;
        }
    }

    pub(super) fn enroll(&self) -> Option<CopyCancellation> {
        let signal = self.signal.as_ref()?;
        signal
            .send_if_modified(|p| {
                if p.enrolled || p.relay_done || p.cancelled {
                    return false;
                }
                p.enrolled = true;
                true
            })
            .then(|| CopyCancellation {
                drain: Self {
                    signal: self.signal.clone(),
                    proxy: true,
                },
            })
    }
}

/// Administrative cancellation shared by the two original copies of one enrolled tunnel.
#[derive(Default)]
pub struct CopyCancellation {
    drain: CopyDrain,
}

impl CopyCancellation {
    pub(super) fn unclaimed() -> Self {
        Self {
            drain: CopyDrain {
                signal: Some(watch::channel(Progress::default()).0),
                proxy: false,
            },
        }
    }

    /// Reconstitute the two owners of an already enrolled connection.
    #[must_use]
    pub fn pair() -> (Self, Self) {
        let signal = watch::channel(Progress {
            enrolled: true,
            ..Progress::default()
        })
        .0;
        (
            Self {
                drain: CopyDrain {
                    signal: Some(signal.clone()),
                    proxy: false,
                },
            },
            Self {
                drain: CopyDrain {
                    signal: Some(signal),
                    proxy: true,
                },
            },
        )
    }

    /// Observe natural completion while the caller retains its socket descriptors.
    #[must_use]
    pub fn drain(&self) -> CopyDrain {
        self.drain.clone()
    }

    /// Run a new copy whose admission owner can cancel the enclosing task.
    ///
    /// # Errors
    /// Return the same socket or deadline error as the transfer.
    pub async fn run_copy(
        mut self,
        transfer: impl Future<Output = io::Result<()>>,
    ) -> io::Result<()> {
        let peer_ended = async {
            if let Some(signal) = &self.drain.signal {
                let mut receiver = signal.subscribe();
                let _ = receiver.wait_for(|p| p.cancelled).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        let result = tokio::select! {
            biased;
            () = peer_ended => return Ok(()),
            result = transfer => result,
        };
        if result.is_ok() {
            self.drain.signal.take();
        }
        result
    }

    /// Run a restored copy under its existing eviction receiver.
    pub async fn run(
        self,
        evicted: oneshot::Receiver<()>,
        transfer: impl Future<Output = io::Result<()>>,
    ) {
        tokio::select! {
            biased;
            _ = evicted => {}
            _ = self.run_copy(transfer) => {}
        }
    }
}

impl Drop for CopyCancellation {
    fn drop(&mut self) {
        if let Some(signal) = &self.drain.signal {
            signal.send_modify(|p| p.cancelled = true);
        }
    }
}
