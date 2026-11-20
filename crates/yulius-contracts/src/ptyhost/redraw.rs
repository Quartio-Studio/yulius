//! Whether a screen can provide evidence about contents at the PTY's current dimensions.

/// A resize can crop off a hint without the child repainting. A full alternate-screen
/// erase from the child makes all surviving cells come from the new geometry.
pub struct Redraw {
    rendered: bool,
    suffix: [u8; 4],
}

impl Default for Redraw {
    fn default() -> Self {
        // A fresh parser has no cells from a previous size.
        Self::restored(true)
    }
}

impl Redraw {
    /// Starts with the provenance of the restored snapshot. A legacy or parked snapshot
    /// cannot establish that its cells were painted at the displayed dimensions.
    pub fn restored(rendered: bool) -> Self {
        Self {
            rendered,
            suffix: [0; 4],
        }
    }

    /// Discards evidence when the grid is resized independently of its child.
    pub fn invalidate(&mut self) {
        *self = Self::restored(false);
    }

    /// Whether an alternate grid at the current dimensions can be used for a typing decision.
    pub fn rendered(&self) -> bool {
        self.rendered
    }

    /// Feed live output into the screen, preserving the four-byte erase sequence across reads.
    /// Inspect the grid only after a complete ED 2 in the alternate screen. Once confirmed,
    /// ordinary output takes just the parser's usual pass.
    pub fn process(&mut self, parser: &mut vt100::Parser, mut bytes: &[u8]) {
        while !self.rendered {
            let Some(end) = bytes.iter().position(|byte| {
                self.suffix.rotate_left(1);
                self.suffix[3] = *byte;
                self.suffix == *b"\x1b[2J"
            }) else {
                break;
            };
            let (through_erase, rest) = bytes.split_at(end + 1);
            parser.process(through_erase);
            let grid = parser.screen();
            self.rendered = grid.alternate_screen()
                && grid.rows(0, grid.size().1).all(|row| row.trim().is_empty());
            bytes = rest;
        }
        parser.process(bytes);
    }
}
