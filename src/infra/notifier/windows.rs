use anyhow::Result;

pub fn check_health() -> Result<()> {
    Ok(())
}

pub fn notify(title: &str, body: &str) -> Result<()> {
    winrt_notification::Toast::new(winrt_notification::Toast::POWERSHELL_APP_ID)
        .title(title)
        .text1(body)
        .show()?;
    Ok(())
}
