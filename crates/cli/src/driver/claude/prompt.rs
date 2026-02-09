// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde_json::Value;

use crate::driver::{PromptContext, PromptKind, QuestionContext};
use crate::screen::ScreenSnapshot;

/// Extract permission prompt context from a session log entry.
///
/// Finds the last `tool_use` block in the message and extracts the tool
/// name and a truncated preview of its input.
pub fn extract_permission_context(json: &Value) -> PromptContext {
    let tool_use = find_last_tool_use(json);
    let (tool, input_preview) = match tool_use {
        Some(block) => {
            let tool = block.get("name").and_then(|v| v.as_str()).map(String::from);
            let preview = block.get("input").and_then(summarize_tool_input);
            (tool, preview)
        }
        None => (None, None),
    };

    PromptContext {
        kind: PromptKind::Permission,
        subtype: None,
        tool,
        input_preview,
        screen_lines: vec![],
        options: vec![],
        options_fallback: false,
        questions: vec![],
        question_current: 0,
    }
}

/// Extract question context from an `AskUserQuestion` tool_use block.
///
/// Handles Claude's tool input format where questions are in a
/// `questions` array with `question` text and `options[].label`.
pub fn extract_ask_user_context(block: &Value) -> PromptContext {
    extract_ask_user_from_tool_input(block.get("input"))
}

/// Extract question context directly from the tool input value.
///
/// Used by the `PreToolUse` hook path where `tool_input` is provided
/// directly (not wrapped in a `tool_use` block).
///
/// Parses all questions from `input.questions[]` into the `questions` vec.
/// Top-level `question`/`options` fields are populated from `questions[0]`
/// for backwards compatibility.
pub fn extract_ask_user_from_tool_input(input: Option<&Value>) -> PromptContext {
    let questions_arr = input.and_then(|i| i.get("questions")).and_then(|q| q.as_array());

    let questions: Vec<QuestionContext> = questions_arr
        .map(|arr| {
            arr.iter()
                .filter_map(|q| {
                    let question = q.get("question")?.as_str()?.to_string();
                    let options = q
                        .get("options")
                        .and_then(|v| v.as_array())
                        .map(|opts| {
                            opts.iter()
                                .filter_map(|v| {
                                    v.get("label")
                                        .and_then(|l| l.as_str())
                                        .or_else(|| v.as_str())
                                        .map(String::from)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(QuestionContext { question, options })
                })
                .collect()
        })
        .unwrap_or_default();

    PromptContext {
        kind: PromptKind::Question,
        subtype: None,
        tool: Some("AskUserQuestion".to_string()),
        input_preview: None,
        screen_lines: vec![],
        options: vec![],
        options_fallback: false,
        questions,
        question_current: 0,
    }
}

/// Extract plan prompt context from the terminal screen.
///
/// Plan prompts are detected via the screen rather than the session log,
/// so context is built from the visible screen lines.
pub fn extract_plan_context(screen: &ScreenSnapshot) -> PromptContext {
    PromptContext {
        kind: PromptKind::Plan,
        subtype: None,
        tool: None,
        input_preview: None,
        screen_lines: screen.lines.clone(),
        options: vec![],
        options_fallback: false,
        questions: vec![],
        question_current: 0,
    }
}

/// Parse numbered option labels from terminal screen lines.
///
/// Scans lines bottom-up looking for patterns like `❯ 1. Yes` or `  2. Don't ask again`.
/// Handles Claude's real TUI format:
/// - Selected option: `❯ 1. Label`
/// - Unselected: `  2. Label`
/// - Description lines indented under options (skipped)
/// - Separator lines `────...` and footer hints (skipped)
///
/// Collects matches and stops at the first non-option, non-skippable line above
/// the block. Returns options in ascending order (option 1 first).
pub fn parse_options_from_screen(lines: &[String]) -> Vec<String> {
    let mut options: Vec<(u32, String)> = Vec::new();
    let mut found_any = false;

    for line in lines.iter().rev() {
        let trimmed = line.trim();

        // Skip blank lines
        if trimmed.is_empty() {
            continue;
        }

        // Skip hint/footer lines (e.g. "Esc to cancel · Tab to amend")
        if is_hint_line(trimmed) {
            continue;
        }

        // Skip separator lines (e.g. "────────────")
        if is_separator_line(trimmed) {
            if found_any {
                // Separator above the options block can appear between groups
                // (e.g. question dialog splits options 1-3 from option 4)
                continue;
            }
            continue;
        }

        // Try to parse as a numbered option
        if let Some((num, label)) = parse_numbered_option(trimmed) {
            options.push((num, label));
            found_any = true;
        } else if found_any {
            // Non-option, non-skippable line. Could be a description line
            // indented under a previous option, or the end of the block.
            // Description lines are deeply indented (5+ spaces) with no
            // leading digit — skip those.
            if is_description_line(line) {
                continue;
            }
            // Otherwise we've hit content above the options block — stop.
            break;
        }
    }

    // Sort by option number ascending and return just the labels
    options.sort_by_key(|(num, _)| *num);
    options.into_iter().map(|(_, label)| label).collect()
}

/// Try to parse a line as a numbered option: `[❯ ] N. label`.
///
/// Strips leading selection indicator (`❯`) and whitespace before matching.
/// The `❯` may be followed by a regular space or a non-breaking space (U+00A0).
/// Returns `(number, label)` if the line matches.
fn parse_numbered_option(trimmed: &str) -> Option<(u32, String)> {
    // Strip the selection indicator (❯) if present, then any mix of
    // regular spaces and non-breaking spaces (U+00A0).
    let s = trimmed.strip_prefix('❯').unwrap_or(trimmed);
    let s = s.trim_start_matches([' ', '\u{00A0}']);

    // Must start with one or more digits
    let digit_end = s.find(|c: char| !c.is_ascii_digit())?;
    if digit_end == 0 {
        return None;
    }

    let num: u32 = s[..digit_end].parse().ok()?;

    // Must be followed by ". "
    let rest = s[digit_end..].strip_prefix(". ")?;

    // Label must be non-empty
    if rest.is_empty() {
        return None;
    }

    // Strip trailing selection indicators (e.g. " ✔" or " ✓") that Claude
    // renders after the currently-active option in picker dialogs.
    let label = rest
        .trim_end()
        .trim_end_matches(['✔', '✓'])
        .trim_end()
        .to_string();

    if label.is_empty() {
        return None;
    }

    Some((num, label))
}

/// Separator lines are composed entirely of box-drawing characters.
fn is_separator_line(trimmed: &str) -> bool {
    !trimmed.is_empty() && trimmed.chars().all(|c| matches!(c, '─' | '╌' | '━' | '═' | '│' | '┃'))
}

/// Hint/footer lines contain navigation instructions.
fn is_hint_line(trimmed: &str) -> bool {
    // Common Claude TUI footer patterns
    trimmed.contains("Esc to cancel")
        || trimmed.contains("Enter to select")
        || trimmed.contains("Enter to confirm")
        || trimmed.contains("Tab to amend")
        || trimmed.contains("Arrow keys to navigate")
}

/// Description lines are indented continuation text under a numbered option.
/// They start with 5+ spaces (deeper than option indentation) and don't begin
/// with a digit (ruling out numbered options themselves).
fn is_description_line(raw_line: &str) -> bool {
    let leading = raw_line.len() - raw_line.trim_start().len();
    if leading < 5 {
        return false;
    }
    let first_non_space = raw_line.trim_start().chars().next();
    !matches!(first_non_space, Some('0'..='9') | Some('❯') | None)
}

/// Truncate tool input JSON to a ~200 character preview string.
fn summarize_tool_input(input: &Value) -> Option<String> {
    let s = serde_json::to_string(input).ok()?;
    if s.len() <= 200 {
        return Some(s);
    }

    // Find a safe truncation point that doesn't split multi-byte chars
    let mut end = 200;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut result = s[..end].to_string();
    result.push_str("...");
    Some(result)
}

/// Find the last `tool_use` block in a message's content array.
fn find_last_tool_use(json: &Value) -> Option<&Value> {
    let content = json.get("message")?.get("content")?.as_array()?;
    content
        .iter()
        .rev()
        .find(|block| block.get("type").and_then(|v| v.as_str()) == Some("tool_use"))
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
