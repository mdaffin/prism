mod app;
mod cli;
mod gh;
mod model;
mod query;
mod ui;

use clap::Parser;
use color_eyre::eyre::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

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
    let mut app = App::new(app::title(
        args.repo.as_deref(),
        args.author.as_deref(),
        &args.search,
        args.dry_run,
    ));
    // Background work reports back through this channel.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let query = query::initial(args.author.as_deref(), &args.search);
    let gh = gh::Gh::new(args.repo);
    let fetch_tx = tx.clone();
    let fetch_gh = gh.clone();
    tokio::spawn(async move {
        let prs = fetch_gh.list_prs(&query, args.limit).await;
        let _ = fetch_tx.send(Msg::PrsLoaded(prs));
    });

    let mut events = EventStream::new();
    while !app.should_quit {
        terminal.draw(|frame| ui::view(&mut app, frame))?;
        let msg = tokio::select! {
            Some(event) = events.next() => match event? {
                Event::Key(key) if key.kind == KeyEventKind::Press => Msg::Key(key),
                // Anything else, such as a resize, only needs a redraw.
                _ => continue,
            },
            Some(msg) = rx.recv() => msg,
            else => break,
        };
        app.update(msg);
        for number in app.details_to_fetch() {
            let (gh, tx) = (gh.clone(), tx.clone());
            tokio::spawn(async move {
                let details = gh.pr_details(number).await;
                let _ = tx.send(Msg::DetailsLoaded(number, details));
            });
        }
    }
    Ok(())
}
