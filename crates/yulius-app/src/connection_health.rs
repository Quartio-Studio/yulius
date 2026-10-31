//! Connection troubles on the panel (SPEC-361, ADR-0246): the egress proxy and the server's
//! sandbox relays report refusals to the app's registry; the app tells the sandbox's people when
//! a trouble begins or ends and lists the open ones they may see.

use std::sync::Arc;

use yulius_domain::connection_health::ConnectionHealth;

use crate::context::App;

impl App {
    /// The registry the egress proxy and the sandbox relays report their troubles to.
    #[must_use]
    pub fn connection_health(&self) -> Arc<ConnectionHealth> {
        Arc::clone(&self.connection_health)
    }
}
