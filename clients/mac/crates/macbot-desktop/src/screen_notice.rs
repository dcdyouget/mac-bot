#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ScreenStatus {
    Error,
    Closed,
}

#[derive(Default)]
pub(super) struct ScreenNotice {
    generation: u64,
    status: Option<ScreenStatus>,
}

impl ScreenNotice {
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn close(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.status = None;
    }

    pub(super) fn is_current(&self, generation: u64, page: &str) -> bool {
        generation == self.generation && page == "computer"
    }

    pub(super) fn record(&mut self, generation: u64, status: ScreenStatus) {
        if generation == self.generation {
            self.status = Some(status);
        }
    }

    pub(super) fn visible_key(&self, page: &str) -> Option<&'static str> {
        if page != "computer" {
            return None;
        }
        self.status.map(|status| match status {
            ScreenStatus::Error => "computer.connection_error",
            ScreenStatus::Closed => "computer.disconnected",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_status_is_visible_only_on_computer() {
        let mut notice = ScreenNotice::default();
        notice.record(notice.generation(), ScreenStatus::Error);
        assert_eq!(
            notice.visible_key("computer"),
            Some("computer.connection_error")
        );
        for page in ["search", "chat", "dashboard", "connect"] {
            assert_eq!(notice.visible_key(page), None);
            assert!(!notice.is_current(notice.generation(), page));
        }
        notice.record(notice.generation(), ScreenStatus::Closed);
        assert_eq!(
            notice.visible_key("computer"),
            Some("computer.disconnected")
        );
    }

    #[test]
    fn closing_clears_status_and_rejects_previous_session_events() {
        let mut notice = ScreenNotice::default();
        let old_session = notice.generation();
        notice.record(old_session, ScreenStatus::Error);
        notice.close();
        assert_eq!(notice.visible_key("computer"), None);
        assert!(!notice.is_current(old_session, "computer"));
        notice.record(old_session, ScreenStatus::Error);
        assert_eq!(notice.visible_key("computer"), None);
        let current_session = notice.generation();
        assert!(notice.is_current(current_session, "computer"));
        notice.record(current_session, ScreenStatus::Closed);
        assert_eq!(
            notice.visible_key("computer"),
            Some("computer.disconnected")
        );
    }
}
