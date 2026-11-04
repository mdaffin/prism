//! PR logic with no I/O.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use regex::Regex;
use unicode_width::UnicodeWidthChar;

use crate::gh::{Check, Comment, Conclusion, ContextState, Mergeable, ReviewDecision, RunStatus};

/// The state of a single check, or of all a PR's checks together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Passing,
    Pending,
    Failing,
    /// The PR has no checks.
    None,
    /// The PR has merge conflicts, whatever its checks say.
    Conflict,
}

impl CheckState {
    /// Lower is better: passing 0, failing and conflict 2, everything else 1.
    pub fn rank(self) -> u8 {
        match self {
            Self::Passing => 0,
            Self::Pending | Self::None => 1,
            Self::Failing | Self::Conflict => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Passing => "✓ passing",
            Self::Pending => "… pending",
            Self::Failing => "✗ failing",
            Self::None => "· no checks",
            Self::Conflict => "✗ conflict",
        }
    }
}

/// The latest run of one check.
#[derive(Debug, PartialEq, Eq)]
pub struct CheckResult {
    /// `workflowName/name` for a check run, `context` for a status context.
    pub key: String,
    pub state: CheckState,
    /// The page to open for it, if `gh` gave one.
    pub url: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Checks {
    pub state: CheckState,
    /// The latest run per key, sorted by key.
    pub results: Vec<CheckResult>,
}

impl Checks {
    /// The results with the worst first, then by key. This is the order of the checks pane; the
    /// PR list sorts the other way round.
    pub fn worst_first(&self) -> Vec<&CheckResult> {
        let mut results: Vec<&CheckResult> = self.results.iter().collect();
        // The sort is stable, so equal ranks stay in key order.
        results.sort_by_key(|result| std::cmp::Reverse(result.state.rank()));
        results
    }
}

pub fn check_state(check: &Check) -> CheckState {
    match check {
        Check::CheckRun(run) if run.status != RunStatus::Completed => CheckState::Pending,
        Check::CheckRun(run) => match run.conclusion {
            Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped => CheckState::Passing,
            Conclusion::Other => CheckState::Failing,
        },
        Check::StatusContext(ctx) => match ctx.state {
            ContextState::Success => CheckState::Passing,
            ContextState::Pending | ContextState::Expected => CheckState::Pending,
            ContextState::Other => CheckState::Failing,
        },
        Check::Unknown => CheckState::Pending,
    }
}

/// Keeps the latest run of each check and works out the overall state. A conflict outranks the
/// check results.
pub fn classify_checks(mergeable: Mergeable, rollup: &[Check]) -> Checks {
    let mut latest: BTreeMap<String, &Check> = BTreeMap::new();
    for check in rollup {
        let (key, started_at) = match check {
            Check::CheckRun(run) => (
                format!("{}/{}", run.workflow_name, run.name),
                &run.started_at,
            ),
            Check::StatusContext(ctx) => (ctx.context.clone(), &ctx.started_at),
            Check::Unknown => ("(unrecognised check)".to_string(), &None),
        };
        // A missing start time sorts as earliest; on a tie the later entry wins.
        let newer = latest
            .get(&key)
            .is_none_or(|seen| started_at >= started_at_of(seen));
        if newer {
            latest.insert(key, check);
        }
    }
    let results: Vec<CheckResult> = latest
        .into_iter()
        .map(|(key, check)| CheckResult {
            key,
            state: check_state(check),
            url: check_url(check),
        })
        .collect();

    let any = |state| results.iter().any(|r| r.state == state);
    let state = if mergeable == Mergeable::Conflicting {
        CheckState::Conflict
    } else if results.is_empty() {
        CheckState::None
    } else if any(CheckState::Failing) {
        CheckState::Failing
    } else if any(CheckState::Pending) {
        CheckState::Pending
    } else {
        CheckState::Passing
    };
    Checks { state, results }
}

/// The page of a check, or `None` if it has none or `gh` left it empty.
fn check_url(check: &Check) -> Option<String> {
    let url = match check {
        Check::CheckRun(run) => &run.details_url,
        Check::StatusContext(ctx) => &ctx.target_url,
        Check::Unknown => &None,
    };
    url.clone().filter(|url| !url.is_empty())
}

fn started_at_of(check: &Check) -> &Option<String> {
    match check {
        Check::CheckRun(run) => &run.started_at,
        Check::StatusContext(ctx) => &ctx.started_at,
        Check::Unknown => &None,
    }
}

/// How long before `now` a timestamp such as `2026-01-02T03:04:05Z` was, like `3 hours ago`.
/// A timestamp that can't be read is shown as it is.
pub fn ago(timestamp: &str, now: jiff::Timestamp) -> String {
    let Ok(then) = timestamp.parse::<jiff::Timestamp>() else {
        return timestamp.to_string();
    };
    let seconds = now.as_second() - then.as_second();
    let units = [
        (365 * 24 * 3600, "year"),
        (30 * 24 * 3600, "month"),
        (7 * 24 * 3600, "week"),
        (24 * 3600, "day"),
        (3600, "hour"),
        (60, "minute"),
    ];
    for (size, unit) in units {
        if seconds >= size {
            let n = seconds / size;
            return format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
        }
    }
    "just now".to_string()
}

/// How long before `now` a timestamp was, in the fewest characters: `5m`, `2h`, `3d`, `2w`, `4mo`,
/// `1y`. Under a minute is `now`. A timestamp that can't be read is shown as it is.
pub fn short_age(timestamp: &str, now: jiff::Timestamp) -> String {
    let Ok(then) = timestamp.parse::<jiff::Timestamp>() else {
        return timestamp.to_string();
    };
    let seconds = now.as_second() - then.as_second();
    let units = [
        (365 * 24 * 3600, "y"),
        (30 * 24 * 3600, "mo"),
        (7 * 24 * 3600, "w"),
        (24 * 3600, "d"),
        (3600, "h"),
        (60, "m"),
    ];
    for (size, unit) in units {
        if seconds >= size {
            return format!("{}{unit}", seconds / size);
        }
    }
    "now".to_string()
}

/// A column of the table that can be hidden: every one but the checkbox and the title columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Number,
    Checks,
    Review,
    Author,
    Updated,
}

impl Column {
    /// Every column, in the order the table shows them.
    pub const ALL: [Column; 5] = [
        Self::Number,
        Self::Checks,
        Self::Review,
        Self::Author,
        Self::Updated,
    ];

    /// The header, and the name in the columns popup.
    pub fn name(self) -> &'static str {
        match self {
            Self::Number => "#",
            Self::Checks => "Checks",
            Self::Review => "Review",
            Self::Author => "Author",
            Self::Updated => "Updated",
        }
    }

    /// What clicking the header sorts by.
    pub fn sort(self) -> Sort {
        match self {
            Self::Number => Sort::Number,
            Self::Checks => Sort::Checks,
            Self::Review => Sort::Review,
            Self::Author => Sort::Author,
            Self::Updated => Sort::Updated,
        }
    }
}

/// How the list is ordered. Whatever the sort, closed and merged PRs come last, as they can't be
/// acted on, and PRs that tie stay in the order GitHub listed them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    /// Passing first, then pending or without checks, then failing or in conflict.
    #[default]
    Checks,
    /// Lowest number, the oldest PR, first.
    Number,
    /// A to Z, ignoring case.
    Title,
    /// Those still needing a review, then those with changes requested, then approved ones.
    Review,
    /// A to Z by login, ignoring case.
    Author,
    /// The longest since anything happened to it first.
    Updated,
}

impl Sort {
    /// The next sort in the cycle, which comes back to the first.
    pub fn next(self) -> Self {
        match self {
            Self::Checks => Self::Number,
            Self::Number => Self::Title,
            Self::Title => Self::Review,
            Self::Review => Self::Author,
            Self::Author => Self::Updated,
            Self::Updated => Self::Checks,
        }
    }
}

pub fn review_label(review_decision: ReviewDecision) -> &'static str {
    match review_decision {
        ReviewDecision::Approved => "✓ approved",
        ReviewDecision::ChangesRequested => "✗ changes",
        ReviewDecision::Other => "· needed",
    }
}

static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// Joins the lines of `text` with spaces, so it fits in one row.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The line of a regex error that says what is wrong: the one after `error:` if there is one, as
/// the first line of a syntax error only says that there is one.
pub fn regex_error_line(error: &str) -> &str {
    error
        .lines()
        .find_map(|line| line.trim().strip_prefix("error: "))
        .unwrap_or_else(|| first_line(error))
}

/// The first line of `text` that isn't blank, such as the headline of an error message.
pub fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// Removes HTML comments and runs of blank lines from a PR or comment body.
pub fn clean_body(body: Option<&str>) -> String {
    let body = body.unwrap_or_default().replace("\r\n", "\n");
    let body = HTML_COMMENT.replace_all(&body, "");
    let body = BLANK_LINES.replace_all(&body, "\n\n");
    body.trim().to_string()
}

/// A body rendered as markdown, and where the links in it are.
#[derive(Debug, Clone, Default)]
pub struct Markdown {
    pub text: Text<'static>,
    /// The links of each line of `text`, left to right, in columns from the start of the line.
    pub links: Vec<Vec<LinkSpan>>,
}

/// Where a link is drawn in a line, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpan {
    /// The first column of the link and the one after its last.
    pub start: usize,
    pub end: usize,
    pub url: String,
}

/// Renders a cleaned body as markdown. The text is copied so it can be kept in the details cache.
pub fn render_markdown(body: &str) -> Markdown {
    let text = tui_markdown::from_str(body);
    let mut lines = Vec::new();
    let mut links = Vec::new();
    for line in text.lines {
        let spans: Vec<Span<'static>> = line
            .spans
            .into_iter()
            .map(|span| Span::styled(span.content.into_owned(), span.style))
            .collect();
        let (line, line_links) = find_links(Line { spans, ..line });
        lines.push(line);
        links.push(line_links);
    }
    Markdown {
        text: Text::from(lines),
        links,
    }
}

/// A comment's body rendered as markdown. A thread's replies follow it, each under a line saying
/// who wrote it and when. Each body is rendered on its own, so an unclosed code fence in one can't
/// swallow the ones after it.
pub fn render_comment(comment: &Comment) -> Markdown {
    let mut rendered = render_markdown(&clean_body(comment.body.as_deref()));
    for reply in &comment.replies {
        let author = reply.author.as_ref().map_or("ghost", |a| a.login.as_str());
        // `2026-01-02T03:04:05Z` as `2026-01-02 03:04`.
        let when = reply.created_at.get(..16).unwrap_or(&reply.created_at);
        let header = format!("@{author} · {}", when.replace('T', " "));
        let body = render_markdown(&clean_body(reply.body.as_deref()));
        rendered.text.lines.push(Line::default());
        rendered.links.push(Vec::new());
        rendered.text.lines.push(Line::styled(
            header,
            Style::new().add_modifier(Modifier::BOLD),
        ));
        rendered.links.push(Vec::new());
        rendered.text.lines.extend(body.text.lines);
        rendered.links.extend(body.links);
    }
    rendered
}

/// How `tui-markdown` styles a link's label and destination.
fn is_link_style(style: Style) -> bool {
    style.fg == Some(Color::Blue) && style.add_modifier.contains(Modifier::UNDERLINED)
}

/// A URL in plain text, which `tui-markdown` leaves as it is.
static BARE_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s<>"]+"#).unwrap());

/// Punctuation that ends a sentence rather than a URL.
const URL_TRAILING: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '\'', '"'];

/// Finds the links of a line of rendered markdown. `tui-markdown` draws a link as its label in the
/// link style, then ` (`, the destination in the link style, then `)`; an autolink the same, with
/// the URL as its label. The bare URLs in plain text are found by their look and given the link
/// style, so they can be seen to be links. Text with a background, which is code, is left alone.
fn find_links(line: Line<'static>) -> (Line<'static>, Vec<LinkSpan>) {
    let spans = line.spans;
    let mut group: Vec<Option<usize>> = vec![None; spans.len()];
    let mut urls: Vec<String> = Vec::new();
    for i in 1..spans.len().saturating_sub(1) {
        let is_destination = is_link_style(spans[i].style)
            && spans[i - 1].content == " ("
            && spans[i + 1].content == ")";
        if !is_destination {
            continue;
        }
        let id = urls.len();
        urls.push(spans[i].content.to_string());
        let mut first = i - 1;
        while first > 0 && is_link_style(spans[first - 1].style) && group[first - 1].is_none() {
            first -= 1;
        }
        group[first..=i + 1].fill(Some(id));
    }

    let mut parts: Vec<(Span<'static>, Option<usize>)> = Vec::new();
    for (span, group) in spans.into_iter().zip(group) {
        if group.is_some() || is_link_style(span.style) || span.style.bg.is_some() {
            parts.push((span, group));
            continue;
        }
        let text = span.content.to_string();
        let mut from = 0;
        for found in BARE_URL.find_iter(&text) {
            let url = found.as_str().trim_end_matches(URL_TRAILING);
            if url.ends_with("://") {
                continue;
            }
            let end = found.start() + url.len();
            if found.start() > from {
                parts.push((
                    Span::styled(text[from..found.start()].to_string(), span.style),
                    None,
                ));
            }
            let style = span.style.patch(Style::new().blue().underlined());
            parts.push((Span::styled(url.to_string(), style), Some(urls.len())));
            urls.push(url.to_string());
            from = end;
        }
        if from < text.len() {
            parts.push((Span::styled(text[from..].to_string(), span.style), None));
        }
    }

    let mut links = Vec::new();
    let mut spans = Vec::new();
    let mut column = 0;
    let mut open: Option<(usize, usize)> = None;
    for (span, group) in parts {
        if open.map(|(id, _)| id) != group {
            if let Some((id, start)) = open.take() {
                links.push(LinkSpan {
                    start,
                    end: column,
                    url: urls[id].clone(),
                });
            }
            open = group.map(|id| (id, column));
        }
        column += span.width();
        spans.push(span);
    }
    if let Some((id, start)) = open {
        links.push(LinkSpan {
            start,
            end: column,
            url: urls[id].clone(),
        });
    }
    (Line { spans, ..line }, links)
}

/// Whether `tui-markdown` drew this line as part of a table.
fn is_table_line(line: &Line) -> bool {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .find(|c| !c.is_whitespace())
        .is_some_and(|c| matches!(c, '┌' | '│' | '├' | '└'))
}

/// The width of the widest table line, which is as far as a body can be scrolled sideways.
pub fn widest_table(text: &Text) -> usize {
    text.lines
        .iter()
        .filter(|line| is_table_line(line))
        .map(Line::width)
        .max()
        .unwrap_or(0)
}

/// Rendered markdown laid out for a pane of some width: the lines to draw, and the links of each.
#[derive(Debug, Default)]
pub struct Laid {
    pub lines: Vec<Line<'static>>,
    /// The links of each of `lines`, in columns of that line as drawn.
    pub links: Vec<Vec<LinkSpan>>,
    /// How each of `lines` goes on from the one before, so that text selected over several rows
    /// can be put back as the lines it was written in.
    pub joins: Vec<Join>,
    /// Whether each of `lines` is a row of a table.
    pub tables: Vec<bool>,
}

/// How a laid-out row goes on from the row before it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Join {
    /// It starts a line of its own.
    #[default]
    New,
    /// The line was wrapped between two words: the space between them is at the end of the row
    /// before, or was dropped for want of room.
    Word,
    /// The line was wrapped inside a word too long for a row, such as a URL.
    Split,
}

/// Lays a body out in `width` columns. A table line is cropped to `width` columns starting `x`
/// columns in, so a table can be scrolled sideways while the text around it stays put; the rest
/// is wrapped. The wrapping is done here, rather than by the widget that draws the lines, so that
/// a link can be found from where it was drawn.
pub fn layout_markdown(markdown: &Markdown, width: usize, x: usize) -> Laid {
    let mut laid = Laid::default();
    for (line, links) in markdown.text.lines.iter().zip(&markdown.links) {
        if is_table_line(line) {
            laid.lines.push(crop_line(line, width, x));
            let shifted = links.iter().filter_map(|link| {
                let start = link.start.saturating_sub(x);
                let end = link.end.saturating_sub(x).min(width);
                (end > start).then(|| LinkSpan {
                    start,
                    end,
                    url: link.url.clone(),
                })
            });
            laid.links.push(shifted.collect());
            laid.joins.push(Join::New);
            laid.tables.push(true);
        } else {
            let (lines, links, joins) = wrap_rows(line, links, width);
            laid.tables.extend(lines.iter().map(|_| false));
            laid.lines.extend(lines);
            laid.links.extend(links);
            laid.joins.extend(joins);
        }
    }
    laid
}

/// Lays out text with no links, wrapped to `width`.
pub fn layout_plain(text: &Text<'static>, width: usize) -> Laid {
    let mut laid = Laid::default();
    for line in &text.lines {
        let (lines, links, joins) = wrap_rows(line, &[], width);
        laid.tables.extend(lines.iter().map(|_| false));
        laid.lines.extend(lines.into_iter().map(|mut row| {
            row.style = text.style.patch(row.style);
            row
        }));
        laid.links.extend(links);
        laid.joins.extend(joins);
    }
    laid
}

/// One character of a line being wrapped.
struct Glyph {
    ch: char,
    style: Style,
    width: usize,
    /// The column it started at in the line before wrapping.
    column: usize,
}

/// Wraps a line to `width` columns, at spaces where it can and inside a word that is longer than
/// a row, such as a URL. A space that fits at the end of a row stays, as does one that starts a
/// line, and one with no room is dropped. Returns the rows, and the links of each in columns of
/// that row.
#[cfg(test)]
pub fn wrap_line(
    line: &Line<'static>,
    links: &[LinkSpan],
    width: usize,
) -> (Vec<Line<'static>>, Vec<Vec<LinkSpan>>) {
    let (lines, links, _) = wrap_rows(line, links, width);
    (lines, links)
}

/// [`wrap_line`], also saying how each row goes on from the one before.
pub fn wrap_rows(
    line: &Line<'static>,
    links: &[LinkSpan],
    width: usize,
) -> (Vec<Line<'static>>, Vec<Vec<LinkSpan>>, Vec<Join>) {
    let width = width.max(1);
    let mut glyphs = Vec::new();
    let mut column = 0;
    for span in &line.spans {
        for ch in span.content.chars() {
            let char_width = ch.width().unwrap_or(0);
            glyphs.push(Glyph {
                ch,
                style: span.style,
                width: char_width,
                column,
            });
            column += char_width;
        }
    }

    let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
    // How each row goes on from the one before it.
    let mut joins = vec![Join::New];
    let mut used = 0;
    let mut i = 0;
    while i < glyphs.len() {
        let space = glyphs[i].ch.is_whitespace();
        let mut end = i;
        while end < glyphs.len() && glyphs[end].ch.is_whitespace() == space {
            end += 1;
        }
        let run_width: usize = glyphs[i..end].iter().map(|g| g.width).sum();
        if space {
            for (k, glyph) in glyphs.iter().enumerate().take(end).skip(i) {
                if used + glyph.width <= width {
                    rows.last_mut().unwrap().push(k);
                    used += glyph.width;
                }
            }
        } else if used + run_width <= width {
            rows.last_mut().unwrap().extend(i..end);
            used += run_width;
        } else if run_width <= width {
            rows.push((i..end).collect());
            joins.push(Join::Word);
            used = run_width;
        } else {
            if used > 0 {
                rows.push(Vec::new());
                joins.push(Join::Word);
                used = 0;
            }
            for (k, glyph) in glyphs.iter().enumerate().take(end).skip(i) {
                if used > 0 && used + glyph.width > width {
                    rows.push(Vec::new());
                    joins.push(Join::Split);
                    used = 0;
                }
                rows.last_mut().unwrap().push(k);
                used += glyph.width;
            }
        }
        i = end;
    }

    let mut lines = Vec::new();
    let mut row_links = Vec::new();
    for row in rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut regions = Vec::new();
        let mut text = String::new();
        let mut style = Style::default();
        let mut open: Option<(usize, usize)> = None;
        let mut x = 0;
        for &k in &row {
            let glyph = &glyphs[k];
            if !text.is_empty() && glyph.style != style {
                spans.push(Span::styled(std::mem::take(&mut text), style));
            }
            style = glyph.style;
            text.push(glyph.ch);
            let link = links
                .iter()
                .position(|link| link.start <= glyph.column && glyph.column < link.end);
            if open.map(|(index, _)| index) != link {
                if let Some((index, start)) = open.take() {
                    regions.push(LinkSpan {
                        start,
                        end: x,
                        url: links[index].url.clone(),
                    });
                }
                open = link.map(|index| (index, x));
            }
            x += glyph.width;
        }
        if !text.is_empty() {
            spans.push(Span::styled(text, style));
        }
        if let Some((index, start)) = open {
            regions.push(LinkSpan {
                start,
                end: x,
                url: links[index].url.clone(),
            });
        }
        lines.push(Line {
            spans,
            style: line.style,
            alignment: line.alignment,
        });
        row_links.push(regions);
    }
    (lines, row_links, joins)
}

/// `width` columns of a line starting `x` columns in.
fn crop_line(line: &Line<'static>, width: usize, x: usize) -> Line<'static> {
    let mut column = 0;
    let mut spans = Vec::new();
    for span in &line.spans {
        let mut kept = String::new();
        for c in span.content.chars() {
            let w = c.width().unwrap_or(0);
            if column >= x && column + w <= x + width {
                kept.push(c);
            }
            column += w;
        }
        if !kept.is_empty() {
            spans.push(Span::styled(kept, span.style));
        }
    }
    Line {
        spans,
        ..line.clone()
    }
}

/// `text` without its first `x` display columns, for scrolling a line sideways.
pub fn skip_columns(text: &str, x: usize) -> String {
    let mut column = 0;
    text.chars()
        .filter(|c| {
            let keep = column >= x;
            column += c.width().unwrap_or(0);
            keep
        })
        .collect()
}

/// Names the title columns can't use, as other parts of a PR already have them.
const RESERVED_COLUMNS: [&str; 13] = [
    "number", "title", "branch", "url", "changes", "body", "comments", "diff", "state", "checks",
    "runs", "review", "updated",
];

/// Compiles the column regex and checks its named groups can be columns.
pub fn parse_column_regex(pattern: &str) -> Result<Regex, String> {
    let regex = Regex::new(pattern).map_err(|error| error.to_string())?;
    let names = column_names(&regex);
    if names.is_empty() {
        return Err("the column regex needs at least one named group, e.g. '(?<package>\\S+) to (?<version>\\S+)'".to_string());
    }
    let clashes: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| RESERVED_COLUMNS.contains(name))
        .collect();
    if !clashes.is_empty() {
        return Err(format!(
            "the column regex's group names clash with built-in columns: {}",
            clashes.join(", ")
        ));
    }
    Ok(regex)
}

/// The names of the regex's groups, in order. They are the table's column headers.
pub fn column_names(regex: &Regex) -> Vec<String> {
    regex
        .capture_names()
        .flatten()
        .map(str::to_string)
        .collect()
}

/// What each named group matched in `title`, or empty text when the title doesn't match.
pub fn column_cells(regex: &Regex, title: &str) -> Vec<String> {
    let captures = regex.captures(title);
    column_names(regex)
        .iter()
        .map(|name| {
            captures
                .as_ref()
                .and_then(|captures| captures.name(name))
                .map_or("", |found| found.as_str())
                .to_string()
        })
        .collect()
}

/// The local filter, matching like fzf: the characters of a term in order but not necessarily
/// together, with smart case (lowercase matches any case, an upper-case letter matches exactly).
/// Terms are separated by spaces and must all match, each against any column.
pub struct Filter {
    atoms: Vec<Atom>,
    matcher: Matcher,
    buffer: Vec<char>,
}

impl Filter {
    pub fn new(text: &str) -> Self {
        let atoms = text
            .split_whitespace()
            .map(|term| {
                Atom::new(
                    term,
                    CaseMatching::Smart,
                    Normalization::Smart,
                    AtomKind::Fuzzy,
                    false,
                )
            })
            .collect();
        Self {
            atoms,
            matcher: Matcher::new(Config::DEFAULT),
            buffer: Vec::new(),
        }
    }

    /// Whether a row with these columns passes. An empty filter passes every row.
    pub fn matches(&mut self, columns: &[String]) -> bool {
        self.atoms.iter().all(|atom| {
            columns.iter().any(|column| {
                let haystack = Utf32Str::new(column, &mut self.buffer);
                atom.score(haystack, &mut self.matcher).is_some()
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passes(filter: &str, columns: &[&str]) -> bool {
        let columns: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
        Filter::new(filter).matches(&columns)
    }

    #[test]
    fn skipping_columns_drops_the_start_of_a_line() {
        assert_eq!(skip_columns("CI / build", 0), "CI / build");
        assert_eq!(skip_columns("CI / build", 5), "build");
        assert_eq!(skip_columns("CI / build", 99), "");
        // A wide character is two columns.
        assert_eq!(skip_columns("日本語", 2), "本語");
    }

    #[test]
    fn a_filter_matches_characters_in_order_but_spread_out() {
        assert!(passes("rnv", &["app/renovate"]));
        assert!(!passes("vnr", &["app/renovate"]));
    }

    #[test]
    fn a_filter_with_no_match_passes_nothing() {
        assert!(!passes("xyz", &["#1", "app/renovate", "Bump alpha"]));
    }

    #[test]
    fn lowercase_matches_any_case_and_upper_case_matches_exactly() {
        assert!(passes("bump", &["Bump alpha"]));
        assert!(passes("Bump", &["Bump alpha"]));
        assert!(!passes("BUMP", &["Bump alpha"]));
        assert!(!passes("Bump", &["bump alpha"]));
    }

    #[test]
    fn every_term_must_match_some_column() {
        let columns = ["#123", "app/renovate", "Bump alpha"];
        assert!(passes("deps 123", &["#123", "deps"]));
        assert!(passes("renovate alpha", &columns));
        assert!(!passes("renovate beta", &columns));
    }

    #[test]
    fn an_empty_filter_passes_every_row() {
        assert!(passes("", &["#1"]));
        assert!(passes("   ", &[]));
    }

    #[test]
    fn a_term_does_not_match_across_columns() {
        assert!(!passes("1ren", &["#1", "app/renovate"]));
    }
    // The checks are built from JSON, as gh prints them.
    fn run(name: &str, status: &str, conclusion: &str, started_at: Option<&str>) -> Check {
        serde_json::from_value(serde_json::json!({
            "__typename": "CheckRun",
            "name": name,
            "workflowName": "CI",
            "status": status,
            "conclusion": conclusion,
            "startedAt": started_at,
        }))
        .unwrap()
    }

    fn ctx(context: &str, state: &str, started_at: Option<&str>) -> Check {
        serde_json::from_value(serde_json::json!({
            "__typename": "StatusContext",
            "context": context,
            "state": state,
            "startedAt": started_at,
        }))
        .unwrap()
    }

    fn decision(value: &str) -> ReviewDecision {
        serde_json::from_value(serde_json::json!(value)).unwrap()
    }

    #[test]
    fn an_unknown_check_keeps_the_pr_from_passing() {
        let checks = classify_checks(
            Mergeable::Other,
            &[ctx("ci/legacy", "SUCCESS", None), Check::Unknown],
        );
        assert_eq!(checks.state, CheckState::Pending);
        // It is listed, so the checks pane shows why.
        assert_eq!(checks.results.len(), 2);

        // On its own it is pending too, not "no checks".
        let checks = classify_checks(Mergeable::Other, &[Check::Unknown]);
        assert_eq!(checks.state, CheckState::Pending);
        // A failing check still outranks it.
        let checks = classify_checks(
            Mergeable::Other,
            &[Check::Unknown, ctx("a", "FAILURE", None)],
        );
        assert_eq!(checks.state, CheckState::Failing);
    }

    #[test]
    fn check_run_state() {
        assert_eq!(
            check_state(&run("a", "IN_PROGRESS", "", None)),
            CheckState::Pending
        );
        assert_eq!(
            check_state(&run("a", "QUEUED", "SUCCESS", None)),
            CheckState::Pending
        );
        for conclusion in ["SUCCESS", "NEUTRAL", "SKIPPED"] {
            let check = run("a", "COMPLETED", conclusion, None);
            assert_eq!(check_state(&check), CheckState::Passing, "{conclusion}");
        }
        for conclusion in ["FAILURE", "CANCELLED", "TIMED_OUT", "ACTION_REQUIRED", ""] {
            let check = run("a", "COMPLETED", conclusion, None);
            assert_eq!(check_state(&check), CheckState::Failing, "{conclusion}");
        }
    }

    #[test]
    fn status_context_state() {
        assert_eq!(check_state(&ctx("c", "SUCCESS", None)), CheckState::Passing);
        assert_eq!(check_state(&ctx("c", "PENDING", None)), CheckState::Pending);
        assert_eq!(
            check_state(&ctx("c", "EXPECTED", None)),
            CheckState::Pending
        );
        assert_eq!(check_state(&ctx("c", "FAILURE", None)), CheckState::Failing);
        assert_eq!(check_state(&ctx("c", "ERROR", None)), CheckState::Failing);
    }

    #[test]
    fn keeps_latest_run_per_key() {
        let checks = classify_checks(
            Mergeable::Other,
            &[
                run(
                    "build",
                    "COMPLETED",
                    "FAILURE",
                    Some("2026-01-01T10:00:00Z"),
                ),
                run(
                    "build",
                    "COMPLETED",
                    "SUCCESS",
                    Some("2026-01-01T11:00:00Z"),
                ),
                run("build", "COMPLETED", "FAILURE", None),
                ctx("ci/legacy", "PENDING", None),
                ctx("ci/legacy", "SUCCESS", Some("2026-01-01T09:00:00Z")),
            ],
        );
        assert_eq!(
            checks.results,
            vec![
                CheckResult {
                    key: "CI/build".to_string(),
                    state: CheckState::Passing,
                    url: None,
                },
                CheckResult {
                    key: "ci/legacy".to_string(),
                    state: CheckState::Passing,
                    url: None,
                },
            ]
        );
        assert_eq!(checks.state, CheckState::Passing);
    }

    #[test]
    fn humanises_times() {
        let now: jiff::Timestamp = "2026-03-10T12:00:00Z".parse().unwrap();
        let ago = |timestamp| ago(timestamp, now);
        assert_eq!(ago("2026-03-10T12:00:00Z"), "just now");
        assert_eq!(ago("2026-03-10T11:59:01Z"), "just now");
        assert_eq!(ago("2026-03-10T11:59:00Z"), "1 minute ago");
        assert_eq!(ago("2026-03-10T11:15:00Z"), "45 minutes ago");
        assert_eq!(ago("2026-03-10T09:00:00Z"), "3 hours ago");
        assert_eq!(ago("2026-03-09T12:00:00Z"), "1 day ago");
        assert_eq!(ago("2026-03-01T12:00:00Z"), "1 week ago");
        assert_eq!(ago("2026-01-10T12:00:00Z"), "1 month ago");
        assert_eq!(ago("2024-03-10T12:00:00Z"), "2 years ago");
        // The future, such as a clock that is a little off, isn't an error.
        assert_eq!(ago("2026-03-10T12:00:30Z"), "just now");
        assert_eq!(ago("yesterday"), "yesterday");
    }

    #[test]
    fn shortens_times_to_a_number_and_a_unit() {
        let now: jiff::Timestamp = "2026-03-10T12:00:00Z".parse().unwrap();
        let short = |timestamp| short_age(timestamp, now);
        assert_eq!(short("2026-03-10T11:59:30Z"), "now");
        assert_eq!(short("2026-03-10T11:15:00Z"), "45m");
        assert_eq!(short("2026-03-10T09:00:00Z"), "3h");
        assert_eq!(short("2026-03-07T12:00:00Z"), "3d");
        assert_eq!(short("2026-02-24T12:00:00Z"), "2w");
        assert_eq!(short("2025-12-10T12:00:00Z"), "3mo");
        assert_eq!(short("2024-03-10T12:00:00Z"), "2y");
        assert_eq!(short("2026-03-10T12:00:30Z"), "now");
        assert_eq!(short("yesterday"), "yesterday");
    }

    #[test]
    fn checks_pane_lists_the_worst_first() {
        let checks = classify_checks(
            Mergeable::Other,
            &[
                run("a", "COMPLETED", "SUCCESS", None),
                run("b", "IN_PROGRESS", "", None),
                run("c", "COMPLETED", "FAILURE", None),
                run("d", "COMPLETED", "SUCCESS", None),
                ctx("e", "ERROR", None),
                run("f", "QUEUED", "", None),
            ],
        );
        let keys: Vec<&str> = checks
            .worst_first()
            .into_iter()
            .map(|result| result.key.as_str())
            .collect();
        assert_eq!(keys, ["CI/c", "e", "CI/b", "CI/f", "CI/a", "CI/d"]);
    }

    #[test]
    fn overall_state_priority() {
        let pass = run("a", "COMPLETED", "SUCCESS", None);
        let pend = run("b", "IN_PROGRESS", "", None);
        let fail = run("c", "COMPLETED", "FAILURE", None);
        let state = |mergeable, checks: &[&Check]| {
            let checks: Vec<Check> = checks.iter().map(|c| (*c).clone()).collect();
            classify_checks(mergeable, &checks).state
        };
        assert_eq!(
            state(Mergeable::Conflicting, &[&pass]),
            CheckState::Conflict
        );
        assert_eq!(state(Mergeable::Conflicting, &[]), CheckState::Conflict);
        assert_eq!(state(Mergeable::Other, &[]), CheckState::None);
        assert_eq!(
            state(Mergeable::Other, &[&pass, &pend, &fail]),
            CheckState::Failing
        );
        assert_eq!(
            state(Mergeable::Other, &[&pass, &pend]),
            CheckState::Pending
        );
        assert_eq!(state(Mergeable::Other, &[&pass]), CheckState::Passing);
    }

    #[test]
    fn ranks() {
        assert_eq!(CheckState::Passing.rank(), 0);
        assert_eq!(CheckState::Pending.rank(), 1);
        assert_eq!(CheckState::None.rank(), 1);
        assert_eq!(CheckState::Failing.rank(), 2);
        assert_eq!(CheckState::Conflict.rank(), 2);
    }

    #[test]
    fn review_labels() {
        assert_eq!(review_label(decision("APPROVED")), "✓ approved");
        assert_eq!(review_label(decision("CHANGES_REQUESTED")), "✗ changes");
        assert_eq!(review_label(decision("REVIEW_REQUIRED")), "· needed");
        assert_eq!(review_label(decision("")), "· needed");
    }

    #[test]
    fn the_first_line_skips_blank_ones() {
        assert_eq!(first_line("\n  headline  \nrest"), "headline");
        assert_eq!(first_line("one"), "one");
        assert_eq!(first_line(" \n"), "");
    }

    #[test]
    fn cleans_body() {
        assert_eq!(clean_body(None), "");
        assert_eq!(
            clean_body(Some(
                "<!-- a\nmulti-line -->Hello<!-- b -->\n\n\n\n\nWorld<!-- c -->\n"
            )),
            "Hello\n\nWorld"
        );
        assert_eq!(clean_body(Some("one\r\n\r\n\r\n\r\ntwo")), "one\n\ntwo");
        assert_eq!(clean_body(Some("keep\n\nthis")), "keep\n\nthis");
    }

    /// The text of `line` that a link covers.
    fn covered(line: &Line, link: &LinkSpan) -> String {
        line.to_string()
            .chars()
            .skip(link.start)
            .take(link.end - link.start)
            .collect()
    }

    #[test]
    fn an_inline_link_covers_its_label_and_destination() {
        let md = render_markdown("See [the notes](https://a.example/n) for more.");
        let links = &md.links[0];
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://a.example/n");
        assert_eq!(
            covered(&md.text.lines[0], &links[0]),
            "the notes (https://a.example/n)"
        );
    }

    #[test]
    fn an_autolink_is_a_link() {
        let md = render_markdown("Go to <https://a.example/auto> now");
        assert_eq!(md.links[0].len(), 1);
        assert_eq!(md.links[0][0].url, "https://a.example/auto");
    }

    #[test]
    fn a_bare_url_is_a_link_without_the_punctuation_after_it() {
        for (body, url) in [
            ("see https://a.example/x, and", "https://a.example/x"),
            ("(see https://a.example/x)", "https://a.example/x"),
            ("ends https://a.example/x.", "https://a.example/x"),
            (
                "query https://a.example/x?a=1&b=2 ok",
                "https://a.example/x?a=1&b=2",
            ),
        ] {
            let md = render_markdown(body);
            assert_eq!(md.links[0].len(), 1, "{body}");
            assert_eq!(md.links[0][0].url, url, "{body}");
            assert_eq!(covered(&md.text.lines[0], &md.links[0][0]), url, "{body}");
        }
        // It is drawn as a link, so it can be seen to be one.
        let md = render_markdown("see https://a.example/x");
        let span = md.text.lines[0]
            .spans
            .iter()
            .find(|span| span.content == "https://a.example/x")
            .unwrap();
        assert!(is_link_style(span.style));
    }

    #[test]
    fn a_url_in_code_is_not_a_link() {
        let md = render_markdown("run `curl https://a.example/x` now");
        assert!(md.links[0].is_empty());
    }

    #[test]
    fn several_links_in_a_line_are_found_in_order() {
        let md = render_markdown(
            "[a](https://a.example/1) and [b](https://a.example/2) and https://a.example/3",
        );
        let urls: Vec<&str> = md.links[0].iter().map(|l| l.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://a.example/1",
                "https://a.example/2",
                "https://a.example/3"
            ]
        );
        assert!(md.links[0][0].end <= md.links[0][1].start);
        assert!(md.links[0][1].end <= md.links[0][2].start);
    }

    #[test]
    fn a_link_in_a_table_cell_is_a_link() {
        let md = render_markdown("| Pkg | Ver |\n|---|---|\n| [lib](https://a.example/l) | 1 |");
        let (line, links) = md
            .text
            .lines
            .iter()
            .zip(&md.links)
            .find(|(_, links)| !links.is_empty())
            .unwrap();
        assert_eq!(links[0].url, "https://a.example/l");
        assert_eq!(covered(line, &links[0]), "lib (https://a.example/l)");
    }

    #[test]
    fn a_line_with_no_links_has_none() {
        let md = render_markdown("just words\n\n- a list");
        assert!(md.links.iter().all(Vec::is_empty));
        assert_eq!(md.links.len(), md.text.lines.len());
    }

    fn rows(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::to_string).collect()
    }

    #[test]
    fn wrapping_breaks_at_spaces_and_keeps_the_spaces_that_fit() {
        let (lines, _) = wrap_line(&Line::from("one two three four"), &[], 9);
        assert_eq!(rows(&lines), ["one two ", "three ", "four"]);
        // A space with no room is dropped rather than starting the next row.
        let (lines, _) = wrap_line(&Line::from("abcd efgh"), &[], 4);
        assert_eq!(rows(&lines), ["abcd", "efgh"]);
        let (lines, _) = wrap_line(&Line::from("  indented text"), &[], 40);
        assert_eq!(rows(&lines), ["  indented text"]);
        let (lines, _) = wrap_line(&Line::from(""), &[], 10);
        assert_eq!(rows(&lines), [""]);
    }

    #[test]
    fn a_word_longer_than_a_row_breaks_inside_it() {
        let (lines, _) = wrap_line(&Line::from("ab abcdefghij"), &[], 4);
        assert_eq!(rows(&lines), ["ab ", "abcd", "efgh", "ij"]);
    }

    #[test]
    fn a_url_longer_than_the_width_wraps_with_its_link_on_every_row() {
        let md = render_markdown("https://example.test/a/very/long/path");
        let (lines, links) = wrap_line(&md.text.lines[0], &md.links[0], 12);
        assert_eq!(
            rows(&lines).concat(),
            "https://example.test/a/very/long/path"
        );
        assert!(lines.len() > 2);
        for (line, links) in lines.iter().zip(&links) {
            assert_eq!(links.len(), 1, "{line}");
            assert_eq!(links[0].url, "https://example.test/a/very/long/path");
            assert_eq!(covered(line, &links[0]), line.to_string());
        }
    }

    #[test]
    fn wrapping_keeps_the_styles_and_finds_a_link_that_starts_mid_row() {
        let md = render_markdown("go to [the notes](https://a.example/n) please");
        let (lines, links) = wrap_line(&md.text.lines[0], &md.links[0], 24);
        assert_eq!(
            rows(&lines),
            ["go to the notes ", "(https://a.example/n) ", "please"]
        );
        // The label is on the first row and the destination on the second.
        // The link takes in the space before the destination, so a click there counts too.
        assert_eq!(covered(&lines[0], &links[0][0]), "the notes ");
        assert_eq!(covered(&lines[1], &links[1][0]), "(https://a.example/n)");
        assert!(lines[0].spans.iter().any(|span| is_link_style(span.style)));
    }

    #[test]
    fn a_table_is_cropped_and_its_links_move_with_it() {
        let md = render_markdown("| Pkg | Ver |\n|---|---|\n| [lib](https://a.example/l) | 1 |");
        let row = md.links.iter().position(|links| !links.is_empty()).unwrap();
        let full = md.links[row][0].clone();

        let laid = layout_markdown(&md, 12, 0);
        assert_eq!(laid.lines.len(), md.text.lines.len());
        assert!(laid.lines.iter().all(|line| line.width() <= 12));

        let laid = layout_markdown(&md, 12, 1);
        let link = &laid.links[row][0];
        assert_eq!((link.start, link.end), (full.start - 1, 12));
        assert!(covered(&laid.lines[row], link).starts_with("lib"));

        // Scrolled past it, it is not there to click.
        let laid = layout_markdown(&md, 12, full.end);
        assert!(laid.links[row].is_empty());
    }
    #[test]
    fn cropping_moves_only_table_lines() {
        let text = render_markdown(
            "A paragraph of prose.\n\n| Name | Value |\n|---|---|\n| alpha | 123456789 |",
        );
        let widest = widest_table(&text.text);
        assert!(widest > 10);
        let before: Vec<String> = text.text.lines.iter().map(Line::to_string).collect();
        // Wide enough that the prose does not wrap, so only the tables change.
        let laid = layout_markdown(&text, 40, 2);
        let cropped: Vec<String> = laid.lines.iter().map(Line::to_string).collect();
        assert_eq!(before.len(), cropped.len());
        for (before, cropped) in before.iter().zip(&cropped) {
            if before.starts_with(['┌', '│', '├', '└']) {
                assert_eq!(
                    cropped,
                    &before.chars().skip(2).take(40).collect::<String>()
                );
            } else {
                assert_eq!(cropped, before);
            }
        }
    }

    #[test]
    fn markdown_keeps_details_tags_and_shows_image_alt_text() {
        let text = render_markdown(
            "<details>\n<summary>Notes</summary>\n\n![badge](https://example.com/b.svg)\n\n</details>",
        );
        let plain: Vec<String> = text.text.lines.iter().map(Line::to_string).collect();
        assert!(
            plain.contains(&"<summary>Notes</summary>".to_string()),
            "{plain:?}"
        );
        assert!(
            plain
                .iter()
                .any(|l| l.contains("badge") && !l.contains("example.com")),
            "{plain:?}"
        );
    }

    const BUMP: &str = r"Bump (?<container>.+?)(?: from \S+)? to (?<version>\S+)$";

    #[test]
    fn the_columns_are_the_named_groups_in_order() {
        let regex = parse_column_regex(BUMP).unwrap();
        assert_eq!(column_names(&regex), ["container", "version"]);
        assert_eq!(
            column_cells(&regex, "Bump example-lib from 1.0.0 to 1.2.0"),
            ["example-lib", "1.2.0"]
        );
        assert_eq!(
            column_cells(&regex, "Bump example-lib to 1.2.0"),
            ["example-lib", "1.2.0"]
        );
        // A title that doesn't match, or a group that doesn't take part, gives an empty cell.
        assert_eq!(column_cells(&regex, "Fix a bug"), ["", ""]);
        let optional = parse_column_regex(r"(?<a>x)(?<b>y)?").unwrap();
        assert_eq!(column_cells(&optional, "x"), ["x", ""]);
    }

    #[test]
    fn a_bad_column_regex_is_rejected() {
        assert!(parse_column_regex("(unclosed").is_err());
        let error = parse_column_regex(r"(\S+) to (\S+)").unwrap_err();
        assert!(error.contains("at least one named group"));
        let error = parse_column_regex(r"(?<title>\S+) to (?<version>\S+)").unwrap_err();
        assert_eq!(
            error,
            "the column regex's group names clash with built-in columns: title"
        );
        let error = parse_column_regex(r"(?<checks>\S+) (?<review>\S+)").unwrap_err();
        assert!(error.ends_with("checks, review"));
    }
}
