mod app;
mod cli;
mod gh;
mod model;
mod query;
mod ui;

use std::time::Duration;

use clap::Parser;
use color_eyre::eyre::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::time::MissedTickBehavior;

use app::{App, Msg};

#[tokio::main]
async fn main() -> Result<()> {
    // Installed before ratatui::init so the terminal is restored before a report is printed.
    color_eyre::install()?;
    let args = cli::Args::parse();
    gh::check_installed()?;

    let terminal = ratatui::init();
    let result = run(terminal, args).await;
    ratatui::restore();
    result
}

async fn run(mut terminal: DefaultTerminal, args: cli::Args) -> Result<()> {
    let title = app::title(args.repo.as_deref(), args.dry_run);
    let query = query::initial(args.author.as_deref(), &args.search);
    let mut app = App::new(title, query, args.dry_run);
    // Background work reports back through this channel.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let gh = gh::Gh::new(args.repo, args.dry_run);

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    // The ticker is only polled while animating; don't catch up on the ticks missed between.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    while !app.should_quit {
        // Start any work the last update asked for.
        if let Some((id, query)) = app.list_to_fetch() {
            let limit = args.limit;
            spawn(&gh, &tx, async move |gh| {
                Msg::PrsLoaded(id, gh.list_prs(&query, limit).await)
            });
        }
        for number in app.details_to_fetch() {
            spawn(&gh, &tx, async move |gh| {
                Msg::DetailsLoaded(number, gh.pr_details(number).await)
            });
        }
        for number in app.diffs_to_fetch() {
            spawn(&gh, &tx, async move |gh| {
                Msg::DiffLoaded(number, gh.pr_diff(number).await)
            });
        }
        for number in app.refreshes_to_run() {
            spawn(&gh, &tx, async move |gh| {
                Msg::PrRefreshed(number, gh.pr(number).await)
            });
        }
        for number in app.approvals_to_run() {
            spawn(&gh, &tx, async move |gh| {
                Msg::Approved(number, gh.approve(number).await)
            });
        }
        if let Some((number, auto)) = app.merge_to_run() {
            let method = args.merge_method;
            spawn(&gh, &tx, async move |gh| {
                Msg::Merged(number, gh.merge(number, auto, method).await)
            });
        }

        terminal.draw(|frame| ui::view(&mut app, frame))?;
        let msg = tokio::select! {
            Some(event) = events.next() => match event? {
                Event::Key(key) if key.kind == KeyEventKind::Press => Msg::Key(key),
                // Anything else, such as a resize, only needs a redraw.
                _ => continue,
            },
            Some(msg) = rx.recv() => msg,
            _ = ticker.tick(), if app.animating() => Msg::Tick,
            else => break,
        };
        app.update(msg);
    }
    Ok(())
}

/// Runs `work` in the background with its own handle to `gh`, and sends the message it returns.
fn spawn<F>(gh: &gh::Gh, tx: &UnboundedSender<Msg>, work: impl FnOnce(gh::Gh) -> F + Send + 'static)
where
    F: Future<Output = Msg> + Send + 'static,
{
    let (gh, tx) = (gh.clone(), tx.clone());
    tokio::spawn(async move {
        let _ = tx.send(work(gh).await);
    });
}
