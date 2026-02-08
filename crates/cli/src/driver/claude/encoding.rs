// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::Duration;

use crate::driver::{NudgeEncoder, NudgeStep, RespondEncoder};

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
pub struct ClaudeRespondEncoder;

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
            delay_after: feedback.map(|_| Duration::from_millis(100)),
        }];

        if let Some(text) = feedback {
            steps.push(NudgeStep {
                bytes: format!("{text}\r").into_bytes(),
                delay_after: None,
            });
        }

        steps
    }

    fn encode_question(&self, option: Option<u32>, text: Option<&str>) -> Vec<NudgeStep> {
        if let Some(n) = option {
            return vec![NudgeStep {
                bytes: format!("{n}\r").into_bytes(),
                delay_after: None,
            }];
        }

        if let Some(text) = text {
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
