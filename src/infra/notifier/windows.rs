use anyhow::Result;

use super::PlatformNotificationOptions;
use crate::ports::NotificationClickSupport;

pub const DEFAULT_APP_ID: &str = winrt_notification::Toast::POWERSHELL_APP_ID;

pub fn effective_app_id(configured: Option<&str>) -> String {
    configured
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_APP_ID)
        .to_string()
}

pub fn check_health(_options: &PlatformNotificationOptions) -> Result<()> {
    Ok(())
}

pub fn click_action_support() -> NotificationClickSupport {
    NotificationClickSupport::Unsupported
}

pub fn notify(
    title: &str,
    body: &str,
    _click_url: Option<&str>,
    options: &PlatformNotificationOptions,
) -> Result<()> {
    let app_id = effective_app_id(options.windows_app_id.as_deref());
    winrt_notification::Toast::new(&app_id)
        .title(title)
        .text1(body)
        .show()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{effective_app_id, DEFAULT_APP_ID};

    #[test]
    fn effective_app_id_uses_default_when_missing() {
        assert_eq!(effective_app_id(None), DEFAULT_APP_ID);
    }

    #[test]
    fn effective_app_id_uses_default_when_blank() {
        assert_eq!(effective_app_id(Some("  ")), DEFAULT_APP_ID);
    }

    #[test]
    fn effective_app_id_prefers_configured_value() {
        assert_eq!(
            effective_app_id(Some("com.example.CustomWinApp")),
            "com.example.CustomWinApp"
        );
    }
}
