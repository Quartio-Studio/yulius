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

    /// Feed live output into the screen, preserving erase sequences across reads.
    /// ED 2 clears the display from any position. Codex uses ED 0 from home. Once confirmed,
    /// ordinary output takes just the parser's usual pass.
    pub fn process(&mut self, parser: &mut vt100::Parser, mut bytes: &[u8]) {
        while !self.rendered {
            let Some(end) = bytes.iter().position(|byte| {
                self.suffix.rotate_left(1);
                self.suffix[3] = *byte;
                self.suffix == *b"\x1b[2J" || self.suffix.ends_with(b"\x1b[J")
            }) else {
                break;
            };
            let (through_erase, rest) = bytes.split_at(end + 1);
            parser.process(through_erase);
            let grid = parser.screen();
            let full_erase = self.suffix == *b"\x1b[2J" || grid.cursor_position() == (0, 0);
            self.rendered = full_erase
                && grid.alternate_screen()
                && grid.rows(0, grid.size().1).all(|row| row.trim().is_empty());
            bytes = rest;
        }
        parser.process(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::Redraw;

    #[test]
    fn a_codex_erase_from_home_proves_a_repaint_across_read_boundaries() {
        let full = b"\x1b[1;1H\x1b[J";
        for split in 0..=full.len() {
            let mut grid = vt100::Parser::new(6, 40, 0);
            grid.process(b"\x1b[?1049h\x1b[1;31Hdraft");
            grid.screen_mut().set_size(6, 20);
            let mut redraw = Redraw::restored(false);
            redraw.process(&mut grid, b"\x1b[2;1H\x1b[J");
            assert!(!redraw.rendered(), "partial erase on an already blank crop");
            redraw.process(&mut grid, &full[..split]);
            redraw.process(&mut grid, &full[split..]);
            assert!(redraw.rendered(), "split at {split}");
        }
        let mut primary = vt100::Parser::new(6, 40, 0);
        let mut redraw = Redraw::restored(false);
        redraw.process(&mut primary, full);
        assert!(!redraw.rendered());
    }

    #[test]
    fn clearing_the_primary_screen_cannot_certify_a_cropped_alternate_screen() {
        let mut grid = vt100::Parser::new(6, 40, 0);
        grid.process(b"\x1b[?47hretained\x1b[1;31Hstashed");
        let mut redraw = Redraw::default();
        redraw.invalidate();
        grid.screen_mut().set_size(6, 20);
        redraw.process(&mut grid, b"\x1b[?47l\x1b[2J\x1b[?47h");
        assert_eq!(grid.screen().contents().trim(), "retained");
        assert!(!redraw.rendered());
        redraw.process(&mut grid, b"\x1b[2Jnew paint");
        assert!(redraw.rendered());
    }
}
