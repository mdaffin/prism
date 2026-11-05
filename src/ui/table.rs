use super::*;

/// The widths of the table's columns other than the title. Each fits its longest value, such as
/// `#12345`, `· no checks` and `✓ approved`, and a space.
pub(super) const NUMBER_WIDTH: u16 = 7;

pub(super) const CHECKS_WIDTH: u16 = 12;

pub(super) const REVIEW_WIDTH: u16 = 11;

/// Fits `app/renovate` and a typical login; a longer one is cut.
pub(super) const AUTHOR_WIDTH: u16 = 16;

/// Fits the header with its arrow, `Updated ▲`; a cell is an age such as `3d`.
pub(super) const UPDATED_WIDTH: u16 = 9;

/// The columns of the table that stay in view when it scrolls sideways: the checkbox and the
/// number, and the gap after each.
pub(super) const FROZEN_WIDTH: u16 = CHECKBOX_WIDTH + 1 + NUMBER_WIDTH + 1;

/// The most columns a title column gets when the table scrolls, so one huge title can't make the
/// table enormous.
pub(super) const MAX_TITLE_COLUMN_WIDTH: usize = 300;

/// The width of the table's first column, which holds `[ ]` or `[x]`.
pub(super) const CHECKBOX_WIDTH: u16 = 3;

/// The table of PRs in a bordered pane, or a message in its place while there are none.
pub(super) fn pr_table(app: &mut App, frame: &mut Frame, area: Rect) {
    let table_block = pane_block(table_title(app), app.focus == Pane::Table);
    app.drawn.areas.table = area;
    app.drawn.areas.table_rows = Rect::default();
    app.drawn.areas.table_checkboxes = Rect::default();
    app.drawn.areas.table_header = Vec::new();
    let table_area = table_block.inner(area);
    frame.render_widget(table_block, area);
    // One row of the table is its header.
    set_page(app, Pane::Table, table_area.height, 2);

    if matches!(app.mode, Mode::Loading) {
        frame.render_widget(Line::from("Loading…"), table_area);
    } else if let Some(message) = empty_table_message(app) {
        let message = Paragraph::new(message).wrap(Wrap { trim: false });
        frame.render_widget(message, table_area);
    } else {
        // Popups are drawn over the table, so it is drawn under them too.
        let shown = app.shown_columns();
        let header = header_cells(app, &shown);
        // The title columns are the header's last cells.
        let layout = table_layout(app, &shown, &header[1 + shown.len()..], table_area.width);
        draw_rows(app, frame, (area, table_area), &shown, header, &layout);
        record_table_areas(app, table_area, &shown, &layout);
    }
}

/// What the table says in place of its rows while the list has none to show, if that is the case.
pub(super) fn empty_table_message(app: &App) -> Option<String> {
    if !matches!(app.mode, Mode::List) {
        return None;
    }
    if app.rows.is_empty() {
        let query = app.submitted_query();
        Some(if app.errors.text().is_some() {
            format!(
                "No PRs loaded for `{query}`. Press Ctrl-R to try again, or / to change the search."
            )
        } else {
            format!("No PRs found for `{query}`. Press / to change the search.")
        })
    } else if app.visible.is_empty() {
        Some(format!(
            "No PRs match the filter `{}`. Press Esc to clear it.",
            app.filter.value()
        ))
    } else {
        None
    }
}

/// The table's header cells: the checkbox's (empty), the `shown` columns' and then each title
/// column's. The sorted column's has an arrow: up for smallest first. A title sort goes by the
/// title columns in order, so it is marked on the first.
pub(super) fn header_cells(app: &App, shown: &[Column]) -> Vec<String> {
    let (sorted, descending) = app.sort();
    let arrow = if descending { " ▼" } else { " ▲" };
    let marked = |column: Sort, name: &str| {
        if sorted == column {
            format!("{name}{arrow}")
        } else {
            name.to_string()
        }
    };
    // The groups of the column regex share the space the title had.
    let mut titles = match &app.columns {
        Some(regex) => model::column_names(regex),
        None => vec!["Title".to_string()],
    };
    titles[0] = marked(Sort::Title, &titles[0]);
    [String::new()]
        .into_iter()
        .chain(shown.iter().map(|&c| marked(c.sort(), c.name())))
        .chain(titles)
        .collect()
}

/// The widths of the table's columns, and whether they are more than the pane has room for.
pub(super) struct TableLayout {
    widths: Vec<Constraint>,
    /// The width the table takes up when drawn in full.
    total: u16,
    /// Whether `total` is more than the pane's width, so the table scrolls sideways.
    scrolling: bool,
}

/// The widths of the checkbox, the `shown` columns and the `titles`, in a pane `available`
/// columns wide. With more than fits, the title columns take the width of their longest cell and
/// the table scrolls sideways; otherwise they share what is left.
pub(super) fn table_layout(
    app: &App,
    shown: &[Column],
    titles: &[String],
    available: u16,
) -> TableLayout {
    let title_widths = title_column_widths(app, titles);
    let fixed = CHECKBOX_WIDTH + shown.iter().map(|&c| column_width(c)).sum::<u16>();
    let gaps = (1 + shown.len() + titles.len() - 1) as u16;
    let total = fixed + title_widths.iter().sum::<u16>() + gaps;
    let scrolling = total > available;
    let widths = [Constraint::Length(CHECKBOX_WIDTH)]
        .into_iter()
        .chain(
            shown
                .iter()
                .map(|&column| Constraint::Length(column_width(column))),
        )
        .chain(title_widths.iter().map(|&width| {
            if scrolling {
                Constraint::Length(width)
            } else {
                Constraint::Fill(1)
            }
        }))
        .collect();
    TableLayout {
        widths,
        total,
        scrolling,
    }
}

/// Draws the header and the visible rows in `table_area`, which is inside `area`'s border, scrolled
/// sideways when the table is wider, and the scrollbars along the border.
pub(super) fn draw_rows(
    app: &mut App,
    frame: &mut Frame,
    (area, table_area): (Rect, Rect),
    shown: &[Column],
    header: Vec<String>,
    layout: &TableLayout,
) {
    // Borrowing the fields rather than calling `visible_rows` leaves `app.table` free.
    let now = jiff::Timestamp::now();
    let rows = app.visible.iter().map(|&i| &app.rows[i]).map(|row| {
        let selected = app.selected.contains(&row.pr.number);
        pr_row(row, selected, shown, app.columns.as_ref(), now)
    });
    let mut table = Table::new(rows, layout.widths.clone())
        .header(Row::new(header).bold())
        .row_highlight_style(Style::new().reversed());
    // The rows are about to be replaced.
    if app.list.searching {
        table = table.dim();
    }
    if layout.scrolling {
        let total = layout.total;
        // Drawn at its full width, then the part in view is copied over, keeping the first
        // columns in place.
        let full = Rect::new(0, 0, total, table_area.height);
        let mut buffer = Buffer::empty(full);
        StatefulWidget::render(table, full, &mut buffer, &mut app.table);
        let x = app.list_x.min(total - table_area.width);
        app.list_x = x;
        copy_scrolled(
            &buffer,
            frame.buffer_mut(),
            table_area,
            frozen_width(shown),
            x,
        );
        pane_hscrollbar(
            app,
            frame,
            ScrollTarget::ListX,
            area,
            (
                usize::from(total),
                usize::from(table_area.width),
                usize::from(x),
            ),
        );
    } else {
        app.list_x = 0;
        frame.render_stateful_widget(table, table_area, &mut app.table);
    }
    // One row of the table is its header.
    pane_scrollbar(
        app,
        frame,
        ScrollTarget::Table,
        area,
        (
            app.visible.len(),
            usize::from(table_area.height.saturating_sub(1)),
            app.table.offset(),
        ),
    );
}

/// Records where the table drew its rows, checkboxes and headers, for the mouse. Call it after
/// `draw_rows`, which settles how far the table is scrolled.
pub(super) fn record_table_areas(
    app: &mut App,
    table_area: Rect,
    shown: &[Column],
    layout: &TableLayout,
) {
    // The header is one row.
    app.drawn.areas.table_rows = Rect {
        y: table_area.y + 1,
        height: table_area.height.saturating_sub(1),
        ..table_area
    };
    app.drawn.areas.table_checkboxes = Rect {
        width: CHECKBOX_WIDTH,
        ..app.drawn.areas.table_rows
    };
    app.drawn.areas.table_offset = app.table.offset();
    let full_width = if layout.scrolling {
        layout.total
    } else {
        table_area.width
    };
    app.drawn.areas.table_header = header_hits(
        table_area,
        shown,
        full_width,
        frozen_width(shown),
        app.list_x,
    );
}

/// Where each sortable column's header is on screen, for a click: each of the `shown` columns, and
/// one area for all the title columns, to the end of the table. The table is `full_width` columns
/// wide, scrolled `x` columns past its `frozen` ones, as drawn above. The checkbox has nothing to
/// sort by.
pub(super) fn header_hits(
    area: Rect,
    shown: &[Column],
    full_width: u16,
    frozen: u16,
    x: u16,
) -> Vec<(Rect, Sort)> {
    // The columns, in the table's own coordinates, with the one column between each.
    let mut start = CHECKBOX_WIDTH + 1;
    let mut columns = Vec::new();
    for &column in shown {
        let end = start + column_width(column);
        columns.push((start, end, column.sort()));
        start = end + 1;
    }
    columns.push((start, full_width, Sort::Title));
    columns
        .into_iter()
        .filter_map(|(start, end, sort)| {
            // The frozen columns stay; the rest move with the scroll, and can't go under them.
            let (start, end) = if end <= frozen {
                (start, end)
            } else {
                (start.saturating_sub(x).max(frozen), end.saturating_sub(x))
            };
            let end = end.min(area.width);
            (start < end).then(|| (Rect::new(area.x + start, area.y, end - start, 1), sort))
        })
        .collect()
}

/// The width of the table's `column`.
pub(super) fn column_width(column: Column) -> u16 {
    match column {
        Column::Number => NUMBER_WIDTH,
        Column::Checks => CHECKS_WIDTH,
        Column::Review => REVIEW_WIDTH,
        Column::Author => AUTHOR_WIDTH,
        Column::Updated => UPDATED_WIDTH,
    }
}

/// The width of the columns that stay in view when the table scrolls sideways: the checkbox, and
/// the number if it is shown, with the gap after each.
pub(super) fn frozen_width(shown: &[Column]) -> u16 {
    if shown.contains(&Column::Number) {
        FROZEN_WIDTH
    } else {
        CHECKBOX_WIDTH + 1
    }
}

/// The widest cell of each title column, the header included, as far as is worth showing.
pub(super) fn title_column_widths(app: &App, titles: &[String]) -> Vec<u16> {
    let mut widths: Vec<usize> = titles.iter().map(|title| title.width()).collect();
    for row in app.visible_rows() {
        for (width, cell) in widths.iter_mut().zip(row.title_cells(app.columns.as_ref())) {
            *width = (*width).max(cell.width());
        }
    }
    widths
        .into_iter()
        .map(|width| to_u16(width.min(MAX_TITLE_COLUMN_WIDTH)))
        .collect()
}

/// Copies `source`, which was drawn at its full width from the top left, into `area` of `target`,
/// scrolled `x` columns past its first `frozen` columns, which stay.
pub(super) fn copy_scrolled(source: &Buffer, target: &mut Buffer, area: Rect, frozen: u16, x: u16) {
    for dy in 0..area.height {
        for dx in 0..area.width {
            let from = if dx < frozen { dx } else { dx + x };
            target[(area.x + dx, area.y + dy)] = source[(from, dy)].clone();
        }
    }
}

/// A PR's row of the table, with a cell for each of the `shown` columns. A closed or merged PR is
/// dimmed.
pub(super) fn pr_row<'a>(
    row: &'a app::Row,
    selected: bool,
    shown: &[Column],
    columns: Option<&Regex>,
    now: jiff::Timestamp,
) -> Row<'a> {
    let mark = if selected { "[x]" } else { "[ ]" };
    let cell = |column: Column| {
        let cell = Cell::new(row.cell(column, now));
        if column == Column::Checks {
            cell.style(checks_style(row))
        } else {
            cell
        }
    };
    let cells = Row::new(
        [Cell::new(mark)]
            .into_iter()
            .chain(shown.iter().map(|&column| cell(column)))
            .chain(row.title_cells(columns).into_iter().map(Cell::new)),
    );
    if row.pr.state == PrState::Open {
        cells
    } else {
        cells.dim()
    }
}

/// The colour of a row's checks. The checks of a closed or merged PR no longer matter, so they
/// aren't coloured.
pub(super) fn checks_style(row: &app::Row) -> Style {
    if row.pr.state == PrState::Open {
        state_style(row.checks.state)
    } else {
        Style::new()
    }
}

/// The local filter box, drawn like the search box.
pub(super) fn filter(app: &App, frame: &mut Frame, area: Rect) {
    let width = area.width.saturating_sub(2);
    let scroll = app.filter.visual_scroll(usize::from(width));
    let block = pane_block("Filter", app.typing == Typing::Filter);
    let input = Paragraph::new(app.filter.value())
        .scroll((0, scroll as u16))
        .block(block);
    frame.render_widget(input, area);
    if app.typing == Typing::Filter {
        let x = app.filter.visual_cursor().saturating_sub(scroll) as u16;
        frame.set_cursor_position(Position::new(area.x + 1 + x, area.y + 1));
    }
}

/// The box that edits the column regex, with the reason a regex was refused in its title.
pub(super) fn columns(app: &App, frame: &mut Frame, area: Rect) {
    let width = area.width.saturating_sub(2);
    let scroll = app.columns_input.visual_scroll(usize::from(width));
    let mut title = Line::from("Columns");
    if let Some(error) = &app.columns_error {
        title.push_span(format!(" · {error}").red().bold());
    }
    let input = Paragraph::new(app.columns_input.value())
        .scroll((0, scroll as u16))
        .block(pane_block(title, true));
    frame.render_widget(input, area);
    let x = app.columns_input.visual_cursor().saturating_sub(scroll) as u16;
    frame.set_cursor_position(Position::new(area.x + 1 + x, area.y + 1));
}

pub(super) fn search(app: &App, frame: &mut Frame, area: Rect) {
    let width = area.width.saturating_sub(2);
    let scroll = app.query.visual_scroll(usize::from(width));
    let mut title = Line::from("Search");
    if app.list.searching {
        title.push_span(format!(" · {} searching", spinner(app)).yellow().bold());
    }
    if app.typing != Typing::Search && app.query_is_unsearched() {
        title.push_span(" · not searched".dim());
    }
    let block = pane_block(title, app.typing == Typing::Search);
    let input = Paragraph::new(app.query.value())
        .scroll((0, scroll as u16))
        .block(block);
    frame.render_widget(input, area);
    if app.typing == Typing::Search {
        let x = app.query.visual_cursor().saturating_sub(scroll) as u16;
        frame.set_cursor_position(Position::new(area.x + 1 + x, area.y + 1));
    }
}

/// The most suggestions shown at once.
pub(super) const MAX_SUGGESTION_ROWS: u16 = 8;

/// The list of suggestions under the search box, starting under the word they replace, drawn over
/// the table. Not drawn where there is no room for at least one row inside its border.
pub(super) fn suggestions(app: &App, frame: &mut Frame, search_area: Rect, body: Rect) {
    let Some((completion, selected)) = app.completion() else {
        return;
    };
    // A box squeezed by a very short terminal is not under anything.
    if search_area.height < INPUT_BOX_ROWS {
        return;
    }
    let top = search_area.bottom();
    let space = body.bottom().saturating_sub(top);
    let rows = completion
        .suggestions
        .len()
        .min(usize::from(MAX_SUGGESTION_ROWS))
        .min(usize::from(space.saturating_sub(2)));
    if rows == 0 {
        return;
    }
    let widest = completion
        .suggestions
        .iter()
        .map(|s| s.width())
        .max()
        .unwrap_or(0);
    // A column of padding each side of the text, and the border.
    let width = (widest as u16 + 4).min(search_area.width);
    let before: usize = app
        .query
        .value()
        .chars()
        .take(completion.start())
        .map(|c| c.width().unwrap_or(0))
        .sum();
    let scroll = app
        .query
        .visual_scroll(usize::from(search_area.width.saturating_sub(2)));
    let x = (search_area.x + 1 + before.saturating_sub(scroll) as u16)
        .min(search_area.right().saturating_sub(width));
    let area = Rect::new(x, top, width, rows as u16 + 2);
    // Scrolled just far enough to keep the chosen one in view.
    let first = selected.map_or(0, |selected| (selected + 1).saturating_sub(rows));
    let lines: Vec<Line> = completion
        .suggestions
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .map(|(i, suggestion)| {
            let line = Line::from(format!(" {suggestion} "));
            if Some(i) == selected {
                line.reversed()
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(Block::bordered()), area);
}

/// The list's title, with how many PRs it holds and how many of those the filter shows, and how
/// many are selected: `Pull requests (12 of 30, 3 selected)`. What isn't true is left out.
pub(super) fn table_title(app: &App) -> String {
    let mut parts = Vec::new();
    if !app.rows.is_empty() {
        if app.visible.len() == app.rows.len() {
            parts.push(app.rows.len().to_string());
        } else {
            parts.push(format!("{} of {}", app.visible.len(), app.rows.len()));
        }
    }
    if !app.selected.is_empty() {
        parts.push(format!("{} selected", app.selected.len()));
    }
    with_counts("Pull requests", &parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::*;

    /// Two open PRs whose checks are the same, so only their numbers tell them apart.
    fn app_with_two_to_sort() -> App {
        app_with(vec![
            pr(9, "Bump zulu", "OPEN", ""),
            pr(3, "Bump alpha", "OPEN", ""),
        ])
    }

    /// The header row of the table, as drawn.
    fn header_of(app: &mut App) -> String {
        let screen = lines(&draw(app, 200, 30));
        table_header_line(app, &screen)
    }

    /// Which of the numbers `#9` and `#3` comes first on screen, as the order of their row lines.
    fn order_of_rows(app: &mut App) -> (usize, usize) {
        let screen = lines(&draw(app, 200, 30));
        let row = |number: &str| {
            screen
                .iter()
                .position(|line| line.contains(number) && line.contains("[ ]"))
                .unwrap()
        };
        (row("#9"), row("#3"))
    }

    #[test]
    fn the_table_starts_sorted_by_checks_smallest_first_in_githubs_order() {
        let mut app = app_with_two_to_sort();
        let top = header_of(&mut app);
        assert!(top.contains("Checks ▲"), "{top}");
        assert!(!top.contains("Review ▲") && !top.contains('▼'), "{top}");
        // The same checks, so GitHub's order: 9, then 3.
        let (nine, three) = order_of_rows(&mut app);
        assert!(nine < three);
    }

    #[test]
    fn s_sorts_by_the_next_column_and_changes_the_rows_drawn() {
        let mut app = app_with_two_to_sort();
        press(&mut app, "s");
        let top = header_of(&mut app);
        assert!(top.contains("# ▲") && !top.contains("Checks ▲"), "{top}");
        let (nine, three) = order_of_rows(&mut app);
        assert!(three < nine);
    }

    #[test]
    fn capital_s_reverses_the_direction_and_the_rows() {
        let mut app = app_with_two_to_sort();
        press(&mut app, "sS");
        let top = header_of(&mut app);
        assert!(top.contains("# ▼"), "{top}");
        let (nine, three) = order_of_rows(&mut app);
        assert!(nine < three);
    }

    #[test]
    fn each_press_of_s_marks_the_next_column() {
        // Presses of `s` from the start, and the header that gets the arrow.
        for (presses, marked) in [
            (2, "Title ▲"),
            (3, "Review ▲"),
            (4, "Author ▲"),
            (5, "Updated ▲"),
        ] {
            let mut app = app_with_two_to_sort();
            press(&mut app, &"s".repeat(presses));
            let top = header_of(&mut app);
            assert!(top.contains(marked), "{presses} presses: {top}");
        }
    }

    #[test]
    fn sorting_by_the_update_time_leaves_no_sorted_by_note_in_the_title() {
        let mut app = app_with_two_to_sort();
        press(&mut app, "sssss");
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(!screen[0].contains("sorted by"), "{}", screen[0]);
    }

    /// The middle of the header cell that sorts by `sort`, as last drawn.
    fn header_cell(app: &App, sort: Sort) -> Position {
        let (area, _) = app
            .drawn
            .areas
            .table_header
            .iter()
            .find(|(_, s)| *s == sort)
            .unwrap_or_else(|| panic!("no header for {sort:?}"));
        Position::new(area.x + area.width / 2, area.y)
    }

    fn click_header(app: &mut App, sort: Sort) {
        let at = header_cell(app, sort);
        press_at(app, at);
    }

    #[test]
    fn the_sortable_headers_are_where_they_are_drawn() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        let buffer = draw(&mut app, 200, 30);
        let header = &app.drawn.areas.table_header;
        let sorts: Vec<Sort> = header.iter().map(|(_, sort)| *sort).collect();
        assert_eq!(
            sorts,
            [
                Sort::Number,
                Sort::Checks,
                Sort::Review,
                Sort::Author,
                Sort::Updated,
                Sort::Title
            ]
        );
        // Each is on the header row, over its own text, and none overlap.
        assert!(text_at(&buffer, header[0].0).trim().starts_with('#'));
        assert!(text_at(&buffer, header[1].0).trim().starts_with("Checks"));
        assert!(text_at(&buffer, header[2].0).trim().starts_with("Review"));
        assert!(text_at(&buffer, header[3].0).trim().starts_with("Author"));
        assert!(text_at(&buffer, header[4].0).trim().starts_with("Updated"));
        assert!(text_at(&buffer, header[5].0).trim().starts_with("Title"));
        for pair in header.windows(2) {
            assert!(pair[0].0.right() <= pair[1].0.x);
            assert_eq!(pair[0].0.y, pair[1].0.y);
        }
        // The title columns reach the end of the table.
        let title = header[5].0;
        assert_eq!(title.right(), app.drawn.areas.table.right() - 1);
    }

    #[test]
    fn clicking_a_header_sorts_by_it_then_reverses_it_then_goes_back_to_the_default() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 200, 30);
        // The default is checks, smallest first, so the first click on it reverses it.
        for expected in [
            (Sort::Checks, true),
            (Sort::Checks, false),
            (Sort::Checks, true),
        ] {
            click_header(&mut app, Sort::Checks);
            draw(&mut app, 200, 30);
            assert_eq!(app.sort(), expected);
        }
        // Another column: smallest first, then reversed, then back to the default.
        for expected in [
            (Sort::Review, false),
            (Sort::Review, true),
            (Sort::Checks, false),
        ] {
            click_header(&mut app, Sort::Review);
            draw(&mut app, 200, 30);
            assert_eq!(app.sort(), expected);
        }
        for expected in [(Sort::Number, false), (Sort::Number, true)] {
            click_header(&mut app, Sort::Number);
            draw(&mut app, 200, 30);
            assert_eq!(app.sort(), expected);
        }
        // Any title column is the title; clicking another column changes over.
        click_header(&mut app, Sort::Title);
        draw(&mut app, 200, 30);
        assert_eq!(app.sort(), (Sort::Title, false));
    }

    #[test]
    fn a_click_on_a_header_reorders_the_rows_and_moves_the_arrow() {
        let mut app = app_with(vec![
            by("alice", 9, "Bump zulu"),
            by("alice", 3, "Bump alpha"),
        ]);
        draw(&mut app, 200, 30);
        click_header(&mut app, Sort::Number);
        let screen = lines(&draw(&mut app, 200, 30));
        let header = table_header_line(&app, &screen);
        assert!(
            header.contains("# ▲") && !header.contains("Checks ▲"),
            "{header}"
        );
        let row = |number: &str| {
            screen
                .iter()
                .position(|line| line.contains(number) && line.contains("[ ]"))
                .unwrap()
        };
        assert!(row("#3") < row("#9"));
        // The highlight stayed on the PR it was on, which was the first listed.
        assert_eq!(app.highlighted().unwrap().pr.number, 9);
        assert_eq!(app.focus, Pane::Table);
    }

    #[test]
    fn a_click_on_the_checkbox_header_does_not_sort_and_one_on_the_author_does() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 200, 30);
        let table = app.drawn.areas.table;
        // The checkbox is the first column inside the border.
        press_at(&mut app, Position::new(table.x + 1, table.y + 1));
        assert_eq!(app.sort(), (Sort::Checks, false));
        for expected in [
            (Sort::Author, false),
            (Sort::Author, true),
            (Sort::Checks, false),
        ] {
            click_header(&mut app, Sort::Author);
            draw(&mut app, 200, 30);
            assert_eq!(app.sort(), expected);
        }
    }

    #[test]
    fn there_is_no_author_header_to_click_when_the_search_names_one_author() {
        let mut app = app_with_query("is:pr author:alice", vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 200, 30);
        let sorts: Vec<Sort> = app
            .drawn
            .areas
            .table_header
            .iter()
            .map(|(_, s)| *s)
            .collect();
        assert_eq!(
            sorts,
            [
                Sort::Number,
                Sort::Checks,
                Sort::Review,
                Sort::Updated,
                Sort::Title
            ]
        );
    }

    /// The header row of the table, as drawn at 200 columns.
    fn table_header_text(app: &mut App) -> String {
        let buffer = draw(app, 200, 30);
        let area = app.drawn.areas.table;
        (area.x..area.right())
            .map(|x| buffer[(x, area.y + 1)].symbol())
            .collect()
    }

    #[test]
    fn the_updated_column_shows_how_long_ago_in_a_word_or_two() {
        let mut pr = by("alice", 1, "Bump alpha");
        pr.updated_at = "2020-01-02T03:04:05Z".to_string();
        let mut app = app_with(vec![pr]);
        let screen = lines(&draw(&mut app, 200, 30)).join("\n");
        assert!(screen.contains("Updated"), "{screen}");
        assert!(screen.contains('y'), "{screen}");
    }

    #[test]
    fn each_column_can_be_hidden_and_takes_its_header_and_click_area_with_it() {
        for column in Column::ALL {
            let mut app = app_with(vec![by("alice", 1, "Bump alpha"), by("bob", 2, "Bump b")]);
            app.toggle_column(column);
            let top = table_header_text(&mut app);
            assert!(!top.contains(column.name()), "{column:?}: {top}");
            // The others are still there, and a click on each still sorts by it.
            let sorts: Vec<Sort> = app
                .drawn
                .areas
                .table_header
                .iter()
                .map(|(_, s)| *s)
                .collect();
            assert!(!sorts.contains(&column.sort()), "{column:?}");
            assert_eq!(sorts.len(), Column::ALL.len(), "{column:?}: {sorts:?}");
            for sort in sorts {
                click_header(&mut app, sort);
                draw(&mut app, 200, 30);
                assert_eq!(app.sort().0, sort, "{column:?}");
            }
        }
    }

    #[test]
    fn hiding_the_number_unfreezes_it_and_the_checkbox_alone_stays() {
        let mut app = app_with_a_long_title();
        app.toggle_column(Column::Number);
        draw_narrow(&mut app);
        for _ in 0..8 {
            press(&mut app, "l");
        }
        draw_narrow(&mut app);
        let table = app.drawn.areas.table;
        assert!(app.list_x > 0);
        for (area, sort) in &app.drawn.areas.table_header {
            assert!(area.x > table.x + CHECKBOX_WIDTH, "{sort:?}");
        }
    }

    #[test]
    fn the_columns_popup_shows_every_column_and_space_hides_the_highlighted_one() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha"), by("bob", 2, "Bump b")]);
        press(&mut app, "v");
        let screen = lines(&draw(&mut app, 200, 30)).join("\n");
        assert!(
            screen.contains("[x] Number") || screen.contains("[x] #"),
            "{screen}"
        );
        for column in Column::ALL {
            assert!(
                screen.contains(&format!("[x] {}", column.name())),
                "{screen}"
            );
        }
        press(&mut app, "j");
        press(&mut app, " ");
        assert_eq!(app.hidden, [Column::Checks]);
        let screen = lines(&draw(&mut app, 200, 30)).join("\n");
        assert!(screen.contains("[ ] Checks"), "{screen}");
        press(&mut app, " ");
        assert!(app.hidden.is_empty());
        // It takes every key but the ones that close it, and Esc closes it.
        press(&mut app, "s");
        assert!(matches!(app.mode, Mode::PickColumns(_)));
        key(&mut app, KeyCode::Esc);
        assert_list_mode(&app);
    }

    #[test]
    fn the_filter_ignores_hidden_columns() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha"), by("bob", 2, "Bump b")]);
        app.filter = tui_input::Input::new("bob".to_string());
        app.toggle_column(Column::Number);
        assert_eq!(app.visible.len(), 1);
        app.toggle_column(Column::Author);
        assert!(app.visible.is_empty());
        app.toggle_column(Column::Author);
        assert_eq!(app.visible.len(), 1);
    }

    /// The text of the screen, which has each pane's title in its top border.
    fn border_titles(app: &mut App, width: u16) -> String {
        lines(&draw(app, width, 40)).join("\n")
    }

    #[test]
    fn the_list_title_counts_the_prs_the_filter_shows_and_those_selected() {
        for width in [200, 100] {
            let mut app = app_with(vec![
                by("alice", 1, "Bump alpha"),
                by("alice", 2, "Bump beta"),
                by("bob", 3, "Fix gamma"),
            ]);
            let screen = border_titles(&mut app, width);
            assert!(screen.contains("Pull requests (3)"), "{width}: {screen}");
            assert!(!screen.contains(" of 3"), "{width}");
            press(&mut app, " ");
            let screen = border_titles(&mut app, width);
            assert!(
                screen.contains("Pull requests (3, 1 selected)"),
                "{width}: {screen}"
            );
            press(&mut app, "fbump");
            key(&mut app, KeyCode::Enter);
            let screen = border_titles(&mut app, width);
            assert!(
                screen.contains("Pull requests (2 of 3, 1 selected)"),
                "{width}: {screen}"
            );
        }
    }

    #[test]
    fn the_checks_comments_and_approvers_titles_say_how_many_they_hold() {
        for width in [200, 100] {
            let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
            // Nothing loaded yet: no count to claim.
            let screen = border_titles(&mut app, width);
            assert!(
                screen.contains("Comments") && !screen.contains("Comments ("),
                "{screen}"
            );
            assert!(
                screen.contains("Approvers") && !screen.contains("Approvers ("),
                "{screen}"
            );
            let details = serde_json::from_value(serde_json::json!({
                "headRefName": "branch",
                "body": "",
                "comments": [
                    {"author": {"login": "a"}, "createdAt": "2026-01-01T00:00:00Z", "body": "x"},
                    {"author": {"login": "b"}, "createdAt": "2026-01-02T00:00:00Z", "body": "y"},
                ],
                "additions": 1,
                "deletions": 1,
                "approvers": {"requests": [
                    {"of": {"Person": "u1"}, "code_owner": false},
                    {"of": {"Person": "u2"}, "code_owner": false},
                    {"of": {"Person": "u3"}, "code_owner": false},
                ]},
            }))
            .unwrap();
            app.details_loaded(1, Ok(details));
            let screen = border_titles(&mut app, width);
            assert!(screen.contains("Comments (2)"), "{width}: {screen}");
            assert!(screen.contains("Approvers (3)"), "{width}: {screen}");
        }
    }

    #[test]
    fn the_headers_follow_a_table_that_scrolls_sideways_and_stay_clear_of_the_frozen_columns() {
        let mut app = app_with_a_long_title();
        draw_narrow(&mut app);
        let before = app.drawn.areas.table_header.clone();
        let number = before.iter().find(|(_, s)| *s == Sort::Number).unwrap().0;
        for _ in 0..8 {
            press(&mut app, "l");
        }
        draw_narrow(&mut app);
        assert!(app.list_x > 0);
        let table = app.drawn.areas.table;
        let after = &app.drawn.areas.table_header;
        // The number is frozen and stays; the others have moved left, or are out of sight, and none
        // reach under the frozen columns or past the table.
        let number_after = after.iter().find(|(_, s)| *s == Sort::Number).unwrap().0;
        assert_eq!(number_after, number);
        for (area, sort) in after {
            if *sort != Sort::Number {
                assert!(area.x >= table.x + 1 + FROZEN_WIDTH, "{sort:?}");
            }
            assert!(area.right() < table.right(), "{sort:?}");
        }
        // The title is still there to click, and sorts by title.
        click_header(&mut app, Sort::Title);
        assert_eq!(app.sort(), (Sort::Title, false));
    }

    #[test]
    fn there_are_no_headers_to_click_with_no_rows_and_a_dialog_ignores_clicks() {
        let mut app = app_with(vec![]);
        draw(&mut app, 200, 30);
        assert!(app.drawn.areas.table_header.is_empty());

        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 200, 30);
        let cell = header_cell(&app, Sort::Number);
        press(&mut app, "?");
        press_at(&mut app, cell);
        assert_eq!(app.sort(), (Sort::Checks, false));
    }

    #[test]
    fn a_title_sort_marks_the_first_title_column_when_there_are_several() {
        let mut app = app_with(vec![pr(1, "Bump alpha to 1.2", "OPEN", "")]);
        app.columns =
            Some(model::parse_column_regex(r"Bump (?<container>.+?) to (?<version>\S+)$").unwrap());
        press(&mut app, "ss");
        let screen = lines(&draw(&mut app, 200, 30));
        let top = table_header_line(&app, &screen);
        assert!(
            top.contains("container ▲") && !top.contains("version ▲"),
            "{top}"
        );
    }

    #[test]
    fn draws_the_table() {
        let mut app = app_with(vec![
            pr(7, "Bump example-lib", "OPEN", "APPROVED"),
            pr(9, "Old change", "MERGED", ""),
        ]);
        let buffer = draw(&mut app, 200, 30);
        let screen = lines(&buffer);

        assert!(screen[0].starts_with("PRs"));
        let header = table_header_line(&app, &screen);
        assert!(header.contains("# ") && header.contains("Review") && header.contains("Title"));
        let open = table_row_line(&app, &screen, "#7");
        assert!(open.contains("[ ]"));
        assert!(open.contains("· no checks"));
        assert!(open.contains("✓ approved"));
        assert!(open.contains("Bump example-lib"));
        let merged = table_row_line(&app, &screen, "#9");
        assert!(merged.contains("· merged") && merged.contains("Old change"));

        // The helper finds cells, so a test can read their colours.
        let position = find(&buffer, "Bump example-lib").unwrap();
        assert!(screen[usize::from(position.y)].contains("#7"));
        assert_eq!(buffer[position].symbol(), "B");
    }

    #[test]
    fn colours_the_checks_column() {
        let mut app = one_of_each();
        let buffer = draw(&mut app, 200, 30);
        for (label, colour) in [
            ("✓ passing", Color::Green),
            ("… pending", Color::Yellow),
            ("· no checks", Color::Yellow),
            ("✗ failing", Color::Red),
            ("✗ conflict", Color::Red),
        ] {
            // The checks pane lists the highlighted PR's state too, so look in the table's rows.
            let table_row = lines(&buffer)
                .iter()
                .position(|line| line.contains(label) && line.contains('#'))
                .unwrap_or_else(|| panic!("no table row for {label}"));
            let column = lines(&buffer)[table_row].find(label).unwrap();
            let position = Position::new(column as u16, table_row as u16);
            assert_eq!(buffer[position].fg, colour, "{label}");
        }
    }

    #[test]
    fn draws_the_author_of_each_pr() {
        let mut app = app_with(vec![
            by("alice", 1, "Bump alpha"),
            by("bob", 2, "Bump beta"),
        ]);
        let screen = lines(&draw(&mut app, 200, 30));

        assert!(shows(&screen, "Author"));
        assert!(
            screen
                .iter()
                .any(|l| l.contains("#1") && l.contains("alice"))
        );
        assert!(screen.iter().any(|l| l.contains("#2") && l.contains("bob")));
    }

    #[test]
    fn draws_a_bot_the_way_the_query_spells_it() {
        let mut app = app_with(vec![by("app/renovate", 1, "Bump alpha")]);
        let screen = lines(&draw(&mut app, 200, 30));

        assert!(
            screen
                .iter()
                .any(|l| l.contains("#1") && l.contains("app/renovate"))
        );
    }

    #[test]
    fn hides_the_author_column_when_the_query_names_one_author() {
        let mut app = app_with_query(
            "is:pr author:app/renovate",
            vec![by("app/renovate", 1, "Bump alpha")],
        );
        let screen = lines(&draw(&mut app, 200, 30));

        assert!(!shows(&screen, "Author"));
        assert!(
            !screen
                .iter()
                .any(|l| l.contains("[ ]") && l.contains("app/renovate"))
        );
        assert!(
            screen
                .iter()
                .any(|l| l.contains("#1") && l.contains("Bump alpha"))
        );
    }

    #[test]
    fn a_search_term_does_not_hide_the_author_column() {
        let mut app = app_with_query("is:pr alice", vec![by("alice", 1, "Bump alpha")]);
        let screen = lines(&draw(&mut app, 200, 30));

        assert!(shows(&screen, "Author"));
    }

    #[test]
    fn a_click_highlights_the_row_under_it_on_a_scrolled_filtered_list() {
        let prs = (1..=40)
            .map(|n| {
                pr(
                    n,
                    &format!("Bump {}", if n % 2 == 0 { "even" } else { "odd" }),
                    "OPEN",
                    "",
                )
            })
            .collect();
        let mut app = app_with(prs);
        // Only the 20 even PRs are shown; the table is too short to show them all.
        press(&mut app, "feven");
        key(&mut app, KeyCode::Enter);
        draw_wide(&mut app);
        click_table_row(&mut app, 2, 5);
        assert_eq!(app.table.selected(), Some(2));
        assert_eq!(app.highlighted().unwrap().pr.number, 6);

        // Highlight the last row so the table scrolls, then click the first row drawn.
        for _ in 0..30 {
            press(&mut app, "j");
        }
        draw_wide(&mut app);
        let offset = app.drawn.areas.table_offset;
        assert!(offset > 0);
        click_table_row(&mut app, 0, 5);
        assert_eq!(app.table.selected(), Some(offset));
        assert_eq!(
            app.highlighted().unwrap().pr.number,
            2 * (offset as u64 + 1)
        );
    }

    #[test]
    fn a_click_on_a_checkbox_toggles_it_and_highlights_the_row() {
        let mut app = app_with(vec![
            pr(1, "Bump alpha", "OPEN", ""),
            pr(2, "Bump beta", "OPEN", ""),
        ]);
        draw(&mut app, 200, 30);
        // The checkbox of the second row, while the first is highlighted.
        click_checkbox(&mut app, 1);
        assert_eq!(app.selected, HashSet::from([2]));
        assert_eq!(app.table.selected(), Some(1));
        let buffer = draw(&mut app, 200, 30);
        assert!(
            lines(&buffer)
                .iter()
                .any(|l| l.contains("[x]") && l.contains("#2"))
        );
        click_checkbox(&mut app, 1);
        assert!(app.selected.is_empty());

        // Just beside the checkbox is the row itself: it highlights and selects nothing.
        click_table_row(&mut app, 0, 3);
        assert!(app.selected.is_empty());
        assert_eq!(app.table.selected(), Some(0));

        // Space acts on the same selection, and a checkbox click adds to it.
        press(&mut app, " ");
        click_checkbox(&mut app, 1);
        assert_eq!(app.selected, HashSet::from([1, 2]));
    }

    #[test]
    fn a_click_on_the_header_or_below_the_rows_does_nothing() {
        let mut app = app_with(vec![
            pr(1, "Bump alpha", "OPEN", ""),
            pr(2, "Bump beta", "OPEN", ""),
        ]);
        draw(&mut app, 200, 30);
        let rows = app.drawn.areas.table_rows;
        click_table_row(&mut app, 1, 5);
        assert_eq!(app.table.selected(), Some(1));
        press_at(&mut app, Position::new(rows.x + 5, rows.y - 1));
        press_at(&mut app, Position::new(rows.x + 5, rows.y + 5));
        assert_eq!(app.table.selected(), Some(1));
    }

    #[test]
    fn the_table_has_a_scrollbar_only_with_more_rows_than_fit() {
        let mut app = app_with((1..=80).map(|n| pr(n, "Bump", "OPEN", "")).collect());
        let buffer = draw_wide(&mut app);
        assert!(has_scrollbar(&buffer, app.drawn.areas.table));

        press(&mut app, "f#80");
        let buffer = draw_wide(&mut app);
        assert!(!has_scrollbar(&buffer, app.drawn.areas.table));
    }

    #[test]
    fn the_list_scrolls_sideways_keeping_the_checkbox_and_number() {
        let mut app = app_with_a_long_title();
        let buffer = draw_narrow(&mut app);
        let table = app.drawn.areas.table;
        assert!(!pane_row(&buffer, table, 1).contains("END"));

        for _ in 0..40 {
            key(&mut app, KeyCode::Right);
        }
        let buffer = draw_narrow(&mut app);
        let row = pane_row(&buffer, table, 1);
        assert!(row.starts_with("[ ] #1 "), "the first columns stay: {row}");
        assert!(row.trim_end().ends_with("END"), "{row}");
        assert!(!row.contains("alice"), "the author scrolled away: {row}");
        // The header scrolls with the rows, and keeps the number's.
        assert!(pane_row(&buffer, table, 0).trim_start().starts_with('#'));
        assert!(!pane_row(&buffer, table, 0).contains("Author"));
        // The second PR's row is still there under its number.
        assert!(pane_row(&buffer, table, 2).starts_with("[ ] #2 "));

        for _ in 0..40 {
            key(&mut app, KeyCode::Left);
        }
        let buffer = draw_narrow(&mut app);
        assert!(pane_row(&buffer, table, 1).contains("alice"));
    }

    #[test]
    fn the_lists_scroll_stays_when_the_highlight_moves() {
        let mut app = app_with_a_long_title();
        draw_narrow(&mut app);
        for _ in 0..3 {
            key(&mut app, KeyCode::Right);
        }
        let x = app.list_x;
        assert!(x > 0);
        press(&mut app, "j");
        draw_narrow(&mut app);
        assert_eq!(app.list_x, x);
    }

    #[test]
    fn a_list_that_fits_does_not_scroll_sideways() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        for _ in 0..5 {
            key(&mut app, KeyCode::Right);
        }
        let buffer = draw_narrow(&mut app);
        assert_eq!(app.list_x, 0);
        assert!(pane_row(&buffer, app.drawn.areas.table, 1).contains("alice"));
    }

    mod boxes;
}
