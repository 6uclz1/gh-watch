use anyhow::Result;

use crate::ports::NotificationClickSupport;

pub fn check_health() -> Result<()> {
    Ok(())
}

pub fn click_action_support() -> NotificationClickSupport {
    NotificationClickSupport::Supported
}

pub fn notify(title: &str, body: &str, click_url: Option<&str>) -> Result<()> {
    if let Some(url) = click_url {
        let title = title.to_string();
        let body = body.to_string();
        let url = url.to_string();
        std::thread::spawn(move || {
            let mut notification = mac_notification_sys::Notification::new();
            notification
                .title(&title)
                .message(&body)
                .wait_for_click(true);
            if let Ok(mac_notification_sys::NotificationResponse::Click) = notification.send() {
                let _ = std::process::Command::new("open").arg(url).status();
            }
        });
        return Ok(());
    }

    mac_notification_sys::send_notification(title, None, body, None)?;
    Ok(())
}
