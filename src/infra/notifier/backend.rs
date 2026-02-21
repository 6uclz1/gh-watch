#[cfg(target_os = "linux")]
use super::wsl_burnttoast::{probe_burnttoast_available, read_proc_wsl_hint};

#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(super) const NON_MACOS_NOOP_WARNING: &str =
    "desktop notifications are supported on macOS and WSL only; using noop notifier";
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(super) const WSL_BURNTTOAST_UNAVAILABLE_WARNING: &str =
    "WSL detected but BurntToast is unavailable via powershell.exe; using noop notifier";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) enum DesktopBackendKind {
    MacOs,
    WslBurntToast,
    Noop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(super) struct LinuxBackendSelection {
    pub(super) kind: DesktopBackendKind,
    pub(super) startup_warning: Option<String>,
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(super) fn is_wsl_from_inputs(
    wsl_distro_name: Option<&str>,
    wsl_interop: Option<&str>,
    proc_hint: Option<&str>,
) -> bool {
    if wsl_distro_name.is_some_and(|value| !value.trim().is_empty()) {
        return true;
    }
    if wsl_interop.is_some_and(|value| !value.trim().is_empty()) {
        return true;
    }
    proc_hint
        .map(|value| value.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(super) fn select_linux_backend(is_wsl: bool, burnttoast_ok: bool) -> LinuxBackendSelection {
    if !is_wsl {
        return LinuxBackendSelection {
            kind: DesktopBackendKind::Noop,
            startup_warning: Some(NON_MACOS_NOOP_WARNING.to_string()),
        };
    }

    if burnttoast_ok {
        LinuxBackendSelection {
            kind: DesktopBackendKind::WslBurntToast,
            startup_warning: None,
        }
    } else {
        LinuxBackendSelection {
            kind: DesktopBackendKind::Noop,
            startup_warning: Some(WSL_BURNTTOAST_UNAVAILABLE_WARNING.to_string()),
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn detect_linux_backend() -> LinuxBackendSelection {
    let distro_name = std::env::var("WSL_DISTRO_NAME").ok();
    let interop = std::env::var("WSL_INTEROP").ok();
    let proc_hint = read_proc_wsl_hint();
    let is_wsl = is_wsl_from_inputs(
        distro_name.as_deref(),
        interop.as_deref(),
        proc_hint.as_deref(),
    );
    let burnttoast_ok = if is_wsl {
        probe_burnttoast_available()
    } else {
        false
    };
    select_linux_backend(is_wsl, burnttoast_ok)
}

#[cfg(test)]
mod tests {
    use super::{is_wsl_from_inputs, select_linux_backend, DesktopBackendKind};

    #[test]
    fn wsl_detection_true_when_wsl_distro_name_exists() {
        assert!(is_wsl_from_inputs(
            Some("Ubuntu"),
            None,
            Some("Linux version 6.6.87.2-microsoft-standard-WSL2")
        ));
    }

    #[test]
    fn wsl_detection_true_when_wsl_interop_exists() {
        assert!(is_wsl_from_inputs(
            None,
            Some("/run/WSL/123_interop"),
            Some("Linux version 6.6.87.2-microsoft-standard-WSL2")
        ));
    }

    #[test]
    fn wsl_detection_true_when_proc_contains_microsoft() {
        assert!(is_wsl_from_inputs(
            None,
            None,
            Some("Linux version 6.6.87.2-microsoft-standard-WSL2")
        ));
    }

    #[test]
    fn wsl_detection_false_when_all_signals_absent() {
        assert!(!is_wsl_from_inputs(
            None,
            None,
            Some("Linux version 6.6.87.2-generic")
        ));
    }

    #[test]
    fn linux_backend_non_wsl_falls_back_to_noop_with_warning() {
        let selected = select_linux_backend(false, false);
        assert_eq!(selected.kind, DesktopBackendKind::Noop);
        let warning = selected.startup_warning.expect("warning should exist");
        assert!(warning.contains("macOS and WSL"));
    }

    #[test]
    fn linux_backend_wsl_with_burnttoast_selects_wsl_backend() {
        let selected = select_linux_backend(true, true);
        assert_eq!(selected.kind, DesktopBackendKind::WslBurntToast);
        assert!(selected.startup_warning.is_none());
    }

    #[test]
    fn linux_backend_wsl_without_burnttoast_falls_back_to_noop_with_warning() {
        let selected = select_linux_backend(true, false);
        assert_eq!(selected.kind, DesktopBackendKind::Noop);
        let warning = selected.startup_warning.expect("warning should exist");
        assert!(warning.contains("BurntToast"));
    }
}
