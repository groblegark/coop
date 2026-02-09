// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::Duration;

use crate::driver::{NudgeEncoder, NudgeStep, QuestionAnswer, RespondEncoder};

/// Encodes nudge messages for Gemini CLI's terminal input.
pub struct GeminiNudgeEncoder {
    /// Delay between typing the message and pressing enter to send.
    pub keyboard_delay: Duration,
}

impl NudgeEncoder for GeminiNudgeEncoder {
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

/// Encodes prompt responses for Gemini CLI's terminal input.
pub struct GeminiRespondEncoder {
    pub feedback_delay: Duration,
}

impl Default for GeminiRespondEncoder {
    fn default() -> Self {
        Self { feedback_delay: Duration::from_millis(100) }
    }
}

impl RespondEncoder for GeminiRespondEncoder {
    fn encode_permission(&self, accept: bool) -> Vec<NudgeStep> {
        let bytes = if accept { b"1\r".to_vec() } else { b"\x1b".to_vec() };
        vec![NudgeStep { bytes, delay_after: None }]
    }

    fn encode_plan(&self, accept: bool, feedback: Option<&str>) -> Vec<NudgeStep> {
        if accept {
            return vec![NudgeStep { bytes: b"y\r".to_vec(), delay_after: None }];
        }

        let mut steps = vec![NudgeStep {
            bytes: b"n\r".to_vec(),
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
        _total_questions: usize,
    ) -> Vec<NudgeStep> {
        // Gemini uses simple single-question prompts; take the first answer.
        let answer = match answers.first() {
            Some(a) => a,
            None => return vec![],
        };

        if let Some(n) = answer.option {
            return vec![NudgeStep { bytes: format!("{n}\r").into_bytes(), delay_after: None }];
        }

        if let Some(ref text) = answer.text {
            return vec![NudgeStep { bytes: format!("{text}\r").into_bytes(), delay_after: None }];
        }

        vec![]
    }
}

#[cfg(test)]
#[path = "encoding_tests.rs"]
mod tests;
