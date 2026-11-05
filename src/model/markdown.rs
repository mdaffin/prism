//! Rendering a markdown body for a pane: finding its links, wrapping it and cropping tables.

use std::sync::LazyLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use regex::Regex;
use unicode_width::UnicodeWidthChar;

use super::clean_body;
use crate::gh::Comment;

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
    // Pass 1: find each ` (destination)` and give it, and the label before it, a group number.
    // `group[i]` says which link span `i` belongs to, and `urls[group]` is where it goes.
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

    // Pass 2: split the plain text around bare URLs, so each URL is a span of its own in the link
    // style. Spans already in a group, in the link style or with a background are kept as they are.
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

    // Pass 3: join the spans of a group into one link, noting the columns it covers.
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
///
/// It works in three passes: break the line into [`Glyph`]s, decide which glyphs go on which row
/// (and how each row joins the last), then build each row's spans and the links inside it.
pub fn wrap_rows(
    line: &Line<'static>,
    links: &[LinkSpan],
    width: usize,
) -> (Vec<Line<'static>>, Vec<Vec<LinkSpan>>, Vec<Join>) {
    let width = width.max(1);
    // Pass 1: one glyph per character, remembering its style and its column before wrapping.
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

    // Pass 2: walk the glyphs a run of spaces or a word at a time, and put each run on a row. A
    // run that fits goes on the current row; a word that doesn't moves to a new row; a word wider
    // than a row is split across rows.
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

    // Pass 3: turn each row's glyphs back into spans, and the links that cover them into regions
    // in the row's own columns.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipping_columns_drops_the_start_of_a_line() {
        assert_eq!(skip_columns("CI / build", 0), "CI / build");
        assert_eq!(skip_columns("CI / build", 5), "build");
        assert_eq!(skip_columns("CI / build", 99), "");
        // A wide character is two columns.
        assert_eq!(skip_columns("日本語", 2), "本語");
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
}
