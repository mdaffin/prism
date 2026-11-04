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

`prism --help` lists the same options, and `--version` prints the version.

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
sort the list: by checks (the default), number, title, review, author or when it was updated. `v`
chooses which columns show, including Updated, which says how long ago each PR changed. `e` opens
the whole message of a fetch that failed, as the panes show only its first line.

- `1`-`5` hide or show a pane and `0` shows them all; the others take the room.
- `+` `-` `>` `<` resize the focused pane, as does dragging the borders with the mouse; `=` resets.
- The mouse clicks, scrolls and drags the scrollbars. Drag across text in the description, a
  comment or the diff to copy it. Shift-drag (Option-drag in iTerm2) uses your terminal's own
  selection instead, and `--no-mouse` turns the capture off altogether.

## Checks

The checks column and pane show a PR as passing, pending or failing the way GitHub's own page
does. A branch-protection check that is required but hasn't started yet counts as pending, as does
a workflow whose newest run was cancelled before reporting anything, even if an older run passed.

## Approving and merging

Select PRs with `Space` (or `Ctrl-A` for every visible PR with passing checks), then press `Ctrl-G`
(or `Enter` to review just the highlighted PR). A dialog lists the PRs and offers
**Approve**, **Approve & merge** or **Cancel**; closed and merged PRs that were selected are left
out.

- PRs are approved several at a time. With Approve & merge, each PR is merged, one at a time, as
  soon as it is approved, using `--merge-method`.
- A PR you have already approved at its current head isn't approved again; it goes straight on to
  merging.
- A PR whose checks are still pending is set to auto-merge, so GitHub merges it once they pass. A
  PR with no checks is merged straight away.
- Each PR shows its own status as the batch runs, and a summary follows. `q` cancels the batch; a
  step already under way can't be stopped, and PRs approved so far aren't merged.
- The selection is kept afterwards.
- `--dry-run` goes through all of this without approving or merging anything.

## Shortcuts

Press `?` in the program for the same list. Enter, `o` and `y` act on the highlighted item of the
focused pane; the Ctrl-keys work from any pane and act on the selected PRs.

<!-- keys:start -->
| Key | Where | Action |
|---|---|---|
| `?` | Everywhere | Show every key |
| `/` | Everywhere | Edit the GitHub search query |
| `f` | Everywhere | Filter the list as you type |
| `s` | Everywhere | Sort the list by the next column (checks, number, title, review, author, then when it was updated); the header shows which, and ▲ or ▼ which way |
| `S` | Everywhere | Reverse the sort |
| `c` | Everywhere | Edit the regex whose named groups become the table's columns |
| `v` | Everywhere | Choose which columns the table shows (Space hides or shows one, for the session) |
| `e` | Everywhere | Show the whole message of a failed fetch, which the panes cut to its first line |
| `1-5` | Everywhere | Hide or show the checks, comments, description, approvers or diff pane, for the session; the others take its room and Tab skips it |
| `0` | Everywhere | Show every pane |
| `+ / -` | Everywhere | Make the focused pane taller or shorter (or drag the border between two panes with the mouse) |
| `> / <` | Everywhere | Make the focused pane wider or narrower |
| `=` | Everywhere | Put every pane back to its usual size |
| `Tab / Shift-Tab` | Everywhere | Move focus between the panes |
| `Up/Down, j/k` | Everywhere | Move the highlight, or scroll the pane |
| `PgUp/PgDn` | Everywhere | Scroll the pane a page, or move the table's highlight a page; the comments' highlight stays |
| `Home/End` | Everywhere | Jump to the first or last item, or the top or bottom of a pane |
| `Space` | Everywhere | Toggle selection (not in the comments) |
| `Ctrl-O` | Everywhere | Open the selected PRs in the browser, asking first for more than five |
| `Ctrl-G` | Everywhere | Review the selected PRs |
| `Ctrl-Enter` | Everywhere | Like Ctrl-G, in a terminal with the kitty keyboard protocol (kitty, WezTerm, Ghostty, iTerm2); elsewhere it is Enter |
| `Ctrl-A` | Everywhere | Select every visible PR with passing checks |
| `Ctrl-R` | Everywhere | Run the query again, keeping the selection |
| `p` | Everywhere | Pause or resume the timed refresh, which also pauses while the terminal is out of focus |
| `Esc` | Everywhere | Clear selected text, then the filter, then the selection of PRs |
| `Mouse drag` | Everywhere | Select text in the description, an open comment or the diff and copy it when the button is released, or with y; Shift-drag (Option-drag in iTerm2) uses the terminal's own selection instead |
| `q` | Everywhere | Quit, close the current dialog, or cancel a running batch |
| `Ctrl-C` | Everywhere | Quit from anywhere |
| `Search box: Tab, Down` | Everywhere | Choose the next suggestion, the first to begin with; Enter takes the chosen one |
| `Search box: Shift-Tab, Up` | Everywhere | Choose the previous suggestion; without a list, Up goes through the queries searched before |
| `Enter` | Pull requests | Review the highlighted PR |
| `o` | Pull requests | Open the highlighted PR in the browser |
| `y` | Pull requests | Copy the highlighted PR's URL |
| `Left/Right, h/l` | Pull requests | Scroll the list sideways, keeping the number in view (also Shift-wheel) |
| `Enter, o` | Checks | Open the highlighted check's page |
| `y` | Checks | Copy the highlighted check's URL |
| `Left/Right, h/l` | Checks | Scroll the check names sideways (also Shift-wheel) |
| `Up/Down, j/k` | Comments | Go comment by comment, scrolling a line at a time until the next one is on screen |
| `Enter, Space` | Comments | Expand or collapse the highlighted comment |
| `o` | Comments | Open the highlighted comment, or its PR if it has no page |
| `y` | Comments | Copy the highlighted comment's URL, or its PR's if it has no page |
| `Left/Right, h/l` | Comments | Scroll the open comments' tables sideways (also Shift-wheel) |
| `Left/Right, h/l` | Description | Scroll the description's tables sideways (also Shift-wheel) |
| `Left/Right, h/l` | Diff | Scroll the diff sideways (also Shift-wheel) |
<!-- keys:end -->

`y` copies with an OSC 52 escape sequence, so it works over SSH if the terminal supports that; in
tmux, turn on `set-clipboard`. `Ctrl-Enter` needs a terminal with the kitty keyboard protocol;
elsewhere use `Ctrl-G`. The timed refresh pauses while the terminal is out of focus, if the
terminal reports that, and catches up when focus returns.

The table above is generated from the program's key list. After changing it, run
`UPDATE_README=1 cargo test` to rewrite it.

## Developing

```sh
cargo test
cargo clippy --all-targets
```

The tests never call `gh`.
