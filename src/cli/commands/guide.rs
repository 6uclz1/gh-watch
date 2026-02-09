use anyhow::Result;

const GUIDE: &str = "\
Core Commands
  gh-watch watch [--config <path>] [--interval-seconds <n>]
  gh-watch once [--config <path>] [--dry-run] [--json]
  gh-watch check [--config <path>]
  gh-watch init [--path <path>] [--force] [--reset-state]
  gh-watch config open
  gh-watch config path
  gh-watch commands
  gh-watch completion <shell>

Tab Completion
  Generate a shell completion script and load it in your shell.
  Example:
    gh-watch completion zsh > ~/.zfunc/_gh-watch
";

pub(crate) fn run() -> Result<()> {
    println!("{GUIDE}");
    Ok(())
}
