use anyhow::Result;

pub fn check_health() -> Result<()> {
    Ok(())
}

pub fn notify(title: &str, body: &str) -> Result<()> {
    mac_notification_sys::send_notification(title, None, body, None)?;
    Ok(())
}
