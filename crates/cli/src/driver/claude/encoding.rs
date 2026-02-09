// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::Duration;

use crate::driver::{NudgeEncoder, NudgeStep, QuestionAnswer, RespondEncoder};

/// Encodes nudge messages for Claude Code's terminal input.
pub struct ClaudeNudgeEncoder;

impl NudgeEncoder for ClaudeNudgeEncoder {
    fn encode(&self, message: &str) -> Vec<NudgeStep> {
        vec![NudgeStep {
            bytes: format!("{message}\r").into_bytes(),
            delay_after: None,
        }]
    }
}

/// Encodes prompt responses for Claude Code's terminal input.
pub struct ClaudeRespondEncoder {
    pub feedback_delay: Duration,
    /// Delay between keystrokes in multi-question sequences.
    pub input_delay: Duration,
}

impl Default for ClaudeRespondEncoder {
    fn default() -> Self {
        Self {
            feedback_delay: Duration::from_millis(100),
            input_delay: Duration::from_millis(100),
        }
    }
}

impl RespondEncoder for ClaudeRespondEncoder {
    fn encode_permission(&self, accept: bool) -> Vec<NudgeStep> {
        let key = if accept { "y" } else { "n" };
        vec![NudgeStep {
            bytes: format!("{key}\r").into_bytes(),
            delay_after: None,
        }]
    }

    fn encode_plan(&self, accept: bool, feedback: Option<&str>) -> Vec<NudgeStep> {
        if accept {
            return vec![NudgeStep {
                bytes: b"y\r".to_vec(),
                delay_after: None,
            }];
        }

        let mut steps = vec![NudgeStep {
            bytes: b"n\r".to_vec(),
            delay_after: feedback.map(|_| self.feedback_delay),
        }];

        if let Some(text) = feedback {
            steps.push(NudgeStep {
                bytes: format!("{text}\r").into_bytes(),
                delay_after: None,
            });
        }

        steps
    }

    fn encode_question(
        &self,
        answers: &[QuestionAnswer],
        total_questions: usize,
    ) -> Vec<NudgeStep> {
        if answers.is_empty() {
            return vec![];
        }

        // All-at-once: multiple answers → emit each answer with delay, then confirm.
        if answers.len() > 1 {
            let mut steps = Vec::new();
            for answer in answers {
                let step = self.encode_single_answer(answer);
                steps.push(NudgeStep {
                    bytes: step,
                    delay_after: Some(self.input_delay),
                });
            }
            // Final confirm (Enter on the confirm tab).
            steps.push(NudgeStep {
                bytes: b"\r".to_vec(),
                delay_after: None,
            });
            return steps;
        }

        // Single answer in a multi-question dialog: just emit the digit (TUI auto-advances).
        let answer = &answers[0];
        if total_questions > 1 {
            let bytes = self.encode_single_answer(answer);
            return vec![NudgeStep {
                bytes,
                delay_after: None,
            }];
        }

        // Single-question dialog: emit answer + confirm.
        let bytes = self.encode_single_answer(answer);
        vec![NudgeStep {
            bytes: [&bytes[..], b"\r"].concat(),
            delay_after: None,
        }]
    }
}

impl ClaudeRespondEncoder {
    fn encode_single_answer(&self, answer: &QuestionAnswer) -> Vec<u8> {
        if let Some(n) = answer.option {
            return format!("{n}").into_bytes();
        }
        if let Some(ref text) = answer.text {
            return format!("{text}\r").into_bytes();
        }
        vec![]
    }
}

#[cfg(test)]
#[path = "encoding_tests.rs"]
mod tests;
