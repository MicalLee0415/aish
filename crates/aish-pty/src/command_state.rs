use std::collections::HashMap;

use crate::control::BackendControlEvent;
use crate::types::{CommandSource, CommandSubmission, PtyCommandResult};

/// Tracks command lifecycle: register → command_started → prompt_ready.
pub struct CommandState {
    active_submission: Option<CommandSubmission>,
    submitted_by_seq: HashMap<i32, CommandSubmission>,
    last_command: String,
    last_exit_code: i32,
    last_result: Option<PtyCommandResult>,
    pending_error: Option<PtyCommandResult>,
}

impl CommandState {
    pub fn new() -> Self {
        Self {
            active_submission: None,
            submitted_by_seq: HashMap::new(),
            last_command: String::new(),
            last_exit_code: 0,
            last_result: None,
            pending_error: None,
        }
    }

    /// Register a command before it is sent to bash.
    pub fn register_command(
        &mut self,
        command: &str,
        source: CommandSource,
        command_seq: Option<i32>,
    ) {
        let command = command.trim().to_string();
        if command.is_empty() {
            return;
        }
        let submission = CommandSubmission::new(command, source, command_seq);
        self.active_submission = Some(submission.clone());
        if let Some(seq) = command_seq {
            self.submitted_by_seq.insert(seq, submission);
        }
    }

    /// Process a decoded backend control event, returning a PtyCommandResult
    /// when a command completes (prompt_ready).
    pub fn handle_event(&mut self, event: &BackendControlEvent) -> Option<PtyCommandResult> {
        match event {
            BackendControlEvent::CommandStarted {
                command_seq,
                command,
                ..
            } => {
                self.on_command_started(*command_seq, command);
                None
            }
            BackendControlEvent::PromptReady {
                command_seq,
                exit_code,
                cwd: _,
                interrupted,
            } => {
                let seq = *command_seq;
                let submission = self.take_submission(seq)?;
                if submission.command.is_empty() {
                    return None;
                }

                let interrupted = *interrupted || *exit_code == 130;
                let result = PtyCommandResult {
                    command: submission.command.clone(),
                    exit_code: *exit_code,
                    source: submission.source,
                    command_seq: seq,
                    interrupted,
                    allow_error_correction: submission.allow_error_correction,
                };

                self.last_command = result.command.clone();
                self.last_exit_code = result.exit_code;
                self.last_result = Some(result.clone());

                if result.allow_error_correction && result.exit_code != 0 && !result.interrupted {
                    self.pending_error = Some(result.clone());
                } else {
                    self.pending_error = None;
                }

                Some(result)
            }
            BackendControlEvent::SessionReady { .. } => None,
            BackendControlEvent::ShellExiting { .. } => None,
            BackendControlEvent::CommandOutput { .. } => None,
            BackendControlEvent::CompletionResult { .. } => None,
        }
    }

    pub fn last_command(&self) -> &str {
        &self.last_command
    }

    pub fn last_exit_code(&self) -> i32 {
        self.last_exit_code
    }

    pub fn last_result(&self) -> Option<&PtyCommandResult> {
        self.last_result.as_ref()
    }

    /// Whether the last completed user command should offer AI error correction.
    pub fn can_correct_error(&self) -> bool {
        self.last_result
            .as_ref()
            .map(|r| r.allow_error_correction && r.exit_code != 0 && !r.interrupted)
            .unwrap_or(false)
    }

    /// Consume the pending error if available.
    pub fn consume_error(&mut self) -> Option<(String, i32)> {
        self.pending_error.take().map(|r| (r.command, r.exit_code))
    }

    /// Clear any pending error-correction state.
    pub fn clear_error_correction(&mut self) {
        self.pending_error = None;
    }

    pub fn reset(&mut self) {
        self.active_submission = None;
        self.submitted_by_seq.clear();
        self.last_command.clear();
        self.last_exit_code = 0;
        self.last_result = None;
        self.pending_error = None;
    }

    fn on_command_started(&mut self, command_seq: Option<i32>, command: &str) {
        // Try to find a matching submission by seq first.
        if let Some(seq) = command_seq {
            if let Some(sub) = self.submitted_by_seq.get_mut(&seq) {
                if sub.command.is_empty() || sub.source != CommandSource::User {
                    sub.command = command.to_string();
                }
                self.active_submission = Some(sub.clone());
                return;
            }
        }

        // Check if the active submission matches.
        if let Some(sub) = self.active_submission.as_mut() {
            let active_seq = sub.command_seq;
            if command_seq.is_none() || active_seq.is_none() || active_seq == command_seq {
                if sub.command.is_empty() || sub.source != CommandSource::User {
                    sub.command = command.to_string();
                }
                return;
            }
        }

        // Create a new submission for unmatched backend events.
        let source = if command_seq.is_some() {
            CommandSource::Backend
        } else {
            CommandSource::User
        };
        let new_sub = CommandSubmission::new(command.to_string(), source, command_seq);
        self.active_submission = Some(new_sub.clone());
        if let Some(seq) = command_seq {
            self.submitted_by_seq.insert(seq, new_sub);
        }
    }

    fn take_submission(&mut self, command_seq: Option<i32>) -> Option<CommandSubmission> {
        if let Some(seq) = command_seq {
            if let Some(sub) = self.submitted_by_seq.remove(&seq) {
                if self.active_submission.as_ref().map(|s| s.command_seq) == Some(Some(seq)) {
                    self.active_submission = None;
                }
                return Some(sub);
            }
            return None;
        }
        // Issue #207 (defense in depth): a `PromptReady{command_seq:None}` event
        // must only match a submission that was itself registered without a
        // seq.  Before the fix, we would unconditionally take
        // `active_submission`, which could be a backend submission registered
        // with a seq — e.g. a freshly-registered `__aish_complete` — and
        // cause `query_completions` to return early with an empty candidate
        // list.  See `test_stale_prompt_ready_with_null_seq_does_not_consume_backend_submission`.
        if let Some(active) = self.active_submission.as_ref() {
            if active.command_seq.is_none() {
                return self.active_submission.take();
            }
        }
        None
    }
}

impl Default for CommandState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_complete_user_command() {
        let mut state = CommandState::new();
        state.register_command("ls", CommandSource::User, None);

        let evt = BackendControlEvent::PromptReady {
            command_seq: None,
            exit_code: 0,
            cwd: "/home".to_string(),
            interrupted: false,
        };
        let result = state.handle_event(&evt);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.command, "ls");
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.source, CommandSource::User);
        assert!(!r.interrupted);
    }

    #[test]
    fn test_backend_command_with_seq() {
        let mut state = CommandState::new();
        state.register_command("pwd", CommandSource::Backend, Some(-1));

        let started = BackendControlEvent::CommandStarted {
            command_seq: Some(-1),
            command: "pwd".to_string(),
            cwd: "/home".to_string(),
        };
        state.handle_event(&started);

        let ready = BackendControlEvent::PromptReady {
            command_seq: Some(-1),
            exit_code: 0,
            cwd: "/home".to_string(),
            interrupted: false,
        };
        let result = state.handle_event(&ready);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.command, "pwd");
        assert_eq!(r.command_seq, Some(-1));
        assert!(!r.allow_error_correction);
    }

    #[test]
    fn test_error_correction_user_fail() {
        let mut state = CommandState::new();
        state.register_command("bad_cmd", CommandSource::User, None);

        let evt = BackendControlEvent::PromptReady {
            command_seq: None,
            exit_code: 127,
            cwd: "/home".to_string(),
            interrupted: false,
        };
        state.handle_event(&evt);
        assert!(state.can_correct_error());
        let error = state.consume_error();
        assert!(error.is_some());
        assert_eq!(error.unwrap().0, "bad_cmd");
    }

    #[test]
    fn test_error_correction_interrupted() {
        let mut state = CommandState::new();
        state.register_command("sleep 10", CommandSource::User, None);

        let evt = BackendControlEvent::PromptReady {
            command_seq: None,
            exit_code: 130,
            cwd: "/home".to_string(),
            interrupted: false,
        };
        state.handle_event(&evt);
        // exit_code 130 counts as interrupted
        assert!(!state.can_correct_error());
    }

    #[test]
    fn test_error_correction_backend_no() {
        let mut state = CommandState::new();
        state.register_command("bad_cmd", CommandSource::Backend, Some(-1));

        let evt = BackendControlEvent::PromptReady {
            command_seq: Some(-1),
            exit_code: 1,
            cwd: "/home".to_string(),
            interrupted: false,
        };
        state.handle_event(&evt);
        assert!(!state.can_correct_error());
    }

    #[test]
    fn test_reset() {
        let mut state = CommandState::new();
        state.register_command("ls", CommandSource::User, None);
        state.reset();
        assert!(state.last_command().is_empty());
        assert_eq!(state.last_exit_code(), 0);
    }

    #[test]
    fn test_session_command_no_error_correction() {
        let mut state = CommandState::new();
        state.register_command("ssh user@host", CommandSource::User, None);

        let evt = BackendControlEvent::PromptReady {
            command_seq: None,
            exit_code: 1,
            cwd: "/home".to_string(),
            interrupted: false,
        };
        state.handle_event(&evt);
        // ssh is a session command — no error correction
        assert!(!state.can_correct_error());
    }

    /// Regression test for issue #207: a stale `PromptReady` with
    /// `command_seq: None` (e.g. lingering from `forward_readline_tab`'s
    /// `set -o emacs` / `set +o emacs; set +o vi` probes) must NOT consume
    /// a backend command submission that was registered with a seq.
    ///
    /// Before the fix, `take_submission(None)` would always fall through
    /// to `self.active_submission.take()`, stealing the backend
    /// submission and causing `query_completions` to return an empty
    /// candidate list (or even return prematurely with the wrong seq).
    #[test]
    fn test_stale_prompt_ready_with_null_seq_does_not_consume_backend_submission() {
        let mut state = CommandState::new();
        // Simulate `__aish_complete` registering a backend submission with seq=-5.
        state.register_command("__aish_complete", CommandSource::Backend, Some(-5));

        // Stale event from `set +o emacs; set +o vi` cleanup (no seq).
        let stale = BackendControlEvent::PromptReady {
            command_seq: None,
            exit_code: 0,
            cwd: "/home".to_string(),
            interrupted: false,
        };
        let result = state.handle_event(&stale);
        assert!(
            result.is_none(),
            "stale PromptReady{{command_seq:None}} must not match a backend submission \
             registered with a seq; got {result:?}"
        );

        // The real completion result for the backend submission should still match.
        let real = BackendControlEvent::PromptReady {
            command_seq: Some(-5),
            exit_code: 0,
            cwd: "/home".to_string(),
            interrupted: false,
        };
        let result = state.handle_event(&real);
        assert!(
            result.is_some(),
            "real PromptReady{{command_seq:Some(-5)}} must still match the backend submission"
        );
        let r = result.unwrap();
        assert_eq!(r.command_seq, Some(-5));
        assert_eq!(r.command, "__aish_complete");
    }

    /// When the *active* submission is itself registered without a seq
    /// (e.g. a user command), the `PromptReady{null}` event should still
    /// match — this preserves the existing user-command path.
    #[test]
    fn test_prompt_ready_with_null_seq_still_matches_user_submission() {
        let mut state = CommandState::new();
        state.register_command("ls", CommandSource::User, None);

        let evt = BackendControlEvent::PromptReady {
            command_seq: None,
            exit_code: 0,
            cwd: "/home".to_string(),
            interrupted: false,
        };
        let result = state.handle_event(&evt);
        assert!(
            result.is_some(),
            "PromptReady{{null}} must still match a user submission"
        );
        assert_eq!(result.unwrap().command, "ls");
    }
}
