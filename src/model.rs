//! PR logic with no I/O. The code lives in the modules below by topic; this file re-exports what
//! the rest of the program uses, so it can say `model::Column` and the like.

mod checks;
mod columns;
mod filter;
mod markdown;
mod text;
mod time;

pub use checks::{CheckState, Checks, classify_checks, review_label};
pub use columns::{Column, Sort, column_cells, column_names, parse_column_regex};
pub use filter::Filter;
pub use markdown::{
    Join, Laid, LinkSpan, Markdown, layout_markdown, layout_plain, render_comment, render_markdown,
    skip_columns, widest_table, wrap_rows,
};
pub use text::{clean_body, first_line, one_line, regex_error_line};
pub use time::{ago, short_age};
