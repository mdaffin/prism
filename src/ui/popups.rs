use super::*;

/// The widest a popup gets, however wide the terminal is.
pub(super) const MAX_POPUP_WIDTH: u16 = 90;

/// The columns between two buttons.
pub(super) const BUTTON_GAP: u16 = 2;

/// The popup over the table that the mode asks for, if any. Records where its buttons are.
pub(super) fn draw_dialog(app: &mut App, frame: &mut Frame, area: Rect) {
    let mut scrolled = None;
    let buttons = match &app.mode {
        Mode::Confirm(confirm) => confirm_dialog(confirm, app.dry_run, frame, area),
        Mode::ConfirmOpen(confirm) => open_dialog(confirm, frame, area),
        Mode::Help(offset) => {
            let (buttons, view) = help_popup(*offset, frame, area);
            scrolled = Some(view);
            buttons
        }
        Mode::Failure(failure) => {
            let (buttons, view) = failure_popup(failure, frame, area);
            scrolled = Some(view);
            buttons
        }
        Mode::PickColumns(at) => columns_popup(app, *at, frame, area),
        Mode::Running(batch) => running_popup(batch, app.dry_run, spinner(app), frame, area),
        Mode::Results(summary) => results_popup(summary, app.dry_run, frame, area),
        Mode::Loading | Mode::List => Vec::new(),
    };
    app.drawn.buttons = buttons;
    // Scrolled too far (the terminal grew, or End was pressed), it is brought back.
    app.drawn.popup = scrolled.unwrap_or_default();
    if let (Some(offset), Some(view)) = (app.mode.scroll_offset_mut(), scrolled) {
        *offset = view.offset;
    }
}

/// The popup that chooses the table's columns: a checkbox for each, and the one at `at`
/// highlighted. The sorted column is named, as hiding it leaves the sort in place.
pub(super) fn columns_popup(app: &App, at: usize, frame: &mut Frame, area: Rect) -> Vec<Rect> {
    let lines = Column::ALL
        .into_iter()
        .enumerate()
        .map(|(i, column)| {
            // The author also has no column when every PR has the same one.
            let note =
                if column == Column::Author && !app.shows_author() && !app.hidden.contains(&column)
                {
                    " (every PR has the same one)"
                } else {
                    ""
                };
            let mark = if app.hidden.contains(&column) {
                "[ ]"
            } else {
                "[x]"
            };
            let line = Line::from(format!("{mark} {}{note}", column.name()));
            if i == at { line.reversed() } else { line }
        })
        .collect();
    let buttons = button_row(["OK"], 0);
    let footer = vec![Line::default(), buttons.line];
    let popup_ = Popup {
        title: "Columns".to_string(),
        dry_run: false,
        lines,
        wrap: false,
        footer,
        buttons: buttons.widths,
    };
    popup(frame, area, popup_)
}

pub(super) fn confirm_dialog(
    confirm: &Confirm,
    dry_run: bool,
    frame: &mut Frame,
    area: Rect,
) -> Vec<Rect> {
    let mut lines: Vec<Line> = confirm
        .prs
        .iter()
        .map(|pr| {
            Line::from(vec![
                format!("#{:<6} ", pr.number).into(),
                Span::styled(
                    format!("{:<12} ", pr.checks.label()),
                    state_style(pr.checks),
                ),
                pr.title.as_str().into(),
            ])
        })
        .collect();
    if confirm.prs.is_empty() {
        lines.push(Line::from("Nothing to review."));
    }
    let mut footer = Vec::new();
    if !confirm.left_out.is_empty() {
        footer.push(Line::default());
        footer.push(
            Line::from(format!(
                "Left out closed or merged: {}",
                pr_list(&confirm.left_out)
            ))
            .yellow(),
        );
    }
    footer.push(Line::default());
    let labels = confirm.choices().iter().map(|choice| choice.label());
    let buttons = button_row(labels, confirm.choice);
    footer.push(buttons.line);

    let title = format!("Review {}", count(confirm.prs.len(), "PR"));
    // One PR per line: long titles are cut off rather than wrapped.
    let dialog = Popup {
        title,
        dry_run,
        lines,
        wrap: false,
        footer,
        buttons: buttons.widths,
    };
    popup(frame, area, dialog)
}

pub(super) fn open_dialog(confirm: &ConfirmOpen, frame: &mut Frame, area: Rect) -> Vec<Rect> {
    let lines = vec![Line::from(pr_list(&confirm.numbers))];
    let highlighted = if confirm.open { 0 } else { 1 };
    let buttons = button_row(["Open", "Cancel"], highlighted);
    let footer = vec![Line::default(), buttons.line];
    let title = format!(
        "Open {} in the browser?",
        count(confirm.numbers.len(), "PR")
    );
    let dialog = Popup {
        title,
        dry_run: false,
        lines,
        wrap: true,
        footer,
        buttons: buttons.widths,
    };
    popup(frame, area, dialog)
}

pub(super) fn help_popup(offset: usize, frame: &mut Frame, area: Rect) -> (Vec<Rect>, PopupView) {
    let mut lines = Vec::new();
    for (pane, title) in keys::GROUPS {
        let mut group = keys::group(pane).peekable();
        if group.peek().is_none() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::from(title).bold());
        lines.extend(group.map(|help| Line::from(format!("{:<16} {}", help.keys, help.action))));
    }
    let buttons = button_row(["OK"], 0);
    let footer = vec![Line::default(), buttons.line];
    let help = Popup {
        title: "Keys".to_string(),
        dry_run: false,
        lines,
        wrap: false,
        footer,
        buttons: buttons.widths,
    };
    popup_scrolled(frame, area, help, offset)
}

/// The popup with a failed fetch's whole message, wrapped here (so that the rows it takes are
/// known, for scrolling) and scrolled like the keys popup.
pub(super) fn failure_popup(
    failure: &Failure,
    frame: &mut Frame,
    area: Rect,
) -> (Vec<Rect>, PopupView) {
    let width = usize::from(
        area.width
            .saturating_sub(4)
            .min(MAX_POPUP_WIDTH)
            .saturating_sub(2),
    );
    let text = Text::from(
        failure
            .text
            .lines()
            .map(|line| Line::from(line.trim_end().to_string()))
            .collect::<Vec<_>>(),
    );
    let lines = model::layout_plain(&text, width.max(1)).lines;
    let buttons = button_row(["OK"], 0);
    let footer = vec![Line::default(), buttons.line];
    let dialog = Popup {
        title: failure.title.clone(),
        dry_run: false,
        lines,
        wrap: false,
        footer,
        buttons: buttons.widths,
    };
    popup_scrolled(frame, area, dialog, failure.offset)
}

pub(super) fn running_popup(
    batch: &Batch,
    dry_run: bool,
    spinner: &str,
    frame: &mut Frame,
    area: Rect,
) -> Vec<Rect> {
    let (finished, total) = batch.progress();
    let progress = format!("{spinner} {finished}/{total} done");
    let step = if batch.merge {
        "Approving and merging"
    } else {
        "Approving"
    };
    let title = format!("{step} {}", count(total, "PR"));
    let mut lines = vec![Line::from(progress), Line::default()];
    // Failures can be long, so the lines wrap.
    lines.extend(batch.items.iter().map(|item| {
        let status = item.current_status();
        let label = Span::styled(item.label(), item_style(status));
        Line::from(vec![Span::raw(format!("#{:<7}", item.pr.number)), label])
    }));
    let footer = vec![Line::default(), Line::from("q: Cancel").dim().centered()];
    let running = Popup {
        title,
        dry_run,
        lines,
        wrap: true,
        footer,
        buttons: Vec::new(),
    };
    popup(frame, area, running)
}

pub(super) fn item_style(status: &ItemStatus) -> Style {
    match status {
        ItemStatus::Pending | ItemStatus::Cancelled => Style::new().dim(),
        ItemStatus::Working => Style::new().cyan(),
        ItemStatus::Done => Style::new().green(),
        ItemStatus::AutoMerge | ItemStatus::Interrupted => Style::new().yellow(),
        ItemStatus::Failed(_) => Style::new().red(),
    }
}

pub(super) fn results_popup(
    summary: &Summary,
    dry_run: bool,
    frame: &mut Frame,
    area: Rect,
) -> Vec<Rect> {
    let mut lines = vec![Line::from(format!(
        "Approved: {}/{}",
        summary.approved, summary.total
    ))];
    if let Some((merged, tried)) = summary.merged {
        lines.push(Line::from(format!("Merged: {merged}/{tried}")));
    }
    if !summary.failures.is_empty() {
        lines.push(Line::default());
        for (number, step, error) in &summary.failures {
            lines.push(Line::from(format!("#{number}  ✗ {step} failed: {error}")).red());
        }
    }
    if summary.cancelled > 0 {
        lines.push(Line::default());
        lines.push(
            Line::from(format!(
                "Cancelled: {} not finished",
                count(summary.cancelled, "PR")
            ))
            .dim(),
        );
    }
    if !summary.interrupted.is_empty() {
        lines.push(Line::default());
        lines.push(
            Line::from(format!(
                "Interrupted while running - check GitHub: {}",
                pr_list(&summary.interrupted)
            ))
            .yellow(),
        );
    }
    if !summary.auto_merge.is_empty() {
        lines.push(Line::default());
        lines.push(
            Line::from(format!(
                "Auto-merge queued (checks still pending): {}",
                pr_list(&summary.auto_merge)
            ))
            .yellow(),
        );
    }
    let buttons = button_row(["OK"], 0);
    let footer = vec![Line::default(), buttons.line];
    // Errors can be long, so they wrap.
    let results = Popup {
        title: "Results".to_string(),
        dry_run,
        lines,
        wrap: true,
        footer,
        buttons: buttons.widths,
    };
    popup(frame, area, results)
}

/// What a popup shows.
pub(super) struct Popup<'a> {
    title: String,
    /// Whether to add ` [DRY RUN]` to the title.
    dry_run: bool,
    lines: Vec<Line<'a>>,
    /// Whether long lines wrap rather than being cut off.
    wrap: bool,
    /// Lines pinned to the bottom, such as buttons.
    footer: Vec<Line<'a>>,
    /// The widths of the buttons on the last footer line, which `button_row` drew centred.
    buttons: Vec<u16>,
}

/// Draws a bordered popup in the middle of `area`. The footer lines are pinned to the bottom so
/// they stay visible even when `lines` doesn't fit. Wrapped lines are sized by estimate, so with
/// `wrap` the popup may cut off the end of `lines`. Returns where its buttons were drawn.
pub(super) fn popup(frame: &mut Frame, area: Rect, popup: Popup) -> Vec<Rect> {
    popup_scrolled(frame, area, popup, 0).0
}

/// Like [`popup`], with its lines scrolled `offset` rows (clamped), and a scrollbar down its right
/// border while they don't all fit. Also returns how many rows there were and how many fit, for
/// the keys that scroll it.
pub(super) fn popup_scrolled(
    frame: &mut Frame,
    area: Rect,
    popup: Popup,
    offset: usize,
) -> (Vec<Rect>, PopupView) {
    let Popup {
        mut title,
        dry_run,
        lines,
        wrap,
        footer,
        buttons,
    } = popup;
    if dry_run {
        title.push_str(" [DRY RUN]");
    }
    let full = area;
    let width = area.width.saturating_sub(4).min(MAX_POPUP_WIDTH);
    let inner_width = usize::from(width.saturating_sub(2).max(1));
    let rows: usize = if wrap {
        lines
            .iter()
            .map(|line| line.width().div_ceil(inner_width).max(1))
            .sum()
    } else {
        lines.len()
    };
    let footer_rows = footer.len() as u16;
    let height = (rows as u16 + footer_rows + 2).min(area.height);
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let block = Block::bordered()
        .border_style(Style::new().cyan())
        .title(Line::from(title).bold());
    let [content_area, footer_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(footer_rows)])
            .areas(block.inner(area));
    let mut content = Paragraph::new(lines);
    if wrap {
        content = content.wrap(Wrap { trim: false });
    }
    let viewport = usize::from(content_area.height);
    let offset = offset.min(rows.saturating_sub(viewport));
    frame.buffer_mut().set_style(full, Modifier::DIM);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(content.scroll((to_u16(offset), 0)), content_area);
    frame.render_widget(Paragraph::new(footer), footer_area);
    // The bar runs beside the lines only, not the footer.
    let bar_area = Rect {
        height: content_area.height + 2,
        ..area
    };
    let _ = scrollbar(frame, bar_area, rows, viewport, offset);
    let view = PopupView {
        content: rows,
        viewport,
        offset,
    };
    (button_areas(&buttons, footer_area), view)
}

/// Where the centred buttons of a row of the given widths are, on the last row of `footer`.
pub(super) fn button_areas(widths: &[u16], footer: Rect) -> Vec<Rect> {
    if widths.is_empty() || footer.is_empty() {
        return Vec::new();
    }
    let total = widths.iter().sum::<u16>() + BUTTON_GAP * (widths.len() as u16 - 1);
    let mut x = footer.x + footer.width.saturating_sub(total) / 2;
    let y = footer.bottom() - 1;
    widths
        .iter()
        .map(|&width| {
            let area = Rect::new(x, y, width, 1);
            x += width + BUTTON_GAP;
            area
        })
        .collect()
}

/// `#1, #2, #3`.
pub(super) fn pr_list(numbers: &[u64]) -> String {
    let numbers: Vec<String> = numbers.iter().map(|n| format!("#{n}")).collect();
    numbers.join(", ")
}

/// A row of buttons, centred, with the one at `highlighted` reversed.
pub(super) fn button_row<'a>(
    labels: impl IntoIterator<Item = &'a str>,
    highlighted: usize,
) -> ButtonRow {
    let mut line = Line::default();
    let mut widths = Vec::new();
    for (i, label) in labels.into_iter().enumerate() {
        if i > 0 {
            line.push_span(" ".repeat(usize::from(BUTTON_GAP)));
        }
        let label = format!("[ {label} ]");
        widths.push(label.chars().count() as u16);
        line.push_span(if i == highlighted {
            label.reversed()
        } else {
            label.into()
        });
    }
    ButtonRow {
        line: line.centered(),
        widths,
    }
}

/// A row of buttons and the width of each, for working out where they were drawn.
pub(super) struct ButtonRow {
    line: Line<'static>,
    widths: Vec<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::*;

    #[test]
    fn colours_the_checks_in_the_confirm_dialog() {
        let mut app = one_of_each();
        // Select everything, then open the dialog.
        select_rows(&mut app, &[0, 1, 2, 3, 4]);
        review(&mut app);
        let buffer = draw_wide(&mut app);
        for (label, colour) in [
            ("✓ passing", Color::Green),
            ("… pending", Color::Yellow),
            ("✗ failing", Color::Red),
            ("✗ conflict", Color::Red),
        ] {
            // The table's checks column is in the left 30 columns; the centred dialog is to the
            // right of that.
            let position = (0..buffer.area.height)
                .find_map(|y| {
                    let line: String = lines(&buffer)[usize::from(y)].chars().skip(30).collect();
                    let x = line.find(label).map(|byte| line[..byte].chars().count())?;
                    Some(Position::new(30 + x as u16, y))
                })
                .unwrap_or_else(|| panic!("no dialog row for {label}"));
            assert_eq!(buffer[position].fg, colour, "{label}");
        }
    }

    #[test]
    fn shows_each_pr_of_a_running_batch_and_what_cancelling_did() {
        let mut app = app_with((1..=6).map(|n| pr(n, "Bump alpha", "OPEN", "")).collect());
        select_rows(&mut app, &[0, 1, 2, 3, 4, 5]);
        review(&mut app);
        key(&mut app, KeyCode::Enter);
        // Four approvals run at once; two wait.
        assert_eq!(app.approvals_to_run().len(), 4);
        let epoch = app.batch_epoch();
        app.update(Msg::Approved(epoch, 1, Ok(())));
        let screen = lines(&draw(&mut app, 200, 30));
        // The popup draws over the table, so a status shares its line with a table row.
        let with = |text: &str| screen.iter().filter(|line| line.contains(text)).count();
        assert_eq!(with("✓ done"), 1);
        assert_eq!(with("… approving"), 3);
        assert_eq!(with("· waiting"), 2);
        assert!(shows(&screen, "q: Cancel"));

        press(&mut app, "q");
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(shows(&screen, "Cancelled: 2 PRs"));
        assert!(shows(&screen, "check GitHub: #2, #3, #4"));
    }

    #[test]
    fn dims_everything_behind_a_popup() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let plain = draw(&mut app, 200, 30);
        let table = find(&plain, "is:pr").unwrap();
        assert!(!plain[table].modifier.contains(Modifier::DIM));

        press(&mut app, "?");
        let buffer = draw(&mut app, 200, 30);
        assert!(buffer[table].modifier.contains(Modifier::DIM));
        let popup = find(&buffer, "Keys").unwrap();
        let inside = Position::new(popup.x + 2, popup.y + 1);
        assert!(!buffer[popup].modifier.contains(Modifier::DIM));
        assert!(!buffer[inside].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn a_dialog_swallows_the_mouse() {
        let mut app = app_with_long_panes();
        let diff = Position::new(app.drawn.areas.diff.x + 2, app.drawn.areas.diff.y + 1);
        press(&mut app, "?");
        draw_wide(&mut app);
        // A click behind it does not reach the panes.
        press_at(&mut app, diff);
        assert_eq!(app.focus, Pane::Table);
        // Not even a box''s area is live behind a dialog.
        press_at(&mut app, Position::new(2, 2));
        assert!(app.typing == Typing::Nothing);
        // The wheel does not scroll the pane behind it, nor does it do anything over a button.
        wheel(&mut app, diff, true);
        assert_eq!(app.scroll.diff, 0);
        let position = button(&app, 0);
        wheel(&mut app, position, true);
        assert_eq!(app.scroll.diff, 0);
        assert!(matches!(app.mode, Mode::Help(_)));
    }

    /// The centre of the dialog's `index`th button, as last drawn.
    fn button(app: &App, index: usize) -> Position {
        let area = app.drawn.buttons[index];
        Position::new(area.x + area.width / 2, area.y)
    }

    fn click_button(app: &mut App, index: usize) {
        let position = button(app, index);
        press_at(app, position);
    }

    /// An app with the confirm dialog open for two PRs, drawn.
    fn app_confirming() -> App {
        let mut app = app_with(vec![
            pr(1, "Bump alpha", "OPEN", ""),
            pr(2, "Bump beta", "OPEN", ""),
        ]);
        press(&mut app, " j ");
        review(&mut app);
        assert!(matches!(app.mode, Mode::Confirm(_)));
        draw_wide(&mut app);
        app
    }

    /// An app with the dialog that asks before opening six PRs open, drawn.
    fn app_confirming_open() -> App {
        let mut app = app_with((1..=6).map(|n| pr(n, "Bump", "OPEN", "")).collect());
        for _ in 0..6 {
            press(&mut app, " j");
        }
        ctrl(&mut app, 'o');
        assert!(matches!(app.mode, Mode::ConfirmOpen(_)));
        draw_wide(&mut app);
        app
    }

    #[test]
    fn the_buttons_are_recorded_where_they_are_drawn() {
        let mut app = app_confirming();
        let buffer = draw_wide(&mut app);
        let drawn: Vec<String> = app
            .drawn
            .buttons
            .iter()
            .map(|area| text_at(&buffer, *area))
            .collect();
        assert_eq!(drawn, ["[ Approve ]", "[ Approve & merge ]", "[ Cancel ]"]);

        // Once the dialog closes, none are recorded.
        press(&mut app, "q");
        draw_wide(&mut app);
        assert!(app.drawn.buttons.is_empty());
    }

    #[test]
    fn clicking_approve_or_approve_and_merge_starts_the_batch() {
        for (index, merge) in [(0, false), (1, true)] {
            let mut app = app_confirming();
            click_button(&mut app, index);
            let Mode::Running(batch) = &app.mode else {
                panic!("button {index} did not start the batch");
            };
            assert_eq!(batch.merge, merge);
            assert_eq!(app.approvals_to_run(), [1, 2]);
        }
    }

    #[test]
    fn clicking_cancel_closes_the_confirm_dialog() {
        let mut app = app_confirming();
        click_button(&mut app, 2);
        assert_list_mode(&app);
        assert!(app.approvals_to_run().is_empty());
    }

    #[test]
    fn clicking_open_or_cancel_answers_the_open_dialog() {
        let mut app = app_confirming_open();
        click_button(&mut app, 0);
        assert_list_mode(&app);
        assert_eq!(app.opens_to_run(), [1, 2, 3, 4, 5, 6]);

        let mut app = app_confirming_open();
        click_button(&mut app, 1);
        assert_list_mode(&app);
        assert!(app.opens_to_run().is_empty());
    }

    #[test]
    fn clicking_ok_closes_the_help_and_results_popups() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "?");
        draw_wide(&mut app);
        assert_eq!(app.drawn.buttons.len(), 1);
        click_button(&mut app, 0);
        assert_list_mode(&app);

        let mut app = app_confirming();
        click_button(&mut app, 0);
        // `q` cancels the running batch, which shows the results.
        press(&mut app, "q");
        assert!(matches!(app.mode, Mode::Results(_)));
        draw_wide(&mut app);
        click_button(&mut app, 0);
        assert_list_mode(&app);
    }

    #[test]
    fn a_click_on_the_dialog_but_not_a_button_or_outside_it_does_nothing() {
        let mut app = app_confirming();
        let first = app.drawn.buttons[0];
        // The row above the buttons is inside the dialog; (0, 0) is outside it, on the title.
        for position in [Position::new(first.x, first.y - 1), Position::new(0, 0)] {
            press_at(&mut app, position);
            let Mode::Confirm(confirm) = &app.mode else {
                panic!("the dialog closed");
            };
            assert_eq!(confirm.choice, 0);
        }
        // Between two buttons.
        let gap = app.drawn.buttons[0].right();
        press_at(&mut app, Position::new(gap, first.y));
        assert!(matches!(app.mode, Mode::Confirm(_)));
        // Nor does a click behind it reach the panes.
        let diff = Position::new(app.drawn.areas.diff.x + 2, app.drawn.areas.diff.y + 1);
        press_at(&mut app, diff);
        assert_eq!(app.focus, Pane::Table);
    }

    #[test]
    fn a_click_does_nothing_while_a_batch_runs() {
        let mut app = app_confirming();
        click_button(&mut app, 0);
        draw_wide(&mut app);
        assert!(app.drawn.buttons.is_empty());
        for position in [Position::new(80, 20), Position::new(0, 0)] {
            press_at(&mut app, position);
            assert!(matches!(app.mode, Mode::Running(_)));
        }
    }

    /// An app with the help popup open and drawn on a screen too short for all of it.
    fn short_help() -> App {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "?");
        draw(&mut app, 200, 14);
        app
    }

    #[test]
    fn a_short_screen_cuts_the_help_popup_and_draws_a_scrollbar_with_the_button_pinned() {
        let mut app = short_help();
        let screen = lines(&draw(&mut app, 200, 14)).join("\n");
        // Too short for every key: the start is there, the end is not, and the bar says so.
        assert!(screen.contains("Everywhere"), "{screen}");
        assert!(
            !screen.contains("Scroll the description's tables sideways"),
            "{screen}"
        );
        assert!(screen.contains("OK"), "{screen}");
        assert!(app.drawn.popup.content > app.drawn.popup.viewport);
        let bar = |app: &mut App| -> usize {
            let buffer = draw(app, 200, 14);
            let area = buffer.area;
            (0..area.height)
                .flat_map(|y| (0..area.width).map(move |x| (x, y)))
                .filter(|&pos| buffer[pos].symbol() == "█")
                .count()
        };
        assert!(bar(&mut app) > 0, "no scrollbar");
    }

    #[test]
    fn end_shows_the_last_help_key_and_the_button_is_still_there() {
        let mut app = short_help();
        key(&mut app, KeyCode::End);
        let screen = lines(&draw(&mut app, 200, 14)).join("\n");
        assert!(
            screen.contains("Scroll the description's tables sideways"),
            "{screen}"
        );
        assert!(!screen.contains("Everywhere"), "{screen}");
        assert!(screen.contains("OK"), "{screen}");
    }

    #[test]
    fn down_at_the_end_of_the_help_popup_does_nothing_further() {
        let mut app = short_help();
        key(&mut app, KeyCode::End);
        draw(&mut app, 200, 14);
        key(&mut app, KeyCode::Down);
        assert!(matches!(app.mode, Mode::Help(at) if at == app.drawn.popup.offset));
    }

    #[test]
    fn home_page_down_up_and_page_up_move_the_help_popup() {
        let mut app = short_help();
        key(&mut app, KeyCode::End);
        draw(&mut app, 200, 14);
        key(&mut app, KeyCode::Home);
        draw(&mut app, 200, 14);
        assert_eq!(app.drawn.popup.offset, 0);
        key(&mut app, KeyCode::PageDown);
        draw(&mut app, 200, 14);
        let page = app.drawn.popup.offset;
        assert_eq!(page, app.drawn.popup.viewport - 1);
        key(&mut app, KeyCode::Up);
        draw(&mut app, 200, 14);
        assert_eq!(app.drawn.popup.offset, page - 1);
        key(&mut app, KeyCode::PageUp);
        draw(&mut app, 200, 14);
        assert_eq!(app.drawn.popup.offset, 0);
    }

    #[test]
    fn the_wheel_scrolls_the_help_popup_three_lines_a_step() {
        let mut app = short_help();
        wheel(&mut app, Position::new(100, 5), true);
        draw(&mut app, 200, 14);
        assert_eq!(app.drawn.popup.offset, 3);
        wheel(&mut app, Position::new(100, 5), false);
        draw(&mut app, 200, 14);
        assert_eq!(app.drawn.popup.offset, 0);
    }

    #[test]
    fn the_help_popup_still_closes_on_a_short_screen() {
        let mut app = short_help();
        key(&mut app, KeyCode::Esc);
        assert_list_mode(&app);
    }

    #[test]
    fn a_tall_help_popup_has_no_scrollbar() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "?");
        let buffer = draw(&mut app, 200, 120);
        assert_eq!(app.drawn.popup.content, app.drawn.popup.viewport);
        // And none is drawn.
        let area = buffer.area;
        let thumbs = (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .filter(|&pos| buffer[pos].symbol() == "█")
            .count();
        assert_eq!(thumbs, 0);
        key(&mut app, KeyCode::End);
        draw(&mut app, 200, 120);
        assert_eq!(app.drawn.popup.offset, 0);
    }

    #[test]
    fn the_help_popup_groups_the_keys_by_pane() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "?");
        let screen = lines(&draw(&mut app, 200, 120));
        // The panes behind the popup have titles too, so look from the popup's own.
        let top = screen
            .iter()
            .position(|line| line.contains("Keys"))
            .unwrap();
        let row = |text: &str| {
            screen
                .iter()
                .skip(top)
                .position(|line| line.contains(text))
                .unwrap_or_else(|| panic!("no {text:?}"))
        };
        let groups = [
            "Everywhere",
            "Pull requests",
            "Checks",
            "Comments",
            "Description",
        ];
        let rows: Vec<usize> = groups.iter().map(|title| row(title)).collect();
        assert!(rows.windows(2).all(|pair| pair[0] < pair[1]), "{rows:?}");
        // Each group lists its own keys under its title.
        assert!(row("Review the selected PRs") > row("Everywhere"));
        assert!(row("Open the highlighted PR in the browser") > row("Pull requests"));
        assert!(row("Open the highlighted check's page") > row("Checks"));
        assert!(row("Expand or collapse the highlighted comment") > row("Comments"));
        assert!(row("Scroll the description's tables sideways") > row("Description"));
    }

    /// A failure of many lines, one of them too long for the popup, with a last line to scroll to.
    fn long_failure() -> color_eyre::Report {
        let lines: Vec<String> = (1..=30).map(|n| format!("detail line {n}")).collect();
        crate::gh::GhError {
            command: "gh api graphql".to_string(),
            output: format!(
                "GraphQL: first line.\n{}\n{}\nthe very last line",
                "a-very-long-line-without-spaces-".repeat(6),
                lines.join("\n")
            ),
        }
        .into()
    }

    /// An app whose details (or, with `diff`, whose diff) failed to load with `long_failure`, drawn
    /// on a 120 by 20 screen.
    fn app_with_a_long_failure(diff: bool) -> App {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        if diff {
            app.diff_loaded(1, Err(long_failure()));
            app.focus = Pane::Diff;
        } else {
            app.details_loaded(1, Err(long_failure()));
        }
        draw(&mut app, 120, 20);
        app
    }

    #[test]
    fn e_opens_the_whole_failure_wrapped_to_the_popup() {
        for diff in [false, true] {
            let mut app = app_with_a_long_failure(diff);
            press(&mut app, "e");
            assert!(matches!(app.mode, Mode::Failure(_)), "diff: {diff}");
            let screen = lines(&draw(&mut app, 120, 20)).join("\n");
            assert!(screen.contains("failed to load"), "{screen}");
            assert!(screen.contains("first line."), "{screen}");
            // The long line is wrapped to the popup rather than cut off, so the message is taller
            // than the popup and the end of it needs scrolling to.
            assert!(!screen.contains("the very last line"), "{screen}");
            assert!(app.drawn.popup.content > app.drawn.popup.viewport);
            assert!(screen.contains("OK"), "{screen}");
        }
    }

    #[test]
    fn end_scrolls_the_failure_to_its_last_line_with_the_button_still_there() {
        for diff in [false, true] {
            let mut app = app_with_a_long_failure(diff);
            press(&mut app, "e");
            // The popup learns its size when drawn, and End scrolls by it.
            draw(&mut app, 120, 20);
            key(&mut app, KeyCode::End);
            let screen = lines(&draw(&mut app, 120, 20)).join("\n");
            assert!(screen.contains("the very last line"), "{screen}");
            assert!(!screen.contains("first line."), "{screen}");
            assert!(screen.contains("OK"), "{screen}");
        }
    }

    #[test]
    fn e_and_esc_each_close_the_failure() {
        for diff in [false, true] {
            let mut app = app_with_a_long_failure(diff);
            press(&mut app, "e");
            press(&mut app, "e");
            assert_list_mode(&app);
            press(&mut app, "e");
            key(&mut app, KeyCode::Esc);
            assert_list_mode(&app);
        }
    }
    #[test]
    fn e_with_nothing_failed_says_so_and_opens_nothing() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "e");
        assert_list_mode(&app);
        assert!(status_bar(&mut app, 160, 30).contains("Nothing has failed"));
    }
}
