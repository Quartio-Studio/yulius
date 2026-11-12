//! The root move helper, reached through files (`yulius_contracts::migrate`, SPEC-470): the
//! request the server writes, the helper's report, the command's checklist and the checklist a
//! move left on this host.

use yulius_contracts::migrate::{MoveChecklist, MoveHelperStatus, MoveRequest};
use yulius_contracts::update::HostLine;

/// The command's status (`<state>/status.json`): its phase and checklist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveStatus {
    /// The move's id.
    pub id: String,
    /// The new host.
    pub target: String,
    /// `Plan`, `Prepare`, `Copy`, `Cutover`, `Finish`, `Abort`, `done` or `aborted`.
    pub phase: String,
    /// The checklist so far.
    pub lines: Vec<HostLine>,
    /// Its last write (seconds since the epoch).
    pub updated_at: u64,
    /// The one-time key's public line, while a person has to add it on the new host.
    pub public_key: Option<String>,
    /// The new public URL, once decided.
    pub new_url: Option<String>,
}

/// The move helper's files.
pub trait MoveHelper: Send + Sync {
    /// Whether the helper's path unit waits for requests on this host. May block briefly.
    fn installed(&self) -> bool;
    /// Writes the request (atomically, owner-only), replacing an earlier one.
    ///
    /// # Errors
    /// A message when the file cannot be written.
    fn request(&self, request: &MoveRequest) -> Result<(), String>;
    /// Writes the waiting request again, unchanged, when the request `id` is still the one in
    /// the file: the path unit starts the helper on a change. Never brings back a request
    /// that was removed, and never replaces a newer one.
    fn request_again(&self, id: &str);
    /// The request file as it stands, if readable and well formed.
    fn pending(&self) -> Option<MoveRequest>;
    /// Removes the request file when the request `id` is the one in it (the helper took it, or
    /// it is too old to be taken): never a newer one that was written meanwhile.
    fn clear_request(&self, id: &str);
    /// The helper's last report, if readable and well formed.
    fn helper_status(&self) -> Option<MoveHelperStatus>;
    /// The command's status, if readable and well formed.
    fn status(&self) -> Option<MoveStatus>;
    /// The checklist a move to this host left, if any.
    fn arrived(&self) -> Option<MoveChecklist>;
}
