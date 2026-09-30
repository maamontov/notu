mod app;
mod config;
mod recovery;
mod store;

use std::{
    io::{self, IsTerminal},
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste},
    execute,
};

#[derive(Parser)]
#[command(version, about = "Fast Markdown notes in your terminal")]
struct Args {
    /// Notes directory (default: your Documents/notu)
    #[arg(short, long, env = "NOTU_WORKSPACE")]
    workspace: Option<PathBuf>,
    /// Create or open today's note immediately
    #[arg(short, long)]
    today: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("notu needs an interactive terminal. Run notu --help for usage.");
    }
    let store = store::Workspace::open(args.workspace)?;
    let mut app = app::App::new(store, args.today)?;
    let mut terminal = ratatui::try_init().context("Cannot initialize terminal")?;
    let result = (|| -> Result<()> {
        execute!(io::stdout(), EnableBracketedPaste)?;
        loop {
            terminal.draw(|frame| app.draw(frame))?;
            // Block until an event arrives: no polling or idle redraws.
            if app.event(event::read()?) {
                break;
            }
        }
        Ok(())
    })();
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}
