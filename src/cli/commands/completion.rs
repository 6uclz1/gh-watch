use std::io;

use anyhow::Result;
use clap::CommandFactory;
use clap_complete::{generate, Shell};

use crate::cli::args::{Cli, CompletionShell};

pub(crate) fn run(shell: CompletionShell) -> Result<()> {
    let mut cmd = Cli::command();
    generate(
        to_clap_shell(shell),
        &mut cmd,
        "gh-watch",
        &mut io::stdout(),
    );
    Ok(())
}

fn to_clap_shell(shell: CompletionShell) -> Shell {
    match shell {
        CompletionShell::Bash => Shell::Bash,
        CompletionShell::Zsh => Shell::Zsh,
        CompletionShell::Fish => Shell::Fish,
        CompletionShell::Pwsh => Shell::PowerShell,
    }
}
