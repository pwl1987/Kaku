//! Shared chrome pieces for the CLI TUIs (config and AI config).
//!
//! These build the translated key-hint spans that appear in popup titles,
//! so the "key + : label" wording and its language-pack lookup keys stay
//! identical across every TUI flow instead of drifting per call site.

use ratatui::style::Style;
use ratatui::text::Span;

use crate::i18n::tr;
use crate::tui_core::theme::{muted, primary, text_fg};

/// A key hint inside a popup title: the key in the primary color followed
/// by `: label`, with the label looked up in the overlay language pack.
pub fn key_hint_spans(hints: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(hints.len() * 3);
    for (idx, (key, label)) in hints.iter().enumerate() {
        if idx > 0 {
            // Two spaces between hints, matching the pre-i18n hand-built
            // spans (`: Save  Esc: Cancel `) — without this the labels
            // run together (`Enter: 保存Esc: 取消`).
            spans.push(Span::styled("  ", Style::default()));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default().fg(primary()),
        ));
        spans.push(Span::styled(
            format!(": {}", tr(label)),
            Style::default().fg(muted()),
        ));
    }
    spans
}

/// A popup title line: `prefix` + the subject + hints, with the
/// double-space separators the TUIs use.
///
/// The subject is the final display text: callers translate it themselves
/// (`tr` for a static label, the tool's own visible label for a field
/// whose key mixes data and copy). Translating it here would look up an
/// already-translated string a second time.
pub fn popup_title_line(
    prefix: &str,
    subject: impl Into<String>,
    hints: &[(&str, &str)],
) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(prefix.to_string(), Style::default().fg(primary())),
        Span::styled(subject.into(), Style::default().fg(text_fg())),
    ];
    if !hints.is_empty() {
        spans.push(Span::styled("  ", Style::default()));
        spans.extend(key_hint_spans(hints));
    }
    spans
}
