//! `yulius instance migrate` (SPEC-460, ADR-0297): one root command on the old host that moves
//! the whole instance to another host as one resumable checklist.

pub mod profile;

#[cfg(test)]
mod tests;
