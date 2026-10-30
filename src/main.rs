mod cli;
mod gh;
mod model;
mod query;

use clap::Parser;
use color_eyre::eyre::Result;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let args = cli::Args::parse();
    gh::check_installed()?;
    let query = query::initial(args.author.as_deref(), &args.search);

    let gh = gh::Gh::new(args.repo);
    let prs = gh.list_prs(&query, args.limit).await?;
    if prs.is_empty() {
        println!("No matching PRs found.");
    }
    let mut rows: Vec<_> = prs
        .into_iter()
        .map(|pr| {
            let checks = model::classify_checks(&pr.mergeable, &pr.status_check_rollup);
            (pr, checks)
        })
        .collect();
    // Passing first.
    rows.sort_by_key(|(_, checks)| checks.state.rank());
    for (pr, checks) in rows {
        let (ticket, title) = model::split_ticket(&pr.title);
        println!(
            "#{} {:?}  {}  {}  {}  {}",
            pr.number,
            pr.state,
            checks.state.label(),
            model::review_label(&pr.review_decision),
            ticket,
            title,
        );
    }
    Ok(())
}
