use anyhow::Result;

pub fn check_health() -> Result<()> {
    Ok(())
}

pub fn notify(title: &str, body: &str) -> Result<()> {
    notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .show()?;
    Ok(())
}
