//! Follows the setting `agent.mcp_http` for the desktop's MCP server over HTTP.
//!
//! The setting is the person's switch (an agent cannot write it). The launcher may also start the server with
//! `--mcp-http`; that is an override for one launch and does not touch the setting. So the switch is followed on
//! change, never as a level: turning it on starts the server, turning it off stops it, and a launch with the
//! switch off does not stop a server the flag started.

use drafftink_ui::AgentServerView;

use crate::platform::{McpHttpControl, McpHttpInfo};

/// The server and the last value of the switch it followed.
pub(crate) struct McpHttpState {
    control: Box<dyn McpHttpControl>,
    followed: Option<bool>,
    problem: Option<String>,
}

impl McpHttpState {
    pub(crate) fn new(control: Box<dyn McpHttpControl>) -> Self {
        Self { control, followed: None, problem: None }
    }

    /// Follows the switch. Returns the view for the preferences when something changed (always on the first call).
    pub(crate) fn follow(&mut self, want: bool) -> Option<AgentServerView> {
        let first = self.followed.is_none();
        if self.followed == Some(want) {
            return None;
        }
        self.followed = Some(want);
        if want {
            self.problem = self.control.start().err();
        } else if !first {
            self.control.stop();
            self.problem = None;
        }
        Some(view(self.control.running(), self.problem.clone()))
    }
}

fn view(running: Option<McpHttpInfo>, problem: Option<String>) -> AgentServerView {
    AgentServerView {
        endpoint: running.as_ref().map(|r| r.endpoint.clone()),
        token_file: running.and_then(|r| r.token_file),
        problem,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// Records what the app asked of the server.
    #[derive(Default)]
    struct Fake {
        calls: Rc<RefCell<Vec<&'static str>>>,
        up: bool,
        fail: bool,
    }

    impl McpHttpControl for Fake {
        fn start(&mut self) -> Result<McpHttpInfo, String> {
            self.calls.borrow_mut().push("start");
            if self.fail {
                return Err("Port 3737 is in use.".to_owned());
            }
            self.up = true;
            Ok(info())
        }
        fn stop(&mut self) {
            self.calls.borrow_mut().push("stop");
            self.up = false;
        }
        fn running(&self) -> Option<McpHttpInfo> {
            self.up.then(info)
        }
    }

    fn info() -> McpHttpInfo {
        McpHttpInfo { endpoint: "http://127.0.0.1:3737/mcp".to_owned(), token_file: Some("/t/3737.token".to_owned()) }
    }

    fn state(fake: Fake) -> (McpHttpState, Rc<RefCell<Vec<&'static str>>>) {
        let calls = fake.calls.clone();
        (McpHttpState::new(Box::new(fake)), calls)
    }

    #[test]
    fn the_switch_off_at_launch_starts_nothing_and_stops_nothing() {
        let (mut s, calls) = state(Fake::default());
        let v = s.follow(false).unwrap();
        assert_eq!(v, AgentServerView::default());
        assert!(s.follow(false).is_none(), "no change, no work");
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn a_server_started_by_the_flag_survives_a_switch_that_is_off() {
        let (mut s, calls) = state(Fake { up: true, ..Fake::default() });
        let v = s.follow(false).unwrap();
        assert_eq!(v.endpoint.as_deref(), Some("http://127.0.0.1:3737/mcp"));
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn turning_the_switch_on_and_off_starts_and_stops() {
        let (mut s, calls) = state(Fake::default());
        s.follow(false);
        let on = s.follow(true).unwrap();
        assert_eq!(on.token_file.as_deref(), Some("/t/3737.token"));
        let off = s.follow(false).unwrap();
        assert_eq!(off, AgentServerView::default());
        assert_eq!(*calls.borrow(), ["start", "stop"]);
    }

    #[test]
    fn the_switch_on_at_launch_starts_the_server() {
        let (mut s, calls) = state(Fake::default());
        assert!(s.follow(true).unwrap().endpoint.is_some());
        assert_eq!(*calls.borrow(), ["start"]);
    }

    #[test]
    fn a_failed_start_is_shown_in_words_and_off_clears_it() {
        let (mut s, _) = state(Fake { fail: true, ..Fake::default() });
        let v = s.follow(true).unwrap();
        assert_eq!((v.endpoint, v.problem.as_deref()), (None, Some("Port 3737 is in use.")));
        assert_eq!(s.follow(false).unwrap().problem, None);
    }
}
