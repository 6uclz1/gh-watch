#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct PollExecutionState {
    poll_requested: bool,
    in_flight: bool,
    queued_refresh: bool,
}

impl PollExecutionState {
    pub(super) fn request_poll(&mut self) -> bool {
        if self.in_flight {
            self.queued_refresh = true;
            return false;
        }

        if self.poll_requested {
            return false;
        }

        self.poll_requested = true;
        true
    }

    pub(super) fn start_poll(&mut self) -> bool {
        if self.in_flight || !self.poll_requested {
            return false;
        }

        self.poll_requested = false;
        self.in_flight = true;
        true
    }

    pub(super) fn finish_poll_and_take_next_request(&mut self) -> bool {
        self.in_flight = false;

        if self.queued_refresh {
            self.queued_refresh = false;
            self.poll_requested = true;
            return true;
        }

        false
    }

    pub(super) fn in_flight(&self) -> bool {
        self.in_flight
    }

    pub(super) fn queued_refresh(&self) -> bool {
        self.queued_refresh
    }
}

#[cfg(test)]
mod tests {
    use super::PollExecutionState;

    #[test]
    fn refresh_requested_while_polling_is_queued_without_parallel_start() {
        let mut state = PollExecutionState::default();

        assert!(state.request_poll());
        assert!(!state.request_poll());

        assert!(state.start_poll());
        assert!(state.in_flight());

        assert!(!state.request_poll());
        assert!(state.queued_refresh());

        let queued_for_next = state.finish_poll_and_take_next_request();
        assert!(queued_for_next);
        assert!(!state.in_flight());
        assert!(!state.queued_refresh());

        assert!(state.start_poll());
        assert!(state.in_flight());
    }
}
