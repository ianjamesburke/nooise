use std::fmt::Display;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use update_informer::{Check, registry};

const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24);
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(2);
const EXPLICIT_UPDATE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Default)]
pub(crate) struct UpdateNotice {
    message: Arc<Mutex<Option<String>>>,
}

impl UpdateNotice {
    pub(crate) fn message(&self) -> Option<String> {
        self.message.lock().ok().and_then(|message| message.clone())
    }

    fn set_message(&self, message: String) {
        if let Ok(mut current) = self.message.lock() {
            *current = Some(message);
        }
    }
}

pub(crate) fn spawn_update_check(notice: UpdateNotice) {
    thread::spawn(move || {
        if let Ok(Some(version)) = check_latest(UPDATE_CHECK_INTERVAL, UPDATE_CHECK_TIMEOUT) {
            notice.set_message(format_update_message(version));
        }
    });
}

pub(crate) fn check_for_update() -> update_informer::Result<Option<update_informer::Version>> {
    check_latest(Duration::ZERO, EXPLICIT_UPDATE_TIMEOUT)
}

pub(crate) fn format_update_message(version: impl Display) -> String {
    format!("update {version} available: nooise update")
}

fn check_latest(
    interval: Duration,
    timeout: Duration,
) -> update_informer::Result<Option<update_informer::Version>> {
    update_informer::new(
        registry::Crates,
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
    )
    .interval(interval)
    .timeout(timeout)
    .check_version()
}

#[cfg(test)]
mod tests {
    use super::{UpdateNotice, format_update_message};

    #[test]
    fn update_message_names_command() {
        assert_eq!(
            format_update_message("v1.2.3"),
            "update v1.2.3 available: nooise update"
        );
    }

    #[test]
    fn notice_starts_empty_and_stores_message() {
        let notice = UpdateNotice::default();
        assert_eq!(notice.message(), None);

        notice.set_message("update v1.2.3 available: nooise update".to_string());
        assert_eq!(
            notice.message(),
            Some("update v1.2.3 available: nooise update".to_string())
        );
    }
}
