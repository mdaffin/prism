//! The mouse on the scrollbars: a click on the track pages towards it, and a drag on the thumb
//! scrolls in proportion, for as long as the button is held, wherever the pointer goes.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use super::panes::{ScrollTarget, ScrollbarHit};
use super::{App, to_u16};

/// A drag on a thumb that is in progress.
#[derive(Debug, Clone, Copy)]
pub struct BarDrag {
    pub target: ScrollTarget,
    /// How many cells into the thumb the pointer took hold of it.
    grab: usize,
}

impl ScrollbarHit {
    /// How far along the track, in cells, the pointer is, which may be before or past the ends.
    fn cell(&self, at: Position) -> isize {
        if self.vertical {
            isize::try_from(at.y).unwrap_or(0) - isize::try_from(self.track.y).unwrap_or(0)
        } else {
            isize::try_from(at.x).unwrap_or(0) - isize::try_from(self.track.x).unwrap_or(0)
        }
    }

    fn track_len(&self) -> usize {
        usize::from(if self.vertical {
            self.track.height
        } else {
            self.track.width
        })
    }

    /// The thumb's first cell and its length, as ratatui draws them: the track is shared out in
    /// proportion to the content.
    fn thumb(&self, offset: usize) -> (usize, usize) {
        let track = self.track_len() as f64;
        let content = self.content.max(1) as f64;
        let length = (self.viewport as f64 * track / content).round().max(1.0);
        let start = (offset as f64 * track / content).round();
        (start as usize, length as usize)
    }

    /// The furthest the content can scroll.
    fn max_offset(&self) -> usize {
        self.content.saturating_sub(self.viewport)
    }

    /// The offset that puts the thumb's first cell at `start`, the other way round from `thumb`.
    fn offset_for(&self, start: isize) -> usize {
        let track = self.track_len().max(1) as f64;
        let offset = (start.max(0) as f64 * self.content as f64 / track).round() as usize;
        offset.min(self.max_offset())
    }
}

impl App {
    /// Handles a mouse event that is on a scrollbar, or a drag that began on one. Returns whether
    /// it did, so that nothing else acts on it.
    pub(super) fn on_scrollbar(&mut self, mouse: MouseEvent) -> bool {
        let at = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(hit) = self
                    .drawn
                    .areas
                    .scrollbars
                    .iter()
                    .find(|hit| hit.track.contains(at))
                    .copied()
                else {
                    return false;
                };
                let cell = hit.cell(at);
                let (start, length) = hit.thumb(hit.offset);
                let (start, end) = (start as isize, (start + length) as isize);
                if (start..end).contains(&cell) {
                    self.bar_drag = Some(BarDrag {
                        target: hit.target,
                        grab: (cell - start) as usize,
                    });
                } else {
                    // A page towards the click, which is as far as the viewport shows.
                    let page = hit.viewport.max(1);
                    let to = if cell < start {
                        hit.offset.saturating_sub(page)
                    } else {
                        (hit.offset + page).min(hit.max_offset())
                    };
                    self.scroll_to(&hit, to);
                }
                true
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(drag) = self.bar_drag else {
                    return false;
                };
                let hit = self
                    .drawn
                    .areas
                    .scrollbars
                    .iter()
                    .find(|hit| hit.target == drag.target)
                    .copied();
                // The bar is gone if the content has shrunk to fit; the drag is over.
                match hit {
                    Some(hit) => {
                        let start = hit.cell(at) - drag.grab as isize;
                        self.scroll_to(&hit, hit.offset_for(start));
                    }
                    None => self.bar_drag = None,
                }
                true
            }
            MouseEventKind::Up(MouseButton::Left) => self.bar_drag.take().is_some(),
            _ => false,
        }
    }

    /// Scrolls what `hit` is the bar of to `offset`.
    fn scroll_to(&mut self, hit: &ScrollbarHit, offset: usize) {
        let value = to_u16(offset);
        match hit.target {
            ScrollTarget::Checks => self.scroll.checks = value,
            ScrollTarget::Approvers => self.scroll.approvers = value,
            ScrollTarget::Comments => self.scroll.comments = value,
            ScrollTarget::Description => self.scroll.description = value,
            ScrollTarget::Diff => self.scroll.diff = value,
            ScrollTarget::ListX => self.list_x = value,
            ScrollTarget::ChecksX => self.scroll.checks_x = value,
            ScrollTarget::DiffX => self.scroll.diff_x = value,
            ScrollTarget::Table => {
                self.table = std::mem::take(&mut self.table).with_offset(offset);
                // The view keeps the highlighted row on screen, so it has to move with the rows.
                if let Some(selected) = self.table.selected() {
                    let last = offset + hit.viewport.saturating_sub(1);
                    if selected < offset {
                        self.highlight(offset);
                    } else if selected > last {
                        self.highlight(last);
                    }
                }
            }
        }
        // The next draw, which clamps the offset, brings the bars to where they are now.
        if let Some(bar) = self
            .drawn
            .areas
            .scrollbars
            .iter_mut()
            .find(|bar| bar.target == hit.target)
        {
            bar.offset = offset;
        }
    }
}
