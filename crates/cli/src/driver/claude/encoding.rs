// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::Duration;

use crate::driver::{NudgeStep, QuestionAnswer, RespondEncoder};

pub use crate::driver::nudge::StandardNudgeEncoder as ClaudeNudgeEncoder;

/// Encodes prompt responses for Claude Code's terminal input.
pub struct ClaudeRespondEncoder {
    /// Delay between keystrokes in multi-step sequences.
    pub input_delay: Duration,
}

impl Default for ClaudeRespondEncoder {
    fn default() -> Self {
        Self { input_delay: Duration::from_millis(200) }
    }
}

impl RespondEncoder for ClaudeRespondEncoder {
    fn encode_permission(&self, option: u32) -> Vec<NudgeStep> {
        // Number key auto-confirms in Claude's TUI picker — no Enter needed.
        vec![NudgeStep { bytes: format!("{option}").into_bytes(), delay_after: None }]
    }

    fn encode_plan(&self, option: u32, feedback: Option<&str>) -> Vec<NudgeStep> {
        // Number key auto-confirms in Claude's TUI picker — no Enter needed.
        // Options 1-3 are direct selections; option 4 is freeform feedback.
        if option <= 3 || feedback.is_none() {
            return vec![NudgeStep { bytes: format!("{option}").into_bytes(), delay_after: None }];
        }

        // Option 4 with feedback: digit auto-selects the text input,
        // then type feedback text + Enter to submit.
        let text = feedback.unwrap_or_default();
        vec![
            NudgeStep {
                bytes: format!("{option}").into_bytes(),
                delay_after: Some(self.input_delay),
            },
            NudgeStep { bytes: format!("{text}\r").into_bytes(), delay_after: None },
        ]
    }

    fn encode_question(
        &self,
        answers: &[QuestionAnswer],
        _total_questions: usize,
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

        // Single answer: digit auto-confirms in the TUI picker, no Enter needed.
        // For multi-question dialogs, the digit auto-advances to the next question.
        let bytes = self.encode_single_answer(&answers[0]);
        vec![NudgeStep { bytes, delay_after: None }]
    }

    fn encode_setup(&self, option: u32) -> Vec<NudgeStep> {
        // Number key auto-confirms in Claude's TUI picker — no Enter needed.
        vec![NudgeStep { bytes: format!("{option}").into_bytes(), delay_after: None }]
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
