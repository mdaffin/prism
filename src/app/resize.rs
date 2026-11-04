//! Resizing the panes: dragging the border between two panes with the mouse, and `+`/`-` and
//! `>`/`<` for the focused pane.
//!
//! The panes of the layout are in stacks (a column of them, or a row), and a stack shares its room
//! by weights out of [`WHOLE`]. A stack has none until it is first resized, so by default the
//! layout is the one `pane_layout` has always had; resizing works the weights out from what was
//! drawn and changes the two on either side of the border. The weights are fractions, so a bigger
//! or smaller terminal keeps the layout.

use std::collections::HashMap;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::{App, Pane};

/// What a stack's weights add up to.
pub const WHOLE: u16 = 1000;

/// The least weight a pane keeps in a stack, so that a stale weight can never collapse it.
pub const MIN_WEIGHT: u16 = 80;

/// The fewest rows and columns a pane keeps when its border is dragged.
pub const MIN_ROWS: u16 = 4;
pub const MIN_COLUMNS: u16 = 16;

/// How far `+`, `-`, `>` and `<` move a border, as a part of the stack's length out of [`WHOLE`].
const KEY_STEP: u32 = 80;

/// The stacks of panes that share room. The items of each are numbered from 0 in the order drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StackId {
    /// Wide: the left column and the details, side by side.
    Columns,
    /// Wide: the list, the checks and the comments, one over the other.
    Left,
    /// Wide: the description over the approvers and diff.
    Detail,
    /// Narrow: the list, the details and the checks and comments, one over the other.
    Rows,
    /// Narrow: the description column beside the approvers and diff.
    DetailColumns,
    /// Narrow: the checks beside the comments.
    Lower,
}

/// The border between two neighbouring items of a stack, as drawn.
#[derive(Debug, Clone)]
pub struct Divider {
    pub stack: StackId,
    /// Whether the items are one over the other (so the border is a row), not side by side.
    pub stacked: bool,
    /// The cells that take a click: the two borders that meet there.
    pub hit: Rect,
    /// The numbers of the items either side of the border in the stack.
    pub between: (usize, usize),
    /// The panes either side, for the keys that resize the focused pane.
    pub before: Vec<Pane>,
    pub after: Vec<Pane>,
    /// Every item that was drawn in the stack and its length along the stack, so that weights
    /// can be worked out from what is on screen.
    pub items: Vec<(usize, u16)>,
    /// The cell where the second item starts, and where the two items together start and end.
    pub boundary: u16,
    pub start: u16,
    pub end: u16,
}

/// How the stacks have been resized. A stack that is not here has its default sizes.
#[derive(Debug, Default, Clone)]
pub struct Sizes {
    weights: HashMap<StackId, [u16; 3]>,
}

impl Sizes {
    /// The weights of `stack`, if it has been resized.
    pub fn of(&self, stack: StackId) -> Option<[u16; 3]> {
        self.weights.get(&stack).copied()
    }

    pub fn is_default(&self) -> bool {
        self.weights.is_empty()
    }

    pub fn reset(&mut self) {
        self.weights.clear();
    }
}

/// A drag on a border that is in progress.
#[derive(Debug, Clone)]
pub struct DividerDrag {
    divider: Divider,
}

impl Divider {
    /// Where `position` is along the stack.
    fn along(&self, position: Position) -> i32 {
        i32::from(if self.stacked { position.y } else { position.x })
    }

    fn minimum(&self) -> u16 {
        if self.stacked { MIN_ROWS } else { MIN_COLUMNS }
    }
}

impl App {
    /// Handles a mouse event on a border between panes, or a drag that began on one. Returns
    /// whether it did.
    pub(super) fn on_divider(&mut self, mouse: MouseEvent) -> bool {
        let at = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let found = self
                    .drawn
                    .areas
                    .dividers
                    .iter()
                    .find(|divider| divider.hit.contains(at))
                    .cloned();
                // The press is not taken, so a click on the border still focuses the pane there;
                // it is only a drag that moves the border.
                self.divider_drag = found.map(|divider| DividerDrag { divider });
                false
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(drag) = self.divider_drag.clone() else {
                    return false;
                };
                let divider = drag.divider;
                self.move_boundary(&divider, divider.along(at));
                true
            }
            MouseEventKind::Up(MouseButton::Left) => self.divider_drag.take().is_some(),
            _ => false,
        }
    }

    /// Moves the border of `divider` so that the second item starts at `to`, never leaving either
    /// item less than its minimum, and shares the stack's weights out again to match.
    pub(super) fn move_boundary(&mut self, divider: &Divider, to: i32) {
        let minimum = i32::from(divider.minimum());
        let (start, end) = (i32::from(divider.start), i32::from(divider.end));
        // With less room than two minimums, the border stays in the middle.
        let (low, high) = if end - start >= 2 * minimum {
            (start + minimum, end - minimum)
        } else {
            let middle = (start + end) / 2;
            (middle, middle)
        };
        let to = to.clamp(low, high);
        let first = u32::try_from(to - start).unwrap_or(0);
        let second = u32::try_from(end - to).unwrap_or(0);
        // The weights of what is drawn, which a stack that has none gets from its lengths.
        let total: u32 = divider.items.iter().map(|&(_, len)| u32::from(len)).sum();
        let mut weights = self.sizes.of(divider.stack).unwrap_or([0; 3]);
        // A pane that was hidden when the weights were worked out has none worth keeping.
        let stale = divider
            .items
            .iter()
            .any(|&(item, _)| weights[item] < MIN_WEIGHT);
        if stale && total > 0 {
            for &(item, len) in &divider.items {
                weights[item] = (u32::from(len) * u32::from(WHOLE) / total) as u16;
            }
            // A pane that is hidden now gets an average share for when it comes back.
            let average = (u32::from(WHOLE) / divider.items.len().max(1) as u32) as u16;
            for (item, weight) in weights.iter_mut().enumerate() {
                if !divider.items.iter().any(|&(i, _)| i == item) {
                    *weight = average;
                }
            }
        }
        let (a, b) = divider.between;
        let pair = u32::from(weights[a]) + u32::from(weights[b]);
        if let Some(new_a) = (pair * first).checked_div(first + second) {
            let new_a = new_a as u16;
            weights[a] = new_a;
            weights[b] = (pair - u32::from(new_a)) as u16;
        }
        self.sizes.weights.insert(divider.stack, weights);
    }

    /// `+` and `-` (`stacked`) or `>` and `<` (side by side): makes the focused pane bigger or
    /// smaller along that axis by moving a border of its. Says so when it has no such border.
    pub(super) fn resize_focused(&mut self, stacked: bool, grow: bool) {
        let focus = self.focus;
        // Of the borders that touch the pane, the one that is between the fewest panes is its own.
        let divider = self
            .drawn
            .areas
            .dividers
            .iter()
            .filter(|d| d.stacked == stacked)
            .filter(|d| d.before.contains(&focus) || d.after.contains(&focus))
            .min_by_key(|d| d.before.len() + d.after.len())
            .cloned();
        let Some(divider) = divider else {
            let way = if stacked {
                "taller or shorter"
            } else {
                "wider or narrower"
            };
            self.note = Some(format!("{} can't be made {way}", capitalised(focus.name())));
            return;
        };
        let length = i32::from(divider.end) - i32::from(divider.start);
        let step = ((length as u32 * KEY_STEP / u32::from(WHOLE)).max(1)) as i32;
        // The first item grows when the border moves towards the second.
        let first_is_focus = divider.before.contains(&focus);
        let towards_second = first_is_focus == grow;
        let to = i32::from(divider.boundary) + if towards_second { step } else { -step };
        self.move_boundary(&divider, to);
    }

    /// `=`: puts every pane back to its default size.
    pub(super) fn reset_sizes(&mut self) {
        if !self.sizes.is_default() {
            self.sizes.reset();
            self.note = Some("Panes back to their usual sizes".to_string());
        }
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
