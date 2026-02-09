// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::Duration;

use crate::driver::{NudgeEncoder, NudgeStep, QuestionAnswer, RespondEncoder};

/// Encodes nudge messages for Claude Code's terminal input.
pub struct ClaudeNudgeEncoder {
    /// Delay between typing the message and pressing enter to send.
    pub keyboard_delay: Duration,
}

impl NudgeEncoder for ClaudeNudgeEncoder {
    fn encode(&self, message: &str) -> Vec<NudgeStep> {
        vec![
            NudgeStep {
                bytes: message.as_bytes().to_vec(),
                delay_after: Some(self.keyboard_delay),
            },
            NudgeStep { bytes: b"\r".to_vec(), delay_after: None },
        ]
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
        Self { feedback_delay: Duration::from_millis(100), input_delay: Duration::from_millis(100) }
    }
}

impl RespondEncoder for ClaudeRespondEncoder {
    fn encode_permission(&self, option: u32) -> Vec<NudgeStep> {
        vec![NudgeStep { bytes: format!("{option}\r").into_bytes(), delay_after: None }]
    }

    fn encode_plan(&self, option: u32, feedback: Option<&str>) -> Vec<NudgeStep> {
        // Options 1-3 are direct selections; option 4 is freeform feedback.
        if option <= 3 {
            return vec![NudgeStep {
                bytes: format!("{option}\r").into_bytes(),
                delay_after: None,
            }];
        }

        // Option 4: type feedback text (the TUI opens a text input).
        let mut steps = vec![NudgeStep {
            bytes: format!("{option}\r").into_bytes(),
            delay_after: feedback.map(|_| self.feedback_delay),
        }];

        if let Some(text) = feedback {
            steps.push(NudgeStep { bytes: format!("{text}\r").into_bytes(), delay_after: None });
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
                steps.push(NudgeStep { bytes: step, delay_after: Some(self.input_delay) });
            }
            // Final confirm (Enter on the confirm tab).
            steps.push(NudgeStep { bytes: b"\r".to_vec(), delay_after: None });
            return steps;
        }

        // Single answer in a multi-question dialog: just emit the digit (TUI auto-advances).
        let answer = &answers[0];
        if total_questions > 1 {
            let bytes = self.encode_single_answer(answer);
            return vec![NudgeStep { bytes, delay_after: None }];
        }

        // Single-question dialog: emit answer + confirm.
        let bytes = self.encode_single_answer(answer);
        vec![NudgeStep { bytes: [&bytes[..], b"\r"].concat(), delay_after: None }]
    }

    fn encode_setup(&self, option: u32) -> Vec<NudgeStep> {
        vec![NudgeStep { bytes: format!("{option}\r").into_bytes(), delay_after: None }]
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
