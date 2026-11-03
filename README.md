# prism

A terminal UI for reviewing, approving and merging pull requests in bulk. It is built for the
queue of near-identical PRs that bots like Renovate open: scan the list, check which are green,
read a few, then approve and merge them in one go.

It talks to GitHub only through the [`gh`](https://cli.github.com) CLI, so `gh` must be installed
and signed in.

```sh
prism --search "status:success label:demo"
prism --author app/renovate
prism --repo OWNER/NAME --author app/renovate
```

## Options

| Option | What it does |
|---|---|
| `--repo OWNER/NAME` | The repository to list. Default: the repository of the current directory. |
| `--author AUTHOR` | Only PRs by this author, such as `app/renovate`. Default: any author. Also read from `$PRISM_AUTHOR`. |
| `--search TERMS` | Extra GitHub search terms, added to the starting query. |
| `--column-regex REGEX` | Replaces the title column with one column per named group. Also read from `$PRISM_COLUMN_REGEX`. |
| `--limit N` | Most PRs to fetch at once. Default: 30. Lower it if GitHub times out on a large repository. |
| `--merge-method METHOD` | `squash` (the default), `merge` or `rebase`. |
| `--history-limit N` | Most search queries to remember between runs. Also read from `$PRISM_HISTORY_LIMIT`. |
| `--no-mouse` | Don't capture the mouse, so your terminal's own text selection works. |
| `--dry-run` | Walk through the flow without approving or merging anything. |

`prism --help` lists the same options.

## Config file

Defaults for the options can live in `config.toml` in prism's config directory
(`~/.config/prism/` on Linux, `~/Library/Application Support/prism/` on macOS). A flag or
environment variable wins over the file. A missing file is fine; an unreadable one, or one with an
unknown key, stops prism from starting rather than being ignored.

```toml
author = "app/renovate"
search = "label:non-prod -label:major"
column_regex = 'Bump (?<container>.+?)(?: from \S+)? to (?<version>\S+)$'
limit = 50
merge_method = "squash"   # or "merge", "rebase"
mouse = true              # false is like --no-mouse
history_limit = 100
```

## Searching

The starting query is `is:pr state:open`, plus `author:AUTHOR` and your `--search` terms. It is
shown in the search box, where you can change it freely. Anything GitHub's PR search accepts
works. For example:

```sh
# Renovate PRs labelled non-prod, except major upgrades
prism --author app/renovate --search 'label:non-prod -label:major'

# PRs that still need a review, and aren't drafts
prism --search 'review:required draft:false'

# PRs into main whose checks have all passed
prism --search 'base:main status:success'

# PRs with "lint" in the title, not touched for a month
prism --search 'lint in:title updated:<2026-10-01'
```

While you type, prism suggests completions below the box for qualifiers and for the repository's
labels, branches, users and teams. Tab or Down chooses one, Enter takes it. Up and Down with no
list walk through earlier queries, which are remembered between runs.

See GitHub's [search syntax for issues and pull
requests](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests)
for every qualifier.

## Columns from titles

Bot PR titles follow a pattern, so `--column-regex` can turn them into columns. Each named group
becomes a column headed by the group's name, in place of the title. A title that doesn't match
gets empty cells. Names that the table already uses (`number`, `title`, `checks`, `review` and
so on) can't be used.

```sh
# "Bump example-lib from 1.0.0 to 1.2.0" -> container: example-lib, version: 1.2.0
prism --column-regex 'Bump (?<container>.+?)(?: from \S+)? to (?<version>\S+)$'

# "Update dependency eslint to v9" -> package: eslint, version: v9
prism --column-regex 'Update dependency (?<package>\S+) to (?<version>\S+)$'

# "chore(deps): bump serde from 1.0.1 to 1.0.2" -> scope: deps, package: serde
prism --column-regex '^\w+\((?<scope>[^)]+)\): bump (?<package>\S+)'
```

Set `PRISM_COLUMN_REGEX` in your shell profile, or `column_regex` in the config file, to use one
every time. Press `c` in the program to edit it while running.

## The screen

The list of PRs sits beside panes for the highlighted PR's checks, comments (with reviews and
review threads), description, approvers and diff. Click a column header, or press `s` / `S`, to
sort the list. `v` chooses which columns show.

- `1`-`5` hide or show a pane and `0` shows them all; the others take the room.
- `+` `-` `>` `<` resize the focused pane, as does dragging the borders with the mouse; `=` resets.
- The mouse clicks, scrolls and drags the scrollbars. Drag across text in the description, a
  comment or the diff to copy it. Use `--no-mouse` if you'd rather have your terminal's own
  selection.

## Shortcuts

Press `?` in the program for the full list, which also covers the keys below that are particular
to one pane.

| Key | Action |
|---|---|
| `/` | Edit the GitHub search query. Enter searches, Esc cancels. |
| `f` | Filter the list as you type. Enter keeps the filter, Esc clears it. |
| `s` / `S` | Sort by the next column / reverse the sort. |
| `c` | Edit the columns regex. |
| `v` | Choose which columns to show. |
| `1`-`5`, `0` | Hide or show a pane, or show them all. |
| `+` `-` `>` `<` `=` | Resize the focused pane, or reset the sizes. |
| `e` | Show the whole message of a failed fetch. |
| `Tab` / `Shift-Tab` | Move focus between the panes. |
| `Up` `Down` / `j` `k` | Move the highlight, or scroll the pane. |
| `PgUp` `PgDn` | Scroll the pane a page, or move the list's highlight a page. |
| `Home` `End` | Jump to the first or last item, or the top or bottom of a pane. |
| `Left` `Right` / `h` `l` | Scroll the tables in a description or comment sideways. |
| `Space` | Toggle selection of the highlighted PR. |
| `Ctrl-A` | Select every visible PR with passing checks. |
| `Enter` | Act on the highlighted item: review the PR in the table, expand or collapse a comment, open a check. Nothing in the description and diff. |
| `o` | Open the highlighted item: the PR in the table, a check or a comment on GitHub. Nothing in the description and diff. |
| `y` | Copy the URL of the highlighted item (the PR's in the table) to the clipboard. Works over SSH if the terminal supports OSC 52; in tmux, turn on `set-clipboard`. |
| `Ctrl-G` | Review the selected PRs, from any pane. `Ctrl-Enter` does the same in terminals with the kitty keyboard protocol. |
| `Ctrl-O` | Open the selected PRs in the browser, from any pane. Asks first for more than five. |
| `Ctrl-R` | Run the query again, keeping the selection. |
| `p` | Pause or resume the timed refresh. It also pauses while the terminal is out of focus, if the terminal reports that, and catches up when focus returns. |
| `Esc` | Clear selected text, then the filter, then the selection. |
| `q` | Quit, close a dialog, or cancel a running batch. |
| `Ctrl-C` | Quit from anywhere. |
| `?` | Show every key. |

## Developing

```sh
cargo test
cargo clippy --all-targets
```

The tests never call `gh`.
