mod cli;
mod gh;
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
    for pr in prs {
        println!("#{} {:?} {}", pr.number, pr.state, pr.title);
    }
    Ok(())
}
