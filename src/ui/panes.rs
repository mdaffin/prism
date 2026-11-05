use super::*;

/// The characters of a horizontal scrollbar. The track is the border's own, so only the thumb
/// shows.
pub(super) const HORIZONTAL_THUMB: &str = "▬";

pub(super) const HORIZONTAL_TRACK: &str = "─";

/// The width of the checks pane's status column, which fits `… pending` and a space.
pub(super) const CHECK_STATUS_WIDTH: u16 = 10;

/// A bordered pane, with a cyan border and title while it has focus.
pub(super) fn pane_block<'a>(title: impl Into<Line<'a>>, focused: bool) -> Block<'a> {
    let block = Block::bordered().title(title);
    // Not `block.cyan()`, which would colour the text inside the pane as well.
    if focused {
        block
            .border_style(Style::new().cyan())
            .title_style(Style::new().cyan())
    } else {
        block
    }
}

/// While `pane` has focus, sets the rows a PgUp/PgDn moves: its `height` less the `kept` rows
/// that stay in view, and at least one.
pub(super) fn set_page(app: &mut App, pane: Pane, height: u16, kept: u16) {
    if app.focus == pane {
        app.drawn.page = height.saturating_sub(kept).max(1);
    }
}

/// A scrollbar down the right border of `area`, drawn only while the pane's `content` rows don't
/// all fit in its `viewport`. `offset` is the first row shown.
pub(super) fn scrollbar(
    frame: &mut Frame,
    area: Rect,
    content: usize,
    viewport: usize,
    offset: usize,
) -> Option<Rect> {
    let margin = Margin::new(0, 1);
    render_scrollbar(
        frame,
        area.inner(margin),
        ScrollbarOrientation::VerticalRight,
        (content, viewport, offset),
    )
}

/// A scrollbar along the bottom border of `area`, drawn only while the pane's `content` columns
/// don't all fit in its `viewport`. `offset` is the first column shown.
pub(super) fn hscrollbar(
    frame: &mut Frame,
    area: Rect,
    content: usize,
    viewport: usize,
    offset: usize,
) -> Option<Rect> {
    let margin = Margin::new(1, 0);
    render_scrollbar(
        frame,
        area.inner(margin),
        ScrollbarOrientation::HorizontalBottom,
        (content, viewport, offset),
    )
}

/// Draws a scrollbar on the edge of `area`, which is already inside the corners of the border,
/// for the `(content, viewport, offset)` lengths along its axis. Returns the cells of its track, or
/// `None` when nothing was drawn because the content fits.
pub(super) fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    orientation: ScrollbarOrientation,
    (content, viewport, offset): (usize, usize, usize),
) -> Option<Rect> {
    if content <= viewport || area.is_empty() {
        return None;
    }
    // The position runs from 0 to the last offset, which is when the thumb reaches the end.
    let mut state = ScrollbarState::new(content - viewport + 1)
        .viewport_content_length(viewport)
        .position(offset);
    let horizontal = orientation == ScrollbarOrientation::HorizontalBottom;
    let mut bar = Scrollbar::new(orientation)
        .begin_symbol(None)
        .end_symbol(None);
    if horizontal {
        bar = bar
            .thumb_symbol(HORIZONTAL_THUMB)
            .track_symbol(Some(HORIZONTAL_TRACK));
    }
    frame.render_stateful_widget(bar, area, &mut state);
    Some(if horizontal {
        Rect::new(area.x, area.bottom() - 1, area.width, 1)
    } else {
        Rect::new(area.right() - 1, area.y, 1, area.height)
    })
}

/// Draws the vertical scrollbar of the pane that `bar` is the target of, and records where it is
/// for the mouse.
pub(super) fn pane_scrollbar(
    app: &mut App,
    frame: &mut Frame,
    bar: ScrollTarget,
    area: Rect,
    (content, viewport, offset): (usize, usize, usize),
) {
    if let Some(track) = scrollbar(frame, area, content, viewport, offset) {
        app.drawn.areas.scrollbars.push(ScrollbarHit {
            target: bar,
            vertical: true,
            track,
            content,
            viewport,
            offset,
        });
    }
}

/// Like [`pane_scrollbar`], for the horizontal one.
pub(super) fn pane_hscrollbar(
    app: &mut App,
    frame: &mut Frame,
    bar: ScrollTarget,
    area: Rect,
    (content, viewport, offset): (usize, usize, usize),
) {
    if let Some(track) = hscrollbar(frame, area, content, viewport, offset) {
        app.drawn.areas.scrollbars.push(ScrollbarHit {
            target: bar,
            vertical: false,
            track,
            content,
            viewport,
            offset,
        });
    }
}

/// The furthest a pane can scroll: until its last row is at the bottom.
pub(super) fn max_scroll(content_rows: usize, area_rows: u16) -> u16 {
    let max = content_rows.saturating_sub(usize::from(area_rows));
    to_u16(max)
}

/// `title` with the `parts` in brackets after it, or alone when there are none.
pub(super) fn with_counts(title: &str, parts: &[String]) -> String {
    if parts.is_empty() {
        title.to_string()
    } else {
        format!("{title} ({})", parts.join(", "))
    }
}

/// A pane's title with how many items it holds, if any.
pub(super) fn counted_title(title: &str, count: usize) -> String {
    let parts: Vec<String> = (count > 0).then(|| count.to_string()).into_iter().collect();
    with_counts(title, &parts)
}

/// The details of the highlighted PR, once loaded.
pub(super) fn loaded_details(app: &App) -> Option<&crate::gh::PrDetails> {
    match app.details.get(&app.highlighted()?.pr.number)? {
        Fetch::Loaded(loaded) => Some(&loaded.details),
        _ => None,
    }
}

pub(super) fn checks(app: &mut App, frame: &mut Frame, area: Rect) {
    app.drawn.areas.checks = area;
    app.drawn.checks_layout = ChecksLayout::default();
    let conflict_row = app
        .highlighted()
        .is_some_and(|r| r.checks.state == CheckState::Conflict);
    let count = app
        .highlighted()
        .map_or(0, |r| r.checks.worst_first().len())
        + usize::from(conflict_row);
    let block = pane_block(counted_title("Checks", count), app.focus == Pane::Checks);
    let inner = block.inner(area);
    set_page(app, Pane::Checks, inner.height, 2);
    frame.render_widget(block, area);
    let Some(row) = app.highlighted() else {
        return;
    };
    // A conflict outranks the checks, so it gets a row of its own at the top.
    let conflict = (row.checks.state == CheckState::Conflict).then_some("Merge conflict");
    let first = usize::from(conflict.is_some());
    let results = row.checks.worst_first();
    let selected = app.scroll.check.min(results.len().saturating_sub(1));
    // The names scroll sideways and the status stays. The gap between the columns is one.
    let names_width = inner.width.saturating_sub(CHECK_STATUS_WIDTH + 1);
    let widest = results
        .iter()
        .map(|result| result.key.width())
        .chain(conflict.map(|name| name.width()))
        .max()
        .unwrap_or(0);
    let x = usize::from(app.scroll.checks_x).min(widest.saturating_sub(usize::from(names_width)));
    let name = |name: &str| model::skip_columns(name, x);
    let conflict = conflict.map(|text| styled_check_row(CheckState::Conflict, name(text)));
    let rows: Vec<Row> = conflict
        .into_iter()
        .chain(results.iter().enumerate().map(|(i, result)| {
            let row = styled_check_row(result.state, name(&result.key));
            if i == selected { row.reversed() } else { row }
        }))
        .collect();
    let count = results.len();
    if rows.is_empty() {
        frame.render_widget(Line::from("No checks."), inner);
        return;
    }
    let rows_count = rows.len();
    // One row of the table is its header.
    let scroll = app
        .scroll
        .checks
        .min(max_scroll(rows.len(), inner.height.saturating_sub(1)));
    let table = Table::new(
        rows,
        [Constraint::Length(CHECK_STATUS_WIDTH), Constraint::Fill(1)],
    )
    .header(Row::new(["Status", "Check"]).bold());
    let mut state = TableState::default().with_offset(usize::from(scroll));
    frame.render_stateful_widget(table, inner, &mut state);
    let viewport = usize::from(inner.height.saturating_sub(1));
    pane_scrollbar(
        app,
        frame,
        ScrollTarget::Checks,
        area,
        (rows_count, viewport, usize::from(scroll)),
    );
    pane_hscrollbar(
        app,
        frame,
        ScrollTarget::ChecksX,
        area,
        (widest, usize::from(names_width), x),
    );
    app.drawn.checks_layout = ChecksLayout {
        count,
        first,
        viewport: usize::from(inner.height.saturating_sub(1)),
    };
    app.scroll.check = selected;
    app.scroll.checks = scroll;
    app.scroll.checks_x = to_u16(x);
}

/// What the approvers pane says about the highlighted PR: nothing without one, and otherwise who
/// has reviewed and who has been asked to, one to a line, with a mark and a colour for how it
/// stands, as in the checks pane, and what else is worth knowing.
pub(super) fn approvers_text(app: &App) -> Text<'static> {
    match app.highlighted().map(|row| app.details.get(&row.pr.number)) {
        None => Text::default(),
        Some(None | Some(Fetch::Loading(_))) => Text::from("Loading…"),
        Some(Some(Fetch::Failed(error))) => {
            let error = model::first_line(error);
            Text::from(format!("Failed to load details: {error}")).red()
        }
        Some(Some(Fetch::Loaded(Details { details, .. }))) => {
            if details.approvers == Approvers::default() {
                Text::from("No reviews or review requests.")
            } else {
                Text::from(approver_lines(&details.approvers))
            }
        }
    }
}

/// The rows the approvers pane would like when it is `width` columns wide, border included: one
/// for each line as wrapped, between [`MIN_APPROVERS_ROWS`] and [`MAX_APPROVERS_ROWS`].
pub(super) fn approvers_rows(app: &App, width: u16) -> u16 {
    let inner = usize::from(width.saturating_sub(2));
    let lines = model::layout_plain(&approvers_text(app), inner).lines.len();
    (to_u16(lines) + 2).clamp(MIN_APPROVERS_ROWS, MAX_APPROVERS_ROWS)
}

/// The approvers pane, above the diff.
pub(super) fn approvers(app: &mut App, frame: &mut Frame, area: Rect) {
    app.drawn.areas.approvers = area;
    let count = loaded_details(app).map_or(0, |d| {
        d.approvers.approvals.len() + d.approvers.requests.len()
    });
    let block = pane_block(
        counted_title("Approvers", count),
        app.focus == Pane::Approvers,
    );
    let inner = block.inner(area);
    set_page(app, Pane::Approvers, inner.height, 1);
    frame.render_widget(block, area);
    let text = approvers_text(app);
    // Wrapped here, as with the description, so the scrollbar knows how many rows there are.
    let laid = model::layout_plain(&text, usize::from(inner.width));
    let rows = laid.lines.len();
    let scroll = app.scroll.approvers.min(max_scroll(rows, inner.height));
    frame.render_widget(
        Paragraph::new(Text::from(laid.lines)).scroll((scroll, 0)),
        inner,
    );
    pane_scrollbar(
        app,
        frame,
        ScrollTarget::Approvers,
        area,
        (rows, usize::from(inner.height), usize::from(scroll)),
    );
    app.scroll.approvers = scroll;
}

/// The approvers pane's lines: those who asked for changes, then those who approved, those whose
/// review was dismissed, and then those still asked, code owners first.
pub(super) fn approver_lines(approvers: &Approvers) -> Vec<Line<'static>> {
    let name = |reviewer: &Reviewer| match reviewer {
        Reviewer::Person(login) => format!("@{login}"),
        Reviewer::Team(slug) => slug.clone(),
        Reviewer::Hidden => "a team or reviewer you can't see".to_string(),
    };
    let mut lines = Vec::new();
    for verdict in [
        Verdict::ChangesRequested,
        Verdict::Approved,
        Verdict::Dismissed,
    ] {
        for approval in approvers.approvals.iter().filter(|a| a.verdict == verdict) {
            // A review of an older commit may not count, so it is yellow like a pending check.
            let (mark, label, style) = match (verdict, approval.stale) {
                (Verdict::ChangesRequested, _) => ("✗", "changes requested", Style::new().red()),
                (Verdict::Approved, false) => ("✓", "approved", Style::new().green()),
                (Verdict::Approved, true) => ("✓", "approved", Style::new().yellow()),
                _ => ("○", "dismissed", Style::new().dim()),
            };
            let mut detail = vec![label.to_string()];
            if approval.stale {
                detail.push("on an older commit".to_string());
            }
            if !approval.for_teams.is_empty() {
                detail.push(format!("for {}", approval.for_teams.join(", ")));
            }
            lines.push(Line::from(vec![
                Span::styled(format!("{mark} "), style),
                Span::raw(format!("{} · ", name(&approval.by))),
                Span::styled(detail.join(" · "), style),
            ]));
        }
    }
    let code_owners = approvers.requests.iter().filter(|r| r.code_owner);
    let others = approvers.requests.iter().filter(|r| !r.code_owner);
    for request in code_owners.chain(others) {
        let label = if request.code_owner {
            "waiting · code owner"
        } else {
            "waiting"
        };
        let style = Style::new().yellow();
        lines.push(Line::from(vec![
            Span::styled("… ", style),
            Span::raw(format!("{} · ", name(&request.of))),
            Span::styled(label, style),
        ]));
    }
    lines
}

/// A row of the checks pane, with its status coloured.
pub(super) fn styled_check_row<'a>(state: CheckState, name: String) -> Row<'a> {
    Row::new([
        Cell::new(state.label()).style(state_style(state)),
        Cell::new(name),
    ])
}

/// The comments of the highlighted PR, as one scrolling document of headers that expand to show
/// the comment under them.
pub(super) fn comments(app: &mut App, frame: &mut Frame, area: Rect) {
    app.drawn.areas.comments = area;
    app.drawn.comment_layout = CommentLayout::default();
    app.drawn.comment_links = Vec::new();
    app.drawn.comment_text = TextView::default();
    let count = loaded_details(app).map_or(0, |d| d.comments.len());
    let block = pane_block(
        counted_title("Comments", count),
        app.focus == Pane::Comments,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (comments, bodies) = match app.highlighted().map(|row| app.details.get(&row.pr.number)) {
        None => return,
        Some(None | Some(Fetch::Loading(_))) => {
            frame.render_widget(Line::from("Loading…"), inner);
            return;
        }
        Some(Some(Fetch::Failed(error))) => {
            let error = model::first_line(error);
            let text = Text::from(format!("Failed to load details: {error}")).red();
            frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            return;
        }
        Some(Some(Fetch::Loaded(Details {
            details,
            comment_bodies,
            ..
        }))) => (&details.comments, comment_bodies),
    };
    if comments.is_empty() {
        frame.render_widget(Line::from("No comments."), inner);
        return;
    }
    let selected = app.scroll.comment.min(comments.len() - 1);
    let width = usize::from(inner.width);
    let widest = (0..comments.len())
        .filter(|i| app.scroll.comments_open.contains(i))
        .map(|i| model::widest_table(&bodies[i].text))
        .max()
        .unwrap_or(0);
    let x = usize::from(app.scroll.comment_x).min(widest.saturating_sub(width));

    let CommentDocument {
        lines: document,
        items,
        links,
        rows,
    } = comment_document(
        comments,
        bodies,
        &app.scroll.comments_open,
        selected,
        inner.width,
        x,
    );
    let total = items.last().map_or(0, |&(start, height)| start + height);
    // Already wrapped, so the links can be found from where they are drawn.
    let pane = Paragraph::new(Text::from(document));
    let scroll = app.scroll.comments.min(max_scroll(total, inner.height));
    frame.render_widget(pane.scroll((scroll, 0)), inner);
    let view = TextView {
        inner,
        scroll: usize::from(scroll),
        rows: std::sync::Arc::new(rows),
        hscroll: 0,
    };
    highlight_selection(app, frame, Pane::Comments, &view);
    app.drawn.comment_text = view;
    pane_scrollbar(
        app,
        frame,
        ScrollTarget::Comments,
        area,
        (total, usize::from(inner.height), usize::from(scroll)),
    );

    set_page(app, Pane::Comments, inner.height, 1);
    app.drawn.comment_layout = CommentLayout {
        items,
        viewport: usize::from(inner.height),
    };
    app.drawn.comment_links = links;
    app.scroll.comment = selected;
    app.scroll.comment_x = to_u16(x);
    app.scroll.comments = scroll;
}

/// The comments as one document, as laid out for a pane.
pub(super) struct CommentDocument {
    lines: Vec<Line<'static>>,
    /// Each comment's first line and height, counted as wrapped to the width, as that layout is
    /// what `update` steps through.
    items: Vec<(usize, usize)>,
    links: Vec<Vec<model::LinkSpan>>,
    /// The rows as text, for selecting.
    rows: Vec<TextRow>,
}

/// The comments as one document: a header per comment, and the body under it when it is open.
pub(super) fn comment_document(
    comments: &[Comment],
    bodies: &[model::Markdown],
    open: &HashSet<usize>,
    selected: usize,
    width: u16,
    x: usize,
) -> CommentDocument {
    let now = jiff::Timestamp::now();
    let width = usize::from(width);
    let mut document = Vec::new();
    let mut links = Vec::new();
    let mut rows = Vec::new();
    let mut items = Vec::with_capacity(comments.len());
    for (i, comment) in comments.iter().enumerate() {
        let is_open = open.contains(&i);
        let author = comment
            .author
            .as_ref()
            .map_or("ghost", |a| a.login.as_str());
        let marker = if is_open { '▼' } else { '▶' };
        // Who, what it is if it isn't a plain comment, and when.
        let mut parts = vec![format!("@{author}")];
        parts.extend(comment.tags());
        parts.push(model::ago(&comment.created_at, now));
        let mut header = Line::from(format!("{marker} {}", parts.join(" · ")));
        if i == selected {
            header = header.reversed();
        }
        let start = document.len();
        let (header_rows, row_links, joins) = model::wrap_rows(&header, &[], width);
        for (row, join) in header_rows.iter().zip(joins) {
            rows.push(TextRow {
                text: line_text(row),
                join,
                table: false,
                header: true,
            });
        }
        document.extend(header_rows);
        links.extend(row_links);
        if is_open {
            let laid = model::layout_markdown(&bodies[i], width, x);
            for ((row, &join), &table) in laid.lines.iter().zip(&laid.joins).zip(&laid.tables) {
                rows.push(TextRow {
                    text: line_text(row),
                    join,
                    table,
                    header: false,
                });
            }
            document.extend(laid.lines);
            links.extend(laid.links);
            document.push(Line::default());
            links.push(Vec::new());
            rows.push(TextRow::default());
        }
        items.push((start, document.len() - start));
    }
    CommentDocument {
        lines: document,
        items,
        links,
        rows,
    }
}

pub(super) fn diff(app: &mut App, frame: &mut Frame, area: Rect) {
    app.drawn.areas.diff = area;
    let block = pane_block("Diff", app.focus == Pane::Diff);
    let inner = block.inner(area);
    set_page(app, Pane::Diff, inner.height, 1);
    app.drawn.diff_text = TextView::default();
    let diff = app
        .highlighted()
        .and_then(|row| app.diffs.get(&row.pr.number));
    let mut scroll = 0;
    let mut x = 0;
    let mut widest = 0;
    let mut lines = 0;
    // What a drag can select, once the diff is there: its lines, made when it loaded.
    let mut rows = None;
    // A failure is a message, so it wraps; a diff is cut off at the pane's edge.
    let mut wrap = false;
    let text = match diff {
        None if app.highlighted().is_none() => Text::default(),
        // A PR waiting for a free fetch slot has no entry yet.
        None | Some(Fetch::Loading(_)) => Text::from("Loading…"),
        Some(Fetch::Loaded(diff)) => {
            lines = diff.text.lines.len();
            widest = diff.widest;
            rows = Some(diff.rows.clone());
            scroll = app
                .scroll
                .diff
                .min(max_scroll(diff.text.lines.len(), inner.height));
            x = app.scroll.diff_x.min(max_scroll(diff.widest, inner.width));
            // Only the rows that fit are copied, as a diff can be very long.
            Text::from(
                diff.text
                    .lines
                    .iter()
                    .skip(usize::from(scroll))
                    .take(usize::from(inner.height))
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        }
        Some(Fetch::Failed(error)) => {
            wrap = true;
            let error = model::first_line(error);
            Text::from(format!("Failed to load diff: {error}")).red()
        }
    };
    let mut pane = Paragraph::new(text).scroll((0, x)).block(block);
    if wrap {
        pane = pane.wrap(Wrap { trim: false });
    }
    frame.render_widget(pane, area);
    if let Some(rows) = rows {
        let view = TextView {
            inner,
            scroll: usize::from(scroll),
            rows,
            hscroll: usize::from(x),
        };
        highlight_selection(app, frame, Pane::Diff, &view);
        app.drawn.diff_text = view;
    }
    pane_scrollbar(
        app,
        frame,
        ScrollTarget::Diff,
        area,
        (lines, usize::from(inner.height), usize::from(scroll)),
    );
    pane_hscrollbar(
        app,
        frame,
        ScrollTarget::DiffX,
        area,
        (widest, usize::from(inner.width), usize::from(x)),
    );
    app.scroll.diff = scroll;
    app.scroll.diff_x = x;
}

/// The highlighted PR's title and its summary line: checks, review, size and branch.
pub(super) fn pull_request(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::bordered().title("Pull request");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(row) = app.highlighted() else {
        return;
    };
    let mut summary = vec![
        Span::styled(row.checks_label(), checks_style(row)),
        Span::raw("  "),
        Span::raw(model::review_label(row.pr.review_decision)),
    ];
    // The size and branch come with the details.
    match app.details.get(&row.pr.number) {
        Some(Fetch::Loaded(Details { details, .. })) => summary.extend([
            Span::raw("  "),
            Span::styled(format!("+{}", details.additions), Style::new().green()),
            Span::raw(" "),
            Span::styled(format!("-{}", details.deletions), Style::new().red()),
            Span::raw(" lines  "),
            Span::raw(details.head_ref_name.as_str()),
        ]),
        None | Some(Fetch::Loading(_)) => summary.push(Span::raw("  Loading…")),
        Some(Fetch::Failed(_)) => {}
    }
    let title = Line::from(format!("#{} {}", row.pr.number, row.pr.title)).bold();
    frame.render_widget(Paragraph::new(vec![title, Line::from(summary)]), inner);
}

/// The text of a laid-out row, as drawn.
pub(super) fn line_text(line: &Line) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// Reverses the selected cells of `view`, the rows a pane drew, if the selection is in `pane`.
pub(super) fn highlight_selection(app: &App, frame: &mut Frame, pane: Pane, view: &TextView) {
    let Some(selection) = app.selection().filter(|s| s.pane == pane) else {
        return;
    };
    for y in 0..usize::from(view.inner.height) {
        let Some(row) = view.rows.get(view.scroll + y) else {
            break;
        };
        let Some((from, to)) = selection.columns_on(view.scroll + y, App::row_width(row)) else {
            continue;
        };
        // The text may be scrolled sideways, past what is shown, which the selection can be too.
        let (from, to) = (
            from.saturating_sub(view.hscroll),
            to.saturating_sub(view.hscroll)
                .min(usize::from(view.inner.width)),
        );
        if from >= to {
            continue;
        }
        let x = view.inner.x + to_u16(from);
        let width = to_u16(to - from).min(view.inner.right().saturating_sub(x));
        let cells = Rect::new(x, view.inner.y + to_u16(y), width, 1);
        frame.buffer_mut().set_style(cells, Modifier::REVERSED);
    }
}

pub(super) fn description(app: &mut App, frame: &mut Frame, area: Rect) {
    app.drawn.areas.description = area;
    app.drawn.description_links = Vec::new();
    // The block's inner area doesn't depend on its title.
    let inner = pane_block("", false).inner(area);
    set_page(app, Pane::Description, inner.height, 1);
    let width = usize::from(inner.width);
    let mut widest = 0;
    let mut x = 0;
    let laid = match app.highlighted() {
        None => model::Laid::default(),
        Some(row) => match app.details.get(&row.pr.number) {
            None | Some(Fetch::Loading(_)) => model::layout_plain(&Text::from("Loading…"), width),
            Some(Fetch::Loaded(Details { body, .. })) => {
                widest = model::widest_table(&body.text);
                x = usize::from(app.scroll.description_x).min(widest.saturating_sub(width));
                model::layout_markdown(body, width, x)
            }
            Some(Fetch::Failed(error)) => {
                let error = model::first_line(error);
                let text = Text::from(format!("Failed to load details: {error}")).red();
                model::layout_plain(&text, width)
            }
        },
    };
    app.scroll.description_x = to_u16(x);
    let title = if widest > width {
        "Description (←/→ scroll tables)"
    } else {
        "Description"
    };
    let block = pane_block(title, app.focus == Pane::Description);
    let lines = laid.lines.len();
    let scroll = app.scroll.description.min(max_scroll(lines, inner.height));
    // What a drag can select: the rows as drawn.
    let rows = laid
        .lines
        .iter()
        .zip(&laid.joins)
        .zip(&laid.tables)
        .map(|((line, &join), &table)| TextRow {
            text: line_text(line),
            join,
            table,
            header: false,
        })
        .collect::<Vec<_>>();
    let view = TextView {
        inner,
        scroll: usize::from(scroll),
        rows: std::sync::Arc::new(rows),
        hscroll: 0,
    };
    // Already wrapped, so the links can be found from where they are drawn.
    let pane = Paragraph::new(Text::from(laid.lines)).scroll((scroll, 0));
    frame.render_widget(pane.block(block), area);
    highlight_selection(app, frame, Pane::Description, &view);
    pane_scrollbar(
        app,
        frame,
        ScrollTarget::Description,
        area,
        (lines, usize::from(inner.height), usize::from(scroll)),
    );
    app.scroll.description = scroll;
    app.drawn.description_links = laid.links;
    app.drawn.description_text = view;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::*;

    #[test]
    fn colours_the_checks_pane_and_shows_a_conflict_first() {
        let mut app = one_of_each();
        // The conflicting PR is last.
        press(&mut app, "jjjj");
        let buffer = draw_wide(&mut app);
        let screen = lines(&buffer);

        let conflict = find(&buffer, "Merge conflict").unwrap();
        let status = Position::new(conflict.x - (CHECK_STATUS_WIDTH + 1), conflict.y);
        assert!(screen[usize::from(status.y)].contains("✗ conflict"));
        assert_eq!(buffer[status].fg, Color::Red);
        // Above the check that passed, which is the only other row.
        let build = find(&buffer, "CI/build").unwrap();
        assert!(build.y > conflict.y);
        let passed = Position::new(build.x - (CHECK_STATUS_WIDTH + 1), build.y);
        assert!(screen[usize::from(passed.y)].contains("✓ passing"));
        assert_eq!(buffer[passed].fg, Color::Green);
    }

    #[test]
    fn a_conflict_with_no_checks_still_shows_the_conflict() {
        let mut app = app_with(vec![pr_checked(1, "CONFLICTING", vec![])]);
        let buffer = draw_wide(&mut app);
        assert!(find(&buffer, "Merge conflict").is_some());
        assert!(find(&buffer, "No checks.").is_none());
    }

    #[test]
    fn the_pull_request_box_shows_the_title_and_a_summary() {
        let mut app = app_with(vec![pr(7, "Bump example-lib", "OPEN", "APPROVED")]);
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(shows(&screen, "Pull request"));
        // Until the details load, only what the list knows is shown.
        let summary = screen
            .iter()
            .find(|line| line.contains("✓ approved") && line.contains("no checks"));
        assert!(summary.unwrap().contains("Loading…"));

        let details = crate::gh::PrDetails {
            head_ref_name: "renovate/example-lib".to_string(),
            additions: 12,
            deletions: 3,
            ..pr_details("", serde_json::json!([]))
        };
        app.details_loaded(7, Ok(details));
        let buffer = draw(&mut app, 200, 30);
        let screen = lines(&buffer);
        let title = screen
            .iter()
            .find(|line| line.contains("#7 Bump example-lib"))
            .unwrap();
        assert!(!title.contains("[ ]"));
        let summary = screen
            .iter()
            .find(|line| line.contains("+12 -3 lines  renovate/example-lib"))
            .unwrap();
        assert!(summary.contains("· no checks") && summary.contains("✓ approved"));
        let added = find(&buffer, "+12").unwrap();
        assert_eq!(buffer[added].fg, Color::Green);
        let removed = find(&buffer, "-3 lines").unwrap();
        assert_eq!(buffer[removed].fg, Color::Red);
    }

    /// An app whose description is `body`, and one open comment of `comment_body`, drawn at 200
    /// by 40.
    fn app_with_text(body: &str, comment_body: &str) -> App {
        let details = pr_details(body, serde_json::json!([comment("someone", comment_body)]));
        let mut app = app_with_details(
            vec![
                pr(1, "Bump alpha", "OPEN", ""),
                pr(2, "Bump beta", "OPEN", ""),
            ],
            details,
        );
        draw_wide(&mut app);
        app
    }

    /// The cell of row `row`, column `column` of a pane's text, as last drawn.
    fn text_cell(view: &TextView, row: usize, column: u16) -> Position {
        Position::new(
            view.inner.x + column,
            view.inner.y + u16::try_from(row - view.scroll).unwrap(),
        )
    }

    #[test]
    fn a_drag_over_wrapped_text_copies_it_as_the_one_line_it_was() {
        let body: String = (1..=90)
            .map(|n| format!("w{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        let mut app = app_with_text(&body, "");
        let view = app.drawn.description_text.clone();
        assert!(
            view.rows.len() > 2,
            "it should have wrapped: {}",
            view.rows.len()
        );
        let last = view.rows.len() - 1;
        let end_column = u16::try_from(view.rows[last].text.chars().count() - 1).unwrap();
        press_at(&mut app, text_cell(&view, 0, 0));
        drag_to(&mut app, text_cell(&view, last, end_column));
        release_at(&mut app, text_cell(&view, last, end_column));
        assert_eq!(app.copy_to_run().as_deref(), Some(body.as_str()));
        assert!(status_bar(&mut app, 160, 30).contains("Copied"));
        // The cells under the drag are drawn reversed, the others are not.
        let buffer = draw_wide(&mut app);
        let first = text_cell(&view, 0, 0);
        assert!(reversed(&buffer, first));
        // The border is never in it.
        let border = Position::new(view.inner.x - 1, view.inner.y);
        assert!(!reversed(&buffer, border));
    }

    #[test]
    fn a_selection_in_part_of_a_row_copies_only_that_part_and_y_copies_it_again() {
        let mut app = app_with_text("hello brave new world", "");
        let view = app.drawn.description_text.clone();
        // "brave" is columns 6 to 10; the drag goes from right to left.
        press_at(&mut app, text_cell(&view, 0, 10));
        drag_to(&mut app, text_cell(&view, 0, 6));
        release_at(&mut app, text_cell(&view, 0, 6));
        assert_eq!(app.copy_to_run().as_deref(), Some("brave"));
        // `y` in the description copies it again; elsewhere it is what it always was.
        app.focus = Pane::Description;
        press(&mut app, "y");
        assert_eq!(app.copy_to_run().as_deref(), Some("brave"));
        // Esc clears the selection, and then `y` is the URL's.
        key(&mut app, KeyCode::Esc);
        assert!(app.selection().is_none());
        draw_wide(&mut app);
        let buffer = draw_wide(&mut app);
        assert!(
            !buffer[text_cell(&view, 0, 6)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
    }

    #[test]
    fn a_drag_across_a_tables_box_copies_its_cells_with_tabs() {
        // A table in Markdown is drawn with its box characters by the renderer.
        let md = "intro\n\n| Name | Val |\n|---|---|\n| a | 1 |\n\noutro";
        let mut app = app_with_text(md, "");
        let view = app.drawn.description_text.clone();
        let last = view.rows.len() - 1;
        press_at(&mut app, text_cell(&view, 0, 0));
        drag_to(&mut app, text_cell(&view, last, 10));
        release_at(&mut app, text_cell(&view, last, 10));
        let copied = app.copy_to_run().unwrap();
        assert!(
            !copied.contains(|c| "┌┐└┘├┤┬┴┼─│".contains(c)),
            "{copied:?}"
        );
        assert!(copied.contains("Name\tVal"), "{copied:?}");
        assert!(copied.contains("a\t1"), "{copied:?}");
        assert!(
            copied.starts_with("intro\n") && copied.ends_with("outro"),
            "{copied:?}"
        );
    }

    #[test]
    fn a_drag_in_an_open_comment_selects_its_text_but_not_from_a_header_or_a_link() {
        let mut app = app_with_text("body", "first line of the comment");
        // Open the comment by clicking its header.
        let header = {
            let area = app.drawn.areas.comments;
            Position::new(area.x + 3, area.y + 1)
        };
        press_at(&mut app, header);
        draw_wide(&mut app);
        let view = app.drawn.comment_text.clone();
        assert!(view.rows[0].header && view.rows.len() > 1);
        // From the body: the text is selected.
        press_at(&mut app, text_cell(&view, 1, 0));
        drag_to(&mut app, text_cell(&view, 1, 4));
        release_at(&mut app, text_cell(&view, 1, 4));
        assert_eq!(app.copy_to_run().as_deref(), Some("first"));
        // From the header: it is a click, which toggles the comment, and selects nothing.
        press_at(&mut app, text_cell(&view, 0, 2));
        drag_to(&mut app, text_cell(&view, 1, 4));
        release_at(&mut app, text_cell(&view, 1, 4));
        assert!(app.copy_to_run().is_none());
        assert!(app.selection().is_none());
    }

    #[test]
    fn a_selection_is_dropped_by_a_press_elsewhere_and_belongs_to_one_pr() {
        let mut app = app_with_text("hello brave new world", "");
        let view = app.drawn.description_text.clone();
        press_at(&mut app, text_cell(&view, 0, 0));
        drag_to(&mut app, text_cell(&view, 0, 4));
        release_at(&mut app, text_cell(&view, 0, 4));
        assert!(app.selection().is_some());
        // Another PR highlighted: it is not that text any more.
        app.focus = Pane::Table;
        key(&mut app, KeyCode::Down);
        assert_eq!(app.highlighted().unwrap().pr.number, 2);
        assert!(app.selection().is_none());
        key(&mut app, KeyCode::Up);
        // Back on the first, the selection is what it was, until a press elsewhere clears it.
        assert!(app.selection().is_some());
        press_at(&mut app, Position::new(0, 0));
        assert!(app.selection().is_none());
    }

    #[test]
    fn a_drag_over_the_diff_copies_its_lines_with_their_markers() {
        let mut app = app_with_text("body", "");
        app.diff_loaded(
            1,
            Ok("@@ -1,2 +1,2 @@\n context line\n-removed line\n+added line\n".to_string()),
        );
        draw_wide(&mut app);
        let view = app.drawn.diff_text.clone();
        assert_eq!(view.rows.len(), 4);
        // From the middle of the second line to the end of the fourth.
        press_at(&mut app, text_cell(&view, 1, 9));
        drag_to(&mut app, text_cell(&view, 3, 30));
        release_at(&mut app, text_cell(&view, 3, 30));
        assert_eq!(app.focus, Pane::Diff);
        assert_eq!(
            app.copy_to_run().as_deref(),
            Some("line\n-removed line\n+added line")
        );
        // Drawn reversed where it was selected, and `y` copies it again.
        let buffer = draw_wide(&mut app);
        assert!(
            buffer[text_cell(&view, 2, 0)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
        assert!(
            !buffer[text_cell(&view, 0, 0)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
        press(&mut app, "y");
        assert!(app.copy_to_run().unwrap().starts_with("line\n"));
    }

    #[test]
    fn a_selection_in_a_diff_scrolled_sideways_is_of_the_text_under_the_pointer() {
        let mut app = app_with_text("body", "");
        let long = format!("+{}END", "x".repeat(300));
        app.diff_loaded(1, Ok(format!("{long}\n+short")));
        draw_wide(&mut app);
        let width = app.drawn.diff_text.inner.width;
        // Scroll to the end, where only the tail of the long line is on screen.
        for _ in 0..200 {
            app.focus = Pane::Diff;
            press(&mut app, "l");
            draw_wide(&mut app);
        }
        let view = app.drawn.diff_text.clone();
        assert!(view.hscroll > 100, "{}", view.hscroll);
        press_at(&mut app, text_cell(&view, 0, width - 3));
        drag_to(&mut app, text_cell(&view, 0, width - 1));
        release_at(&mut app, text_cell(&view, 0, width - 1));
        assert_eq!(app.copy_to_run().as_deref(), Some("END"));
        // The reversed cells are under the pointer, not at the start of the line.
        let buffer = draw_wide(&mut app);
        assert!(
            buffer[text_cell(&view, 0, width - 1)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
        assert!(
            !buffer[text_cell(&view, 0, 0)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
    }

    #[test]
    fn a_failed_or_loading_diff_has_nothing_to_select() {
        let mut app = app_with_text("body", "");
        draw_wide(&mut app);
        let inner = app.drawn.areas.diff;
        press_at(&mut app, Position::new(inner.x + 2, inner.y + 1));
        drag_to(&mut app, Position::new(inner.x + 9, inner.y + 1));
        release_at(&mut app, Position::new(inner.x + 9, inner.y + 1));
        assert!(app.selection().is_none());
        assert!(app.copy_to_run().is_none());
    }

    #[test]
    fn dragging_past_the_bottom_of_the_description_scrolls_it() {
        let body = vec!["a line"; 100].join("\n\n");
        let mut app = app_with_text(&body, "");
        let view = app.drawn.description_text.clone();
        press_at(&mut app, text_cell(&view, 0, 0));
        let below = Position::new(view.inner.x + 2, view.inner.bottom() + 1);
        for _ in 0..5 {
            drag_to(&mut app, below);
            draw_wide(&mut app);
        }
        assert_eq!(app.scroll.description, 5);
        release_at(&mut app, below);
        // The text from the first row to the last one shown is copied.
        let copied = app.copy_to_run().unwrap();
        assert!(copied.lines().count() > 10, "{copied:?}");
    }

    #[test]
    fn the_highlighted_check_is_drawn_and_goes_back_to_the_first_with_the_pr() {
        let mut app = app_with_long_panes();
        app.focus = Pane::Checks;
        press(&mut app, "jj");
        let buffer = draw_wide(&mut app);
        assert_eq!(app.scroll.check, 2);
        // The first row under the header is check 0; the highlight is on the third.
        let checks = app.drawn.areas.checks;
        let reversed = |row: u16| {
            buffer[(checks.x + 2, checks.y + 2 + row)]
                .modifier
                .contains(Modifier::REVERSED)
        };
        assert!(!reversed(0) && !reversed(1) && reversed(2));

        // It stays through redraws, and goes back to the first when the PR changes.
        draw_wide(&mut app);
        assert_eq!(app.scroll.check, 2);
        app.focus = Pane::Table;
        press(&mut app, "j");
        draw_wide(&mut app);
        assert_eq!(app.scroll.check, 0);
    }

    #[test]
    fn a_wide_table_in_the_description_scrolls_sideways() {
        let mut app = app_with(vec![pr_checked(1, "MERGEABLE", vec![])]);
        let wide = "x".repeat(120);
        let body = format!("Some prose\n\n| Name | Value |\n|---|---|\n| first | {wide}END |");
        let details = pr_details(&body, serde_json::json!([]));
        app.details_loaded(1, Ok(details));
        let screen = lines(&draw_wide(&mut app));
        assert!(shows(&screen, "scroll tables"));
        assert!(!shows(&screen, "END"));

        app.focus = Pane::Description;
        for _ in 0..20 {
            press(&mut app, "l");
        }
        let screen = lines(&draw_wide(&mut app));
        assert!(shows(&screen, "END"));
        // The prose stays where it was, and the scroll stops at the table's end.
        assert!(shows(&screen, "Some prose"));
        let at_end = app.scroll.description_x;
        press(&mut app, "l");
        draw_wide(&mut app);
        assert_eq!(app.scroll.description_x, at_end);

        // And it scrolls back.
        press(&mut app, "h");
        assert!(app.scroll.description_x < at_end);
    }

    /// `n` review requests of people called `{prefix}0`, `{prefix}1` and so on, as JSON.
    fn requests(prefix: &str, n: usize) -> serde_json::Value {
        (0..n)
            .map(|i| {
                serde_json::json!({"of": {"Person": format!("{prefix}{i}")}, "code_owner": false})
            })
            .collect()
    }

    /// Details with these approvals and requests (as JSON), and nothing else.
    fn details_with_approvers(approvers: serde_json::Value) -> crate::gh::PrDetails {
        crate::gh::PrDetails {
            approvers: serde_json::from_value(approvers).unwrap(),
            ..pr_details("", serde_json::json!([]))
        }
    }

    #[test]
    fn the_approvers_pane_grows_with_its_lines_up_to_a_limit() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let mut heights = Vec::new();
        for n in [2, 4, 30] {
            app.details_loaded(
                1,
                Ok(details_with_approvers(
                    serde_json::json!({"requests": requests("u", n)}),
                )),
            );
            draw(&mut app, 200, 60);
            heights.push(app.drawn.areas.approvers.height);
        }
        // Two rows of border, and then a line each, until the limit.
        assert_eq!(heights, [4, 6, MAX_APPROVERS_ROWS]);
    }

    #[test]
    fn a_short_terminal_keeps_most_of_the_details_for_the_description() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.details_loaded(
            1,
            Ok(details_with_approvers(
                serde_json::json!({"requests": requests("u", 30)}),
            )),
        );
        draw(&mut app, 100, 24);
        let areas = &app.drawn.areas;
        assert!(areas.description.height >= areas.approvers.height);
    }

    #[test]
    fn the_approvers_pane_marks_each_reviewer_by_how_it_stands() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let details = details_with_approvers(serde_json::json!({
            "approvals": [
                {"by": {"Person": "alice"}, "verdict": "APPROVED", "stale": false,
                 "for_teams": ["org/owners"]},
                {"by": {"Person": "bob"}, "verdict": "APPROVED", "stale": true},
                {"by": {"Person": "carol"}, "verdict": "CHANGES_REQUESTED", "stale": false},
            ],
            "requests": [
                {"of": {"Team": "org/owners"}, "code_owner": true},
                {"of": {"Person": "dave"}, "code_owner": false},
                {"of": "Hidden", "code_owner": true},
            ],
        }));
        app.details_loaded(1, Ok(details));
        let buffer = draw(&mut app, 200, 60);
        let screen = lines(&buffer);
        let has = |text: &str| shows(&screen, text);
        assert!(has("Approvers"));
        assert!(has("✗ @carol · changes requested"));
        assert!(has("✓ @alice · approved · for org/owners"));
        assert!(has("✓ @bob · approved · on an older commit"));
        assert!(has("… org/owners · waiting · code owner"));
        assert!(has("… @dave · waiting"));
        // It is long, so it wraps in the narrow pane.
        assert!(has("… a team or reviewer"));

        // The colours are the checks pane's: red, green, and yellow for what may not count.
        let colour = |text: &str| {
            let at = find(&buffer, text).unwrap();
            buffer[at].fg
        };
        assert_eq!(colour("✗ @carol"), Color::Red);
        assert_eq!(colour("✓ @alice"), Color::Green);
        assert_eq!(colour("✓ @bob"), Color::Yellow);
        assert_eq!(colour("… @dave"), Color::Yellow);

        // Changes requested come first, then approvals, and what is still asked comes last, the
        // code owners before the rest.
        let row = |text: &str| find(&buffer, text).unwrap().y;
        assert!(row("@carol") < row("@alice") && row("@alice") < row("@bob"));
        assert!(row("@bob") < row("org/owners · waiting"));
        assert!(row("org/owners · waiting") < row("@dave"));
    }

    #[test]
    fn the_approvers_pane_says_when_there_is_nothing_and_while_it_loads() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let screen = lines(&draw_wide(&mut app));
        assert!(shows(&screen, "Loading…"));
        app.details_loaded(1, Ok(details_with_approvers(serde_json::json!({}))));
        let screen = lines(&draw_wide(&mut app));
        assert!(shows(&screen, "No reviews or review requests."));
        app.details_loaded(1, Err(color_eyre::eyre::eyre!("boom")));
        let screen = lines(&draw_wide(&mut app));
        assert!(shows(&screen, "Failed to load details: boom"));
    }

    #[test]
    fn a_long_list_of_approvers_scrolls_with_a_scrollbar() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.details_loaded(
            1,
            Ok(details_with_approvers(
                serde_json::json!({"requests": requests("user", 30)}),
            )),
        );
        let buffer = draw_wide(&mut app);
        assert!(has_scrollbar(&buffer, app.drawn.areas.approvers));
        assert!(find(&buffer, "@user0 ").is_some() && find(&buffer, "@user29 ").is_none());
        focus(&mut app, Pane::Approvers);
        key(&mut app, KeyCode::End);
        let screen = lines(&draw_wide(&mut app));
        assert!(shows(&screen, "@user29 "));
        assert!(!shows(&screen, "@user0 "));
    }

    #[test]
    fn reviews_and_threads_are_headed_by_what_they_are_and_a_thread_shows_its_replies() {
        let mut app = app_with(vec![pr_checked(1, "MERGEABLE", vec![])]);
        let details = pr_details(
            "",
            serde_json::json!([
                {"author": {"login": "alice"}, "createdAt": "2026-01-02T03:00:00Z", "body": "plain"},
                {"author": {"login": "bob"}, "createdAt": "2026-01-02T04:00:00Z",
                 "body": "Looks good", "kind": {"Review": "APPROVED"}},
                {"author": {"login": "carol"}, "createdAt": "2026-01-02T05:00:00Z",
                 "body": "Why this?", "kind": {"Thread": {
                    "path": "src/main.rs", "line": 12, "resolved": true, "outdated": false}},
                 "replies": [
                    {"author": {"login": "dave"}, "createdAt": "2026-01-02T05:30:00Z",
                     "body": "Because."}]},
                {"author": {"login": "erin"}, "createdAt": "2026-01-02T06:00:00Z",
                 "body": "Old", "kind": {"Thread": {
                    "path": "src/lib.rs", "line": null, "resolved": false, "outdated": true}}},
            ]),
        );
        app.details_loaded(1, Ok(details));
        app.focus = Pane::Comments;
        let screen = lines(&draw_wide(&mut app));
        let has = |text: &str| shows(&screen, text);
        assert!(has("▶ @alice · "));
        assert!(!has("@alice · approved"));
        assert!(has("▶ @bob · approved · "));
        assert!(has("▶ @carol · src/main.rs:12 · 1 reply · resolved · "));
        assert!(has("▶ @erin · src/lib.rs · outdated · "));
        assert!(!has("Because."));

        // Opening the thread shows the first comment, then each reply under who wrote it.
        press(&mut app, "jj ");
        let screen = lines(&draw_wide(&mut app));
        let has = |text: &str| shows(&screen, text);
        assert!(has("Why this?"));
        assert!(has("@dave · 2026-01-02 05:30"));
        assert!(has("Because."));
    }

    #[test]
    fn an_open_comment_is_rendered_as_markdown_and_its_table_scrolls_sideways() {
        let mut app = app_with(vec![pr_checked(1, "MERGEABLE", vec![])]);
        let wide = "x".repeat(120);
        let table =
            format!("Some **bold** prose\n\n| Name | Value |\n|---|---|\n| a | {wide}END |");
        let details = pr_details(
            "",
            serde_json::json!([comment("someone", "first"), comment("someone", &table)]),
        );
        app.details_loaded(1, Ok(details));
        app.focus = Pane::Comments;
        // Collapsed, there are only the headers.
        let screen = lines(&draw_wide(&mut app));
        assert!(shows(&screen, "▶ @someone"));
        assert!(!shows(&screen, "Some bold prose"));

        press(&mut app, "j ");
        let screen = lines(&draw_wide(&mut app));
        assert!(shows(&screen, "▼ @someone"));
        assert!(shows(&screen, "Some bold prose"));
        assert!(!shows(&screen, "END"));

        for _ in 0..20 {
            press(&mut app, "l");
        }
        let screen = lines(&draw_wide(&mut app));
        assert!(shows(&screen, "END"));

        // Collapsing hides it again.
        press(&mut app, " ");
        let screen = lines(&draw_wide(&mut app));
        assert!(!shows(&screen, "Some bold prose"));
    }

    #[test]
    fn a_click_on_a_comment_header_highlights_and_toggles_it() {
        let mut app = app_with_long_panes();
        let comments = app.drawn.areas.comments;
        let header = |row: u16| Position::new(comments.x + 2, comments.y + 1 + row);

        // All collapsed: the headers are on consecutive rows.
        press_at(&mut app, header(1));
        assert_eq!(app.scroll.comment, 1);
        assert!(app.scroll.comments_open.contains(&1));
        assert_eq!(app.focus, Pane::Comments);
        draw_wide(&mut app);
        // The body is under the header now, and a click on it does nothing more.
        press_at(&mut app, header(2));
        assert_eq!(app.scroll.comment, 1);
        assert!(app.scroll.comments_open.contains(&1));
        // The border and the space below the last comment do nothing either.
        press_at(&mut app, Position::new(comments.x + 2, comments.y));
        assert!(app.scroll.comments_open.contains(&1));
        // A second click collapses it.
        press_at(&mut app, header(1));
        assert!(app.scroll.comments_open.is_empty());
    }

    #[test]
    fn a_click_on_a_header_in_a_scrolled_pane_finds_the_right_comment() {
        let mut app = app_with_long_panes();
        let comments = app.drawn.areas.comments;
        press_at(&mut app, Position::new(comments.x + 2, comments.y + 1));
        assert!(app.scroll.comments_open.contains(&0));
        draw_wide(&mut app);
        // Scroll to the end, where the pane's scroll has been clamped, so the second comment's
        // header is some way down the pane.
        let second = app.drawn.comment_layout.items[1].0;
        app.scroll.comments = u16::MAX;
        draw_wide(&mut app);
        let row = u16::try_from(second).unwrap() - app.scroll.comments;
        assert!(row > 0);
        press_at(
            &mut app,
            Position::new(comments.x + 2, comments.y + 1 + row),
        );
        assert_eq!(app.scroll.comment, 1);
        assert!(app.scroll.comments_open.contains(&1));
    }

    #[test]
    fn a_click_on_a_check_highlights_it_in_a_scrolled_pane() {
        let mut app = app_with_long_panes();
        let checks = app.drawn.areas.checks;
        // The first row under the header, then after scrolling the same row is a later check.
        press_at(&mut app, Position::new(checks.x + 2, checks.y + 3));
        assert_eq!(app.scroll.check, 1);
        app.scroll.checks = 5;
        draw_wide(&mut app);
        press_at(&mut app, Position::new(checks.x + 2, checks.y + 2));
        assert_eq!(app.scroll.check, 5);
        // The border and the header highlight nothing.
        press_at(&mut app, Position::new(checks.x + 2, checks.y));
        press_at(&mut app, Position::new(checks.x + 2, checks.y + 1));
        assert_eq!(app.scroll.check, 5);
        assert_eq!(app.focus, Pane::Checks);
    }

    #[test]
    fn a_pane_with_too_much_text_has_a_scrollbar() {
        let mut app = app_with_long_panes();
        // Open the first comment, whose body is long.
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Tab);
        press(&mut app, " ");
        let buffer = draw_wide(&mut app);
        let areas = &app.drawn.areas;
        for (name, area) in [
            ("checks", areas.checks),
            ("comments", areas.comments),
            ("description", areas.description),
            ("diff", areas.diff),
        ] {
            assert!(has_scrollbar(&buffer, area), "{name}");
        }
    }

    #[test]
    fn a_pane_whose_text_fits_has_no_scrollbar() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let buffer = draw_wide(&mut app);
        let areas = &app.drawn.areas;
        for (name, area) in [
            ("table", areas.table),
            ("checks", areas.checks),
            ("comments", areas.comments),
            ("description", areas.description),
            ("diff", areas.diff),
        ] {
            assert!(!has_scrollbar(&buffer, area), "{name}");
        }
    }

    #[test]
    fn the_scrollbar_thumb_moves_to_the_bottom_with_the_scroll() {
        let mut app = app_with_long_panes();
        let buffer = draw_wide(&mut app);
        let diff = app.drawn.areas.diff;
        let at_top = thumb(&buffer, diff, Axis::Vertical);
        assert_eq!(at_top.first(), Some(&(diff.y + 1)));
        assert!(at_top.last() < Some(&(diff.bottom() - 2)));

        for _ in 0..5 {
            key(&mut app, KeyCode::Tab);
        }
        key(&mut app, KeyCode::End);
        let buffer = draw_wide(&mut app);
        let at_bottom = thumb(&buffer, diff, Axis::Vertical);
        assert_eq!(at_bottom.last(), Some(&(diff.bottom() - 2)));
        assert!(at_bottom.first() > Some(&(diff.y + 1)));
    }

    #[test]
    fn the_diff_scrolls_sideways_to_the_end_of_its_longest_line() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.diff_loaded(1, Ok(format!("+{}END\n ctx", "x".repeat(300))));
        let buffer = draw(&mut app, 120, 40);
        let diff = app.drawn.areas.diff;
        assert!(pane_row(&buffer, diff, 0).starts_with("+xxx"));
        assert!(!pane_row(&buffer, diff, 0).contains("END"));

        focus(&mut app, Pane::Diff);
        for _ in 0..60 {
            key(&mut app, KeyCode::Right);
        }
        let buffer = draw(&mut app, 120, 40);
        // Stopped at the end of the line, not scrolled on into blank space.
        assert!(pane_row(&buffer, diff, 0).trim_end().ends_with("xxxEND"));
        assert!(pane_row(&buffer, diff, 0).starts_with("xxx"));
        assert_eq!(pane_row(&buffer, diff, 0).chars().last(), Some('D'));

        for _ in 0..60 {
            key(&mut app, KeyCode::Left);
        }
        let buffer = draw(&mut app, 120, 40);
        assert!(pane_row(&buffer, diff, 0).starts_with("+xxx"));
    }

    #[test]
    fn the_checks_scroll_their_names_sideways_and_keep_the_status() {
        let name = format!("{}END", "n".repeat(120));
        let mut app = app_with(vec![pr_checked(
            1,
            "MERGEABLE",
            vec![run(&name, "COMPLETED", "SUCCESS")],
        )]);
        draw(&mut app, 120, 40);
        focus(&mut app, Pane::Checks);
        let buffer = draw(&mut app, 120, 40);
        let checks = app.drawn.areas.checks;
        assert!(pane_row(&buffer, checks, 1).contains("passing"));
        assert!(!pane_row(&buffer, checks, 1).contains("END"));

        for _ in 0..30 {
            key(&mut app, KeyCode::Right);
        }
        let buffer = draw(&mut app, 120, 40);
        let row = pane_row(&buffer, checks, 1);
        assert!(row.contains("passing"), "the status stays: {row}");
        assert!(row.trim_end().ends_with("END"), "{row}");
        assert!(!row.contains("CI/"), "{row}");
    }

    fn has_horizontal_scrollbar(buffer: &Buffer, area: Rect) -> bool {
        !thumb(buffer, area, Axis::Horizontal).is_empty()
    }

    #[test]
    fn the_list_checks_and_diff_have_a_horizontal_scrollbar_only_when_cut_off() {
        let title = "long ".repeat(40);
        let mut app = app_with(vec![by("alice", 1, &title), by("bob", 2, "Short")]);
        app.diff_loaded(1, Ok(format!("+{}", "x".repeat(300))));
        app.diff_loaded(2, Ok("+short".to_string()));
        let buffer = draw_narrow(&mut app);
        let areas = &app.drawn.areas;
        assert!(has_horizontal_scrollbar(&buffer, areas.table));
        assert!(has_horizontal_scrollbar(&buffer, areas.diff));
        // Its checks are all short.
        assert!(!has_horizontal_scrollbar(&buffer, areas.checks));

        // The second PR's diff fits; the list still has the first PR's long title.
        press(&mut app, "j");
        let buffer = draw_narrow(&mut app);
        assert!(!has_horizontal_scrollbar(&buffer, app.drawn.areas.diff));
        assert!(has_horizontal_scrollbar(&buffer, app.drawn.areas.table));
    }

    #[test]
    fn a_long_check_name_gives_the_checks_a_horizontal_scrollbar() {
        let name = "n".repeat(150);
        let mut app = app_with(vec![pr_checked(
            1,
            "MERGEABLE",
            vec![run(&name, "COMPLETED", "SUCCESS")],
        )]);
        let buffer = draw_narrow(&mut app);
        assert!(has_horizontal_scrollbar(&buffer, app.drawn.areas.checks));
    }

    #[test]
    fn no_pane_has_a_horizontal_scrollbar_when_its_text_fits() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.diff_loaded(1, Ok("+added".to_string()));
        let buffer = draw_narrow(&mut app);
        let areas = &app.drawn.areas;
        for (name, area) in [
            ("table", areas.table),
            ("checks", areas.checks),
            ("description", areas.description),
            ("diff", areas.diff),
        ] {
            assert!(!has_horizontal_scrollbar(&buffer, area), "{name}");
        }
    }

    #[test]
    fn the_horizontal_thumb_moves_from_the_left_end_to_the_right_end() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.diff_loaded(1, Ok(format!("+{}", "x".repeat(300))));
        let buffer = draw_narrow(&mut app);
        let diff = app.drawn.areas.diff;
        let at_start = thumb(&buffer, diff, Axis::Horizontal);
        assert_eq!(at_start.first(), Some(&(diff.x + 1)));
        assert!(at_start.last() < Some(&(diff.right() - 2)));

        focus(&mut app, Pane::Diff);
        for _ in 0..60 {
            key(&mut app, KeyCode::Right);
        }
        let buffer = draw_narrow(&mut app);
        let at_end = thumb(&buffer, diff, Axis::Horizontal);
        assert_eq!(at_end.last(), Some(&(diff.right() - 2)));
        assert!(at_end.first() > Some(&(diff.x + 1)));
    }

    /// An app whose first PR has this description and, if given, one comment with this body.
    fn app_with_bodies(body: &str, comment_body: Option<&str>) -> App {
        let comments: Vec<serde_json::Value> = comment_body
            .map(|body| {
                let mut comment = comment("someone", body);
                comment["url"] = "https://example.test/c".into();
                comment
            })
            .into_iter()
            .collect();
        let details = pr_details(body, comments.into());
        app_with_details(vec![pr(1, "Bump alpha", "OPEN", "")], details)
    }

    /// Clicks the first place `text` is drawn on screen, and returns the pages queued.
    fn click_text(app: &mut App, width: u16, text: &str) -> Vec<String> {
        let buffer = draw(app, width, 40);
        let position = find(&buffer, text).unwrap_or_else(|| panic!("{text} is not drawn"));
        press_at(app, position);
        app.pages_to_run()
    }

    #[test]
    fn a_click_on_a_link_in_the_description_opens_it_and_focuses_the_pane() {
        for text in ["the notes", "a.example/n"] {
            let mut app = app_with_bodies("Read [the notes](https://a.example/n) please.", None);
            assert_eq!(
                click_text(&mut app, 200, text),
                ["https://a.example/n"],
                "{text}"
            );
            assert_eq!(app.focus, Pane::Description);
        }
    }

    #[test]
    fn a_click_on_a_bare_url_opens_it() {
        let mut app = app_with_bodies("See https://a.example/bare, thanks.", None);
        assert_eq!(
            click_text(&mut app, 200, "a.example/bare"),
            ["https://a.example/bare"]
        );
    }

    #[test]
    fn a_click_on_text_that_is_not_a_link_does_nothing() {
        let mut app = app_with_bodies("Read [the notes](https://a.example/n) please.", None);
        assert!(click_text(&mut app, 200, "please").is_empty());
        assert!(app.errors.text().is_none());
    }

    #[test]
    fn a_link_is_found_in_a_scrolled_description() {
        let body = format!(
            "{}\n\n[far down](https://a.example/far)",
            vec!["line"; 80].join("\n\n")
        );
        let mut app = app_with_bodies(&body, None);
        draw_wide(&mut app);
        focus(&mut app, Pane::Description);
        key(&mut app, KeyCode::End);
        assert_eq!(
            click_text(&mut app, 200, "far down"),
            ["https://a.example/far"]
        );
    }

    #[test]
    fn a_url_that_wraps_over_several_rows_opens_from_any_of_them() {
        let url = "https://a.example/a/very/long/path/that/cannot/fit/on/one/row";
        let mut app = app_with_bodies(&format!("[{url}]({url})"), None);
        // The pane is under 60 columns wide.
        let buffer = draw_narrow(&mut app);
        let description = app.drawn.areas.description;
        assert!(find(&buffer, url).is_none(), "it is wrapped");
        // Every row inside the pane that has text is the link.
        let mut clicked = 0;
        for row in 1..description.height - 1 {
            let text = pane_row(&buffer, description, row - 1);
            if text.trim().is_empty() {
                continue;
            }
            press_at(
                &mut app,
                Position::new(description.x + 2, description.y + row),
            );
            assert_eq!(app.pages_to_run(), [url], "row {row}: {text}");
            clicked += 1;
        }
        assert!(clicked > 2);
    }

    #[test]
    fn a_link_in_a_table_still_opens_when_the_table_is_scrolled_sideways() {
        let body = "| Package | Change |\n|---|---|\n| [lib](https://a.example/l) | a long cell to make the table wider than the pane it is in |";
        let mut app = app_with_bodies(body, None);
        draw_narrow(&mut app);
        focus(&mut app, Pane::Description);
        assert_eq!(click_text(&mut app, 100, "lib"), ["https://a.example/l"]);
        key(&mut app, KeyCode::Right);
        draw_narrow(&mut app);
        assert!(app.scroll.description_x > 0);
        assert_eq!(
            click_text(&mut app, 100, "a.example/l"),
            ["https://a.example/l"]
        );
    }

    #[test]
    fn a_click_on_a_link_in_an_open_comment_opens_it_without_toggling_the_comment() {
        let mut app = app_with_bodies("desc", Some("Look at [this](https://a.example/c1) one."));
        draw_wide(&mut app);
        focus(&mut app, Pane::Comments);
        press(&mut app, " ");
        assert_eq!(click_text(&mut app, 200, "this"), ["https://a.example/c1"]);
        assert!(app.scroll.comments_open.contains(&0), "still open");
    }

    #[test]
    fn a_click_on_a_comments_header_still_toggles_it_and_a_collapsed_comment_has_no_links() {
        let mut app = app_with_bodies("desc", Some("Look at [this](https://a.example/c1) one."));
        let buffer = draw_wide(&mut app);
        assert!(find(&buffer, "this").is_none());
        let header = find(&buffer, "@someone").unwrap();
        press_at(&mut app, header);
        assert!(app.scroll.comments_open.contains(&0));
        assert!(app.pages_to_run().is_empty());
    }

    #[test]
    fn a_link_that_is_not_http_or_https_is_not_opened_and_says_why() {
        for url in [
            "file:///etc/passwd",
            "FILE:///etc/passwd",
            "vscode://open/x",
            "javascript:alert(1)",
            "../relative",
            "mailto:a@b.example",
        ] {
            let mut app = app_with_bodies(&format!("[local thing]({url}) here"), None);
            assert!(click_text(&mut app, 200, "local thing").is_empty(), "{url}");
            let error = app.errors.text().unwrap_or_default();
            assert!(error.contains("only http and https"), "{url}: {error}");
        }
        // Whatever the case of the scheme, a web link is fine.
        let mut app = app_with_bodies("[the notes](HTTPS://a.example/n)", None);
        assert_eq!(
            click_text(&mut app, 200, "the notes"),
            ["HTTPS://a.example/n"]
        );
    }

    #[test]
    fn clicks_do_nothing_with_links_when_the_details_have_not_loaded() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let buffer = draw_wide(&mut app);
        let description = app.drawn.areas.description;
        assert!(find(&buffer, "Loading").is_some());
        press_at(
            &mut app,
            Position::new(description.x + 2, description.y + 1),
        );
        assert!(app.pages_to_run().is_empty());
    }

    #[test]
    fn a_failed_details_fetch_shows_gh_s_first_line_in_the_comments_and_description() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.details_loaded(1, Err(gh_failure()));
        let screen = lines(&draw_wide(&mut app)).join("\n");
        // Once in each of the Comments, Approvers and Description panes.
        let message = "Failed to load details: GraphQL: Could not resolve to a PullRequest";
        assert_eq!(screen.matches(message).count(), 3, "{screen}");
        assert!(!screen.contains("gh pr view"), "{screen}");
        assert!(!screen.contains("more detail"), "{screen}");
    }

    #[test]
    fn a_failed_diff_shows_gh_s_first_line_and_wraps() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.diff_loaded(1, Err(gh_failure()));
        // Narrow, so the message needs more than one row of the diff pane.
        let screen = lines(&draw(&mut app, 80, 40));
        let diff = app.drawn.areas.diff;
        let pane: Vec<String> = screen[usize::from(diff.y)..usize::from(diff.y + diff.height)]
            .iter()
            .map(|line| line.chars().skip(usize::from(diff.x)).collect())
            .collect();
        let pane = pane.join("\n");
        assert!(pane.contains("Failed to load diff: GraphQL"), "{pane}");
        // The end of the first line is there, on a later row, and nothing after it.
        assert!(pane.contains("number of 1."), "{pane}");
        assert!(
            !pane.contains("more detail") && !pane.contains("gh pr view"),
            "{pane}"
        );
    }
}
