use anyhow::Result;

use crate::{domain::events::WatchEvent, ports::NotifierPort};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub fn build_notification_body(event: &WatchEvent, include_url: bool) -> String {
    let mut lines = vec![format!("{} by @{}", event.title, event.actor)];
    if include_url {
        lines.push(event.url.clone());
    }
    lines.join("\n")
}

fn build_notification_title(event: &WatchEvent) -> String {
    format!("{} [{}]", event.repo, event.kind)
}

#[derive(Debug, Clone, Copy)]
pub struct DesktopNotifier;

impl NotifierPort for DesktopNotifier {
    fn check_health(&self) -> Result<()> {
        platform::check_health()
    }

    fn notify(&self, event: &WatchEvent, include_url: bool) -> Result<()> {
        let title = build_notification_title(event);
        let body = build_notification_body(event, include_url);
        platform::notify(&title, &body)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NoopNotifier;

impl NotifierPort for NoopNotifier {
    fn check_health(&self) -> Result<()> {
        Ok(())
    }

    fn notify(&self, _event: &WatchEvent, _include_url: bool) -> Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    pub use super::macos::{check_health, notify};
}

#[cfg(target_os = "linux")]
mod platform {
    pub use super::linux::{check_health, notify};
}

#[cfg(target_os = "windows")]
mod platform {
    pub use super::windows::{check_health, notify};
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod platform {
    use anyhow::Result;

    pub fn check_health() -> Result<()> {
        Ok(())
    }

    pub fn notify(_title: &str, _body: &str) -> Result<()> {
        Ok(())
    }
}
