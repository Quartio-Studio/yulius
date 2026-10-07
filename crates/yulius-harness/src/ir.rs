//! Helpers shared by the transcript readers.

use yulius_domain::time::Timestamp;
use yulius_domain::transcript::{Block, Role, Turn};

/// Appends blocks to the last turn when it has the same role, else starts a new turn.
pub(crate) fn push_blocks(
    turns: &mut Vec<Turn>,
    role: Role,
    blocks: Vec<Block>,
    at: Option<Timestamp>,
) {
    if blocks.is_empty() {
        return;
    }
    match turns.last_mut() {
        Some(last) if last.role == role => last.blocks.extend(blocks),
        _ => turns.push(Turn { role, blocks, at }),
    }
}
