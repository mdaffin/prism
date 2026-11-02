//! The panes: which has focus, how far each is scrolled, where each was drawn, and the keys and
//! mouse clicks that move through them.

use std::collections::HashSet;

use ratatui::layout::{Position, Rect};

use super::{App, toggle};

/// Rows scrolled off the top of the checks, description, comments and diff panes. They go back to
/// the top when the highlight moves. The view clamps them to the content it draws.
#[derive(Default)]
pub struct Scroll {
    pub checks: u16,
    pub description: u16,
    /// Columns the description's tables are scrolled sideways. The view clamps it.
    pub description_x: u16,
    /// The same for the open comments, which share it.
    pub comment_x: u16,
    pub diff: u16,
    pub comments: u16,
    /// The highlighted check of the highlighted PR. The view clamps it to the checks there are.
    pub check: usize,
    /// The highlighted comment of the highlighted PR. The view clamps it to the comments there are.
    pub comment: usize,
    /// The comments of the highlighted PR that are expanded. All start collapsed.
    pub comments_open: HashSet<usize>,
}

/// Where the view drew each comment, in lines from the top of the comments pane's document, so
/// that `update` can tell whether a comment's header is on screen.
#[derive(Default)]
pub struct CommentLayout {
    /// The first line and the height of each comment, its header included.
    pub items: Vec<(usize, usize)>,
    /// The lines the pane shows at once.
    pub viewport: usize,
}

/// What the view drew in the checks pane, so that `update` can keep the highlighted check on
/// screen.
#[derive(Default)]
pub struct ChecksLayout {
    /// The checks, not counting the conflict row.
    pub count: usize,
    /// The rows above the first check: 1 if there is a conflict row, else 0.
    pub first: usize,
    /// The rows the pane shows at once, without the header.
    pub viewport: usize,
}

/// The panes that can have focus. Tab cycles through them in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Table,
    Checks,
    Comments,
    Description,
    Diff,
}

impl Pane {
    /// Every pane, in the order Tab cycles through them.
    const ORDER: [Pane; 5] = [
        Pane::Table,
        Pane::Checks,
        Pane::Comments,
        Pane::Description,
        Pane::Diff,
    ];

    fn position(self) -> usize {
        Self::ORDER
            .iter()
            .position(|&pane| pane == self)
            .expect("every pane is in ORDER")
    }

    pub(super) fn next(self) -> Self {
        Self::ORDER[(self.position() + 1) % Self::ORDER.len()]
    }

    pub(super) fn previous(self) -> Self {
        Self::ORDER[(self.position() + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

/// What the view found out as it drew, which `update` needs to act on what is on screen.
pub struct Drawn {
    /// The rows a PgUp/PgDn moves in the focused pane.
    pub page: u16,
    /// Where each pane was drawn, for the mouse.
    pub areas: PaneAreas,
    pub comment_layout: CommentLayout,
    pub checks_layout: ChecksLayout,
}

impl Default for Drawn {
    fn default() -> Self {
        Self {
            // Before the first draw, a page is a row.
            page: 1,
            areas: PaneAreas::default(),
            comment_layout: CommentLayout::default(),
            checks_layout: ChecksLayout::default(),
        }
    }
}

/// The lines that a notch of the wheel scrolls a pane.
const WHEEL_LINES: isize = 3;

/// The screen area of each pane, including its border.
#[derive(Default)]
pub struct PaneAreas {
    pub table: Rect,
    /// The table's rows, below its header. Empty while no rows are drawn.
    pub table_rows: Rect,
    /// The checkbox column of `table_rows`.
    pub table_checkboxes: Rect,
    /// The position in `visible` of the first row drawn.
    pub table_offset: usize,
    pub checks: Rect,
    pub comments: Rect,
    pub description: Rect,
    pub diff: Rect,
}

impl PaneAreas {
    fn of(&self, pane: Pane) -> Rect {
        match pane {
            Pane::Table => self.table,
            Pane::Checks => self.checks,
            Pane::Comments => self.comments,
            Pane::Description => self.description,
            Pane::Diff => self.diff,
        }
    }

    pub(super) fn at(&self, position: Position) -> Option<Pane> {
        Pane::ORDER
            .into_iter()
            .find(|&pane| self.of(pane).contains(position))
    }
}

impl App {
    /// One notch of the wheel: what one Up/Down press does in the table and the comments pane, and
    /// `WHEEL_LINES` lines elsewhere.
    pub(super) fn wheel(&mut self, pane: Pane, direction: isize) {
        match pane {
            Pane::Table => self.move_pane(pane, direction),
            Pane::Comments => self.step_comments(direction),
            Pane::Checks => self.step_checks(direction),
            _ => self.move_pane(pane, direction * WHEEL_LINES),
        }
    }

    /// Moves the table's highlight, or scrolls the focused pane, by `by` rows.
    pub(super) fn move_focused(&mut self, by: isize) {
        self.move_pane(self.focus, by)
    }

    pub(super) fn move_pane(&mut self, pane: Pane, by: isize) {
        match pane {
            Pane::Table => self.move_highlight(by),
            Pane::Checks => {
                self.scroll.checks = scrolled(self.scroll.checks, by);
            }
            Pane::Comments => {
                self.scroll.comments = scrolled(self.scroll.comments, by);
            }
            Pane::Description => {
                self.scroll.description = scrolled(self.scroll.description, by);
            }
            Pane::Diff => {
                self.scroll.diff = scrolled(self.scroll.diff, by);
            }
        }
    }

    /// Scrolls the tables of the description or the comment preview sideways.
    pub(super) fn scroll_sideways(&mut self, pane: Pane, by: isize) {
        let x = match pane {
            Pane::Description => &mut self.scroll.description_x,
            Pane::Comments => &mut self.scroll.comment_x,
            _ => return,
        };
        *x = scrolled(*x, by);
    }

    /// Up/Down: like `move_focused`, except that the comments pane goes comment by comment.
    pub(super) fn step_focused(&mut self, by: isize) {
        match self.focus {
            Pane::Comments => self.step_comments(by),
            Pane::Checks => self.step_checks(by),
            _ => self.move_focused(by),
        }
    }

    /// Up/Down in the checks pane: moves the highlight to another check and scrolls the view to
    /// keep it on screen.
    pub(super) fn step_checks(&mut self, by: isize) {
        let count = self.drawn.checks_layout.count;
        let Some(last) = count.checked_sub(1) else {
            return;
        };
        let current = self.scroll.check.min(last);
        self.pick_check(current.saturating_add_signed(by).min(last));
    }

    /// Highlights check `n`, and scrolls the view if it is off screen. The first check also shows
    /// the conflict row above it.
    pub(super) fn pick_check(&mut self, n: usize) {
        let layout = &self.drawn.checks_layout;
        self.scroll.check = n;
        let row = n + layout.first;
        let top = usize::from(self.scroll.checks);
        let first = if n == 0 {
            0
        } else if row < top {
            row
        } else if row >= top + layout.viewport {
            (row + 1).saturating_sub(layout.viewport)
        } else {
            return;
        };
        self.scroll.checks = to_u16(first);
    }

    /// Up/Down in the comments pane. It moves the highlight to the next comment once that
    /// comment's header is on screen, and scrolls a line at a time until then, so a comment
    /// taller than the pane is read in full before moving on.
    pub(super) fn step_comments(&mut self, by: isize) {
        let layout = &self.drawn.comment_layout;
        if layout.items.is_empty() {
            return;
        }
        let top = usize::from(self.scroll.comments);
        let bottom = top + layout.viewport;
        let current = self.scroll.comment.min(layout.items.len() - 1);
        let next = current
            .checked_add_signed(by)
            .filter(|&n| n < layout.items.len());
        // A header the wheel has scrolled past counts as reached, so the highlight can catch up.
        let reached = |n: usize| {
            let start = layout.items[n].0;
            if by > 0 { start < bottom } else { start >= top }
        };
        match next {
            Some(n) if reached(n) => {
                let start = layout.items[n].0;
                self.scroll.comment = n;
                if start < top {
                    self.scroll.comments = to_u16(start);
                } else if start >= bottom {
                    let first = (start + 1).saturating_sub(layout.viewport);
                    self.scroll.comments = to_u16(first);
                }
            }
            _ => {
                self.scroll.comments = scrolled(self.scroll.comments, by);
                self.keep_comment_on_screen();
            }
        }
    }

    /// Moves the highlight to a comment that is on screen if a line of scrolling took it off, so
    /// that Space toggles what is being read: the topmost one if it went off the top, the
    /// bottommost if it went off the bottom. A page or a jump doesn't do this.
    pub(super) fn keep_comment_on_screen(&mut self) {
        let layout = &self.drawn.comment_layout;
        let Some(&(last_start, last_height)) = layout.items.last() else {
            return;
        };
        // The view clamps the scroll when it draws; do the same here to see what it will show.
        let max_top = (last_start + last_height).saturating_sub(layout.viewport);
        let top = usize::from(self.scroll.comments).min(max_top);
        let bottom = top + layout.viewport;
        let on_screen = |&(start, height): &(usize, usize)| start < bottom && start + height > top;
        let current = self.scroll.comment.min(layout.items.len() - 1);
        if on_screen(&layout.items[current]) {
            return;
        }
        let found = if layout.items[current].0 >= bottom {
            layout.items.iter().rposition(on_screen)
        } else {
            layout.items.iter().position(on_screen)
        };
        if let Some(n) = found {
            self.scroll.comment = n;
        }
    }

    /// A click on a check's row highlights it. A click on the header, the conflict row or below
    /// the last check does nothing more than focus the pane.
    pub(super) fn click_check(&mut self, position: Position) {
        let area = self.drawn.areas.checks;
        // The pane's border is the first row and the table's header the second.
        let Some(row) = position.y.checked_sub(area.y + 2) else {
            return;
        };
        let layout = &self.drawn.checks_layout;
        let row = usize::from(row);
        if row >= layout.viewport {
            return;
        }
        let index = (row + usize::from(self.scroll.checks)).checked_sub(layout.first);
        if let Some(n) = index.filter(|&n| n < layout.count) {
            self.scroll.check = n;
        }
    }

    /// A click on a comment's header highlights it and expands or collapses it. A click on a
    /// body, or below the last comment, does nothing more than focus the pane.
    pub(super) fn click_comment(&mut self, position: Position) {
        let area = self.drawn.areas.comments;
        // The pane's border is the first row, and the rows below it are the document.
        let Some(row) = position.y.checked_sub(area.y + 1) else {
            return;
        };
        let row = usize::from(row);
        let layout = &self.drawn.comment_layout;
        if row >= layout.viewport {
            return;
        }
        let line = row + usize::from(self.scroll.comments);
        if let Some(index) = layout.items.iter().position(|&(start, _)| start == line) {
            self.scroll.comment = index;
            self.toggle_comment();
        }
    }

    /// Expands or collapses the highlighted comment.
    pub(super) fn toggle_comment(&mut self) {
        let Some(last) = self.drawn.comment_layout.items.len().checked_sub(1) else {
            return;
        };
        let current = self.scroll.comment.min(last);
        toggle(&mut self.scroll.comments_open, current);
    }

    /// PgUp/PgDn: a page in the focused pane.
    pub(super) fn page_focused(&mut self, direction: isize) {
        self.move_focused(direction * self.drawn.page as isize);
    }

    /// Home/End: the first or last item of the table or the comments pane, and the top or bottom
    /// of the other panes. The view clamps the scroll, so `u16::MAX` is the bottom.
    pub(super) fn jump_focused(&mut self, end: bool) {
        match self.focus {
            Pane::Table => self.highlight(if end { usize::MAX } else { 0 }),
            Pane::Checks => {
                if let Some(last) = self.drawn.checks_layout.count.checked_sub(1) {
                    self.pick_check(if end { last } else { 0 });
                }
            }
            Pane::Comments => {
                let Some(last) = self.drawn.comment_layout.items.len().checked_sub(1) else {
                    return;
                };
                let n = if end { last } else { 0 };
                // The view pulls the scroll back so the pane isn't left past its end.
                let start = self.drawn.comment_layout.items[n].0;
                self.scroll.comment = n;
                self.scroll.comments = to_u16(start);
            }
            Pane::Description => self.scroll.description = if end { u16::MAX } else { 0 },
            Pane::Diff => self.scroll.diff = if end { u16::MAX } else { 0 },
        }
    }

    pub(super) fn move_highlight(&mut self, by: isize) {
        if let Some(current) = self.table.selected() {
            self.highlight(current.saturating_add_signed(by));
        }
    }

    /// Highlights the row at this position in `visible`, or the last one if it is past the end.
    pub(super) fn highlight(&mut self, position: usize) {
        let Some(current) = self.table.selected() else {
            return;
        };
        let new = position.min(self.visible.len().saturating_sub(1));
        if new != current {
            self.table.select(Some(new));
            self.scroll = Scroll::default();
        }
    }
}

/// `from` moved by `by`, but not below 0 or above `u16::MAX`.
fn scrolled(from: u16, by: isize) -> u16 {
    let by = by.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
    from.saturating_add_signed(by)
}

/// `n`, or `u16::MAX` if it is too big.
pub fn to_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}
