mod cli;
mod query;

use std::io::ErrorKind;
use std::process::{Command, Stdio};

use clap::Parser;
use color_eyre::eyre::{Result, bail};

fn main() -> Result<()> {
    color_eyre::install()?;
    let args = cli::Args::parse();
    check_gh_installed()?;
    let query = query::initial(args.author.as_deref(), &args.search);
    println!("{query}");
    Ok(())
}

fn check_gh_installed() -> Result<()> {
    match Command::new("gh")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => bail!("gh is not installed."),
        Err(e) => Err(e.into()),
    }
}
