use anyhow::Result;

use crate::ports::NotificationClickSupport;

pub fn check_health() -> Result<()> {
    Ok(())
}

pub fn click_action_support() -> NotificationClickSupport {
    NotificationClickSupport::Unsupported
}

pub fn notify(title: &str, body: &str, _click_url: Option<&str>) -> Result<()> {
    winrt_notification::Toast::new(winrt_notification::Toast::POWERSHELL_APP_ID)
        .title(title)
        .text1(body)
        .show()?;
    Ok(())
}
