// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::Duration;

use crate::driver::{NudgeEncoder, NudgeStep, QuestionAnswer, RespondEncoder};

/// Encodes nudge messages for Gemini CLI's terminal input.
pub struct GeminiNudgeEncoder;

impl NudgeEncoder for GeminiNudgeEncoder {
    fn encode(&self, message: &str) -> Vec<NudgeStep> {
        vec![NudgeStep {
            bytes: format!("{message}\r").into_bytes(),
            delay_after: None,
        }]
    }
}

/// Encodes prompt responses for Gemini CLI's terminal input.
pub struct GeminiRespondEncoder {
    pub feedback_delay: Duration,
}

impl Default for GeminiRespondEncoder {
    fn default() -> Self {
        Self {
            feedback_delay: Duration::from_millis(100),
        }
    }
}

impl RespondEncoder for GeminiRespondEncoder {
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
        _total_questions: usize,
    ) -> Vec<NudgeStep> {
        // Gemini uses simple single-question prompts; take the first answer.
        let answer = match answers.first() {
            Some(a) => a,
            None => return vec![],
        };

        if let Some(n) = answer.option {
            return vec![NudgeStep {
                bytes: format!("{n}\r").into_bytes(),
                delay_after: None,
            }];
        }

        if let Some(ref text) = answer.text {
            return vec![NudgeStep {
                bytes: format!("{text}\r").into_bytes(),
                delay_after: None,
            }];
        }

        vec![]
    }
}

#[cfg(test)]
#[path = "encoding_tests.rs"]
mod tests;
