//! Servers with real processes: the fake agent behind a real ptyhost, as `yulius serve` runs them.

pub mod first_message;
pub mod pool;
pub mod supervision;
