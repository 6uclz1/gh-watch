use anyhow::Result;

use crate::{
    config::NotificationConfig,
    ports::{
        NotificationClickSupport, NotificationDispatchResult, NotificationPayload, NotifierPort,
    },
};

#[cfg(target_os = "linux")]
use super::backend::detect_linux_backend;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use super::backend::NON_MACOS_NOOP_WARNING;
#[cfg(target_os = "macos")]
use super::macos_osascript::{check_osascript_available, notify_via_osascript};
#[cfg(target_os = "linux")]
use super::wsl_burnttoast::notify_via_burnttoast;
use super::{
    backend::DesktopBackendKind,
    message::{
        build_notification_body_from_payload, build_notification_title_from_payload,
        dispatch_result,
    },
};

#[derive(Debug, Clone)]
pub struct DesktopNotifier {
    backend: DesktopBackendKind,
    startup_warnings: Vec<String>,
}

impl Default for DesktopNotifier {
    fn default() -> Self {
        Self::from_notification_config(&NotificationConfig::default())
    }
}

impl DesktopNotifier {
    pub fn from_notification_config(_config: &NotificationConfig) -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                backend: DesktopBackendKind::MacOs,
                startup_warnings: Vec::new(),
            }
        }

        #[cfg(target_os = "linux")]
        {
            let selected = detect_linux_backend();
            let startup_warnings = selected
                .startup_warning
                .into_iter()
                .collect::<Vec<String>>();

            Self {
                backend: selected.kind,
                startup_warnings,
            }
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Self {
                backend: DesktopBackendKind::Noop,
                startup_warnings: vec![NON_MACOS_NOOP_WARNING.to_string()],
            }
        }
    }

    pub fn startup_warnings(&self) -> Vec<String> {
        self.startup_warnings.clone()
    }
}

impl NotifierPort for DesktopNotifier {
    fn check_health(&self) -> Result<()> {
        match self.backend {
            DesktopBackendKind::MacOs => {
                #[cfg(target_os = "macos")]
                {
                    check_osascript_available()
                }

                #[cfg(not(target_os = "macos"))]
                {
                    Ok(())
                }
            }
            DesktopBackendKind::WslBurntToast => Ok(()),
            DesktopBackendKind::Noop => Ok(()),
        }
    }

    fn click_action_support(&self) -> NotificationClickSupport {
        match self.backend {
            DesktopBackendKind::WslBurntToast
            | DesktopBackendKind::MacOs
            | DesktopBackendKind::Noop => NotificationClickSupport::Unsupported,
        }
    }

    fn notify(
        &self,
        payload: &NotificationPayload,
        include_url: bool,
    ) -> Result<NotificationDispatchResult> {
        let title = build_notification_title_from_payload(payload);
        let body = build_notification_body_from_payload(payload, include_url);

        match self.backend {
            DesktopBackendKind::MacOs => {
                #[cfg(target_os = "macos")]
                {
                    notify_via_osascript(&title, &body)?;
                    Ok(dispatch_result(include_url, false))
                }

                #[cfg(not(target_os = "macos"))]
                {
                    let _ = (&title, &body);
                    Ok(dispatch_result(include_url, false))
                }
            }
            DesktopBackendKind::WslBurntToast => {
                #[cfg(target_os = "linux")]
                {
                    notify_via_burnttoast(&title, &body)?;
                    Ok(dispatch_result(include_url, false))
                }

                #[cfg(not(target_os = "linux"))]
                {
                    let _ = (&title, &body);
                    Ok(dispatch_result(include_url, false))
                }
            }
            DesktopBackendKind::Noop => Ok(dispatch_result(include_url, false)),
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use crate::config::NotificationConfig;
    use crate::ports::{NotificationClickSupport, NotifierPort};

    use super::{DesktopBackendKind, DesktopNotifier};

    #[test]
    fn wsl_click_action_support_is_unsupported() {
        let notifier = DesktopNotifier {
            backend: DesktopBackendKind::WslBurntToast,
            startup_warnings: Vec::new(),
        };

        assert_eq!(
            notifier.click_action_support(),
            NotificationClickSupport::Unsupported
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn startup_warnings_macos_are_empty() {
        let notifier = DesktopNotifier::from_notification_config(&NotificationConfig::default());
        assert!(notifier.startup_warnings().is_empty());
    }
}
