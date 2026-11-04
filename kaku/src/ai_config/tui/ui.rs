use std::borrow::Cow;
use std::collections::HashSet;

use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use super::{App, FieldEntry, Tool};
use crate::i18n::{display_width, pad_end_width, tr, truncate_width};
use crate::tui_core::chrome::popup_title_line;
use crate::tui_core::theme::{accent, bg, divider, muted, panel, primary, red, success, text_fg};

/// 树行 key 的分隔符：`Provider ▸ Base URL`。
const TREE_SEP: &str = " ▸ ";

/// 树行缩进比普通行多 3 列（`    │  ` vs `    `），键宽要相应减去 3，
/// 两类行的值列才对得齐。
const TREE_INDENT_DELTA: usize = 3;

/// 一个工具的可见标签。
///
/// 树行的 key 形如 `Provider ▸ Base URL`：前半是 provider 名（用户数据，
/// 不能翻译，也不能进语言包的 key），后半才是界面文案。provider 名本身
/// 那一行同样是用户数据，原样显示。
fn visible_labels(fields: &[FieldEntry]) -> Vec<String> {
    let provider_names: HashSet<&str> = fields
        .iter()
        .filter_map(|field| field.key.find(TREE_SEP).map(|pos| &field.key[..pos]))
        .collect();
    fields
        .iter()
        .map(|field| {
            if let Some(pos) = field.key.find(TREE_SEP) {
                let suffix = &field.key[pos + TREE_SEP.len()..];
                return format!("↳ {}", tr(suffix));
            }
            if provider_names.contains(field.key.as_str()) {
                return field.key.clone();
            }
            tr(&field.key).into_owned()
        })
        .collect()
}

/// 弹窗标题用的单字段版：弹窗（编辑器/选择器）每次重绘只需要当前字段
/// 一个标签，不必为整表做翻译和 HashSet 构建。树行保留 provider 名
/// （上下文），只翻译后半段文案。
fn field_title(fields: &[FieldEntry], idx: usize) -> String {
    let field = &fields[idx];
    if let Some(pos) = field.key.find(TREE_SEP) {
        return format!(
            "{}{}{}",
            &field.key[..pos],
            TREE_SEP,
            tr(&field.key[pos + TREE_SEP.len()..])
        );
    }
    let is_provider_row = fields.iter().any(|f| {
        f.key
            .find(TREE_SEP)
            .map(|pos| &f.key[..pos] == field.key.as_str())
            == Some(true)
    });
    if is_provider_row {
        return field.key.clone();
    }
    tr(&field.key).into_owned()
}

/// 头部摘要 = 静态英文前缀 + 用户数据（模型、账号、用量）。只翻译前缀，
/// 数据原样保留，避免语言包里的巧合条目改写用户数据。
fn tr_summary(summary: &str) -> Cow<'_, str> {
    // Whole-string copy first (no data suffix to preserve).
    const WHOLE: [&str; 6] = [
        "Loading usage...",
        "Setup failed",
        "Config unreadable",
        "Quota unavailable",
        "Open Antigravity to sync quota",
        "Open Antigravity to load quota",
    ];
    if WHOLE.contains(&summary) {
        return tr(summary);
    }
    const PREFIXES: [&str; 2] = ["Ready · ", "Setup required · "];
    for prefix in PREFIXES {
        if let Some(rest) = summary.strip_prefix(prefix) {
            let mut out = tr(prefix).into_owned();
            out.push_str(rest);
            return Cow::Owned(out);
        }
    }
    Cow::Borrowed(summary)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MainLayoutMode {
    HeaderOnly,
    HeaderAndFooter,
    Expanded,
    Compact,
}

pub(super) fn ui(frame: &mut ratatui::Frame, app: &mut App) {
    let full = frame.area();
    if full.width < 2 || full.height < 2 {
        return;
    }
    // Use the full frame so background and popup layers stay visually consistent
    // with the active theme across the whole terminal width.
    let area = full;

    // Clear frame content first to avoid stale glyph artifacts when redrawing
    // in non-alternate-screen mode.
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(Style::default().bg(bg())), area);

    let rendered_tool_rows = app.rendered_tool_row_count() as u16;
    match resolve_main_layout(area.height, rendered_tool_rows) {
        MainLayoutMode::HeaderOnly => {
            let chunks = Layout::vertical([Constraint::Length(2)]).split(area);
            render_header(frame, chunks[0], None);
        }
        MainLayoutMode::HeaderAndFooter => {
            let chunks =
                Layout::vertical([Constraint::Length(2), Constraint::Length(1)]).split(area);
            render_header(frame, chunks[0], None);
            render_status_bar(frame, chunks[1], app);
        }
        MainLayoutMode::Expanded => {
            let chunks = Layout::vertical([
                Constraint::Length(2),                  // logo header
                Constraint::Length(rendered_tool_rows), // tool list
                Constraint::Fill(1),                    // flexible gap above status
                Constraint::Length(1),                  // spacer above status
                Constraint::Length(1),                  // status bar (stick to bottom)
            ])
            .split(area);

            render_header(frame, chunks[0], None);
            render_tools(frame, chunks[1], app);
            render_status_bar(frame, chunks[4], app);
        }
        MainLayoutMode::Compact => {
            let chunks = Layout::vertical([
                Constraint::Length(2), // logo header
                Constraint::Fill(1),   // tool list
                Constraint::Length(1), // spacer above status
                Constraint::Length(1), // status bar (stick to bottom)
            ])
            .split(area);

            render_header(frame, chunks[0], None);
            render_tools(frame, chunks[1], app);
            render_status_bar(frame, chunks[3], app);
        }
    }

    if app.is_selecting() {
        render_selector(frame, area, app);
    } else if app.is_editing() {
        render_editor(frame, area, app);
    }
}

fn resolve_main_layout(area_height: u16, content_rows: u16) -> MainLayoutMode {
    let remaining_height = area_height.saturating_sub(2);
    if remaining_height == 0 {
        MainLayoutMode::HeaderOnly
    } else if remaining_height == 1 {
        MainLayoutMode::HeaderAndFooter
    } else if content_rows + 2 <= remaining_height {
        MainLayoutMode::Expanded
    } else {
        MainLayoutMode::Compact
    }
}

fn render_header(frame: &mut ratatui::Frame, area: Rect, status: Option<&str>) {
    let mut spans = vec![
        Span::styled(
            "  Kaku",
            Style::default().fg(primary()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(muted())),
        Span::styled("AI", Style::default().fg(text_fg())),
    ];
    if let Some(status) = status {
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled(status, Style::default().fg(muted())));
    }
    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(vec![line, Line::from("")]), area);
}

fn render_tools(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let mut items: Vec<ListItem> = Vec::new();
    let mut selected_flat: Option<usize> = None;
    let mut flat = 0usize;
    // 每个工具的可见标签在整帧里算一次：宽度和行渲染都用它，
    // 语言包是进程级只读的，查表结果在帧内不变。
    let labels: Vec<Vec<String>> = app
        .tools
        .iter()
        .map(|tool| visible_labels(&tool.fields))
        .collect();

    for (ti, tool) in app.tools.iter().enumerate() {
        let is_current_tool = ti == app.tool_index;
        let is_collapsed = app.tool_is_collapsed(ti);
        let header_selected =
            is_current_tool && tool.tool == Tool::KakuAssistant && app.field_index == 0;

        let tool_style = if header_selected || (is_current_tool && tool.tool != Tool::KakuAssistant)
        {
            Style::default().fg(primary()).add_modifier(Modifier::BOLD)
        } else if tool.installed {
            Style::default().fg(success()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted())
        };

        let mut header = Line::from(vec![
            Span::styled(
                if header_selected || (is_current_tool && tool.tool != Tool::KakuAssistant) {
                    "➤ "
                } else {
                    "  "
                },
                Style::default().fg(primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(tool.tool.label(), tool_style),
        ]);
        if tool.tool == Tool::KakuAssistant {
            header.spans.push(Span::styled(
                if is_collapsed { " ▸" } else { " ▾" },
                Style::default().fg(if is_current_tool { primary() } else { muted() }),
            ));
        }
        if let Some(summary) = &tool.summary {
            header.spans.push(Span::styled("  ", Style::default()));
            header.spans.push(Span::styled(
                tr_summary(summary),
                Style::default().fg(text_fg()),
            ));
        } else if !tool.installed {
            header.spans.push(Span::styled(
                tr("not installed"),
                Style::default().fg(muted()),
            ));
        }
        items.push(ListItem::new(header));
        if header_selected {
            selected_flat = Some(flat);
        }
        flat += 1;

        if is_collapsed {
            items.push(ListItem::new(Line::raw("")));
            flat += 1;
            continue;
        }

        // 一个工具一列宽度：普通行和树行的值列对齐（树行缩进多 3 列，
        // 键宽相应减 3）。上限 30 列，超长译文按显示宽度截断，不会把值
        // 挤出屏幕。
        let tool_labels = &labels[ti];
        let key_width = tool_labels
            .iter()
            .zip(tool.fields.iter())
            .map(|(label, field)| {
                display_width(label)
                    + if field.key.contains(TREE_SEP) {
                        TREE_INDENT_DELTA
                    } else {
                        0
                    }
            })
            .max()
            .unwrap_or(24)
            .clamp(24, 30);

        for (fi, field) in tool.fields.iter().enumerate() {
            let display_index = if tool.tool == Tool::KakuAssistant {
                fi + 1
            } else {
                fi
            };
            let is_selected = is_current_tool && display_index == app.field_index;
            if is_selected {
                selected_flat = Some(flat);
            }

            let last = fi == tool.fields.len() - 1;
            let connector = if last { "└" } else { "├" };
            let rule = "─";

            let val_color = if field.value.starts_with('✓')
                || (field.key.contains("API Key") && field.value != "-")
            {
                success()
            } else if field.value.starts_with('✗') {
                red()
            } else if field.value == "-" {
                muted()
            } else {
                accent()
            };

            let extra_indent = field.key.contains(TREE_SEP);
            // 树行缩进多 3 列，键宽相应减 3，两类行的值列才对齐。
            let row_key_width = key_width - if extra_indent { TREE_INDENT_DELTA } else { 0 };
            let display_key = truncate_width(&tool_labels[fi], row_key_width);

            let indent_str = if extra_indent { "    │  " } else { "    " };
            let tree_color = if is_selected { primary() } else { divider() };
            let row_color = if is_selected { primary() } else { text_fg() };
            let key_marker = if is_selected { "› " } else { "  " };

            let val_prefix = if field.value.starts_with('✓') || field.value.starts_with('✗') {
                ""
            } else if field.editable {
                "→ "
            } else {
                "· "
            };

            let key_style = Style::default().fg(row_color).add_modifier(if is_selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
            let value_style = Style::default()
                .fg(if is_selected { primary() } else { val_color })
                .add_modifier(if is_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                });

            let line = Line::from(vec![
                Span::styled(indent_str, Style::default().fg(tree_color)),
                Span::styled(
                    connector,
                    Style::default()
                        .fg(tree_color)
                        .add_modifier(if is_selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::styled(format!("{rule} "), Style::default().fg(tree_color)),
                Span::styled(
                    key_marker,
                    Style::default()
                        .fg(if is_selected { primary() } else { muted() })
                        .add_modifier(if is_selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::styled(pad_end_width(&display_key, row_key_width), key_style),
                Span::styled(val_prefix, value_style),
                Span::styled(&field.value, value_style),
            ]);

            items.push(ListItem::new(line));
            flat += 1;
        }

        items.push(ListItem::new(Line::raw("")));
        flat += 1;
    }

    let mut state = ListState::default();
    state.select(selected_flat);

    let list = List::new(items).highlight_style(Style::default());

    frame.render_stateful_widget(list, area, &mut state);
}

fn render_status_bar(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let status = if let Some(msg) = &app.last_error {
        Line::from(vec![
            Span::styled(" ✖ ", Style::default().fg(red())),
            Span::styled(msg.as_str(), Style::default().fg(red())),
        ])
    } else if let Some(msg) = &app.status_msg {
        Line::from(vec![
            Span::styled(" ℹ ", Style::default().fg(success())),
            Span::styled(msg.as_str(), Style::default().fg(text_fg())),
        ])
    } else {
        render_status_hint(area.width)
    };

    frame.render_widget(Paragraph::new(status), area);
}

fn render_status_hint(width: u16) -> Line<'static> {
    if width >= 52 {
        Line::from(vec![
            Span::styled(
                " ↑↓ ",
                Style::default().fg(primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Navigate", Style::default().fg(muted())),
            Span::styled(" | ", Style::default().fg(muted())),
            Span::styled(
                " Enter ",
                Style::default().fg(primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Edit", Style::default().fg(muted())),
            Span::styled(" | ", Style::default().fg(muted())),
            Span::styled(
                " Esc ",
                Style::default().fg(primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Back", Style::default().fg(muted())),
            Span::styled(" | ", Style::default().fg(muted())),
            Span::styled(
                " R ",
                Style::default().fg(primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Refresh", Style::default().fg(muted())),
        ])
    } else if width >= 34 {
        Line::from(vec![
            Span::styled(
                " ↑↓ ",
                Style::default().fg(primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Move", Style::default().fg(muted())),
            Span::styled(" · ", Style::default().fg(muted())),
            Span::styled(
                " Enter ",
                Style::default().fg(primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Edit", Style::default().fg(muted())),
            Span::styled(" · ", Style::default().fg(muted())),
            Span::styled(
                " Esc ",
                Style::default().fg(primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Back", Style::default().fg(muted())),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " ↑↓ ",
                Style::default().fg(primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Move", Style::default().fg(muted())),
            Span::styled(" · ", Style::default().fg(muted())),
            Span::styled(
                " Enter ",
                Style::default().fg(primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Edit", Style::default().fg(muted())),
        ])
    }
}

pub(super) fn render_editor(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(tool) = app.tools.get(app.tool_index) else {
        return;
    };
    let Some((field_idx, edit_buf, edit_cursor)) = app.editing_view() else {
        return;
    };
    if field_idx >= tool.fields.len() {
        return;
    }
    // 弹窗标题只需当前字段一个标签。
    let field_label = field_title(&tool.fields, field_idx);

    let popup_width = ((area.width as f32 * 0.8) as u16).min(area.width.saturating_sub(4));
    let popup_height = 5u16.min(area.height.saturating_sub(4));
    let popup = Rect::new(
        (area.width.saturating_sub(popup_width)) / 2,
        (area.height.saturating_sub(popup_height)) / 2,
        popup_width,
        popup_height,
    );

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(Line::from(popup_title_line(
            " Edit: ",
            field_label,
            &[("Enter", "Save"), ("Esc", "Cancel")],
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(primary()))
        .style(Style::default().bg(panel()));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let content_area = inner.inner(Margin::new(1, 0));

    let line = if edit_buf.is_empty() {
        Line::from(Span::styled(" ", Style::default().bg(primary())))
    } else {
        let cursor_pos = edit_cursor;
        let before = &edit_buf[..cursor_pos];
        let after = &edit_buf[cursor_pos..];

        if cursor_pos >= edit_buf.len() {
            Line::from(vec![
                Span::styled(before, Style::default().fg(text_fg())),
                Span::styled(" ", Style::default().bg(primary())),
            ])
        } else {
            let mut chars = after.chars();
            let current_char = chars.next().unwrap_or(' ');
            let remaining = chars.as_str();

            Line::from(vec![
                Span::styled(before, Style::default().fg(text_fg())),
                Span::styled(
                    current_char.to_string(),
                    Style::default().bg(primary()).fg(bg()),
                ),
                Span::styled(remaining, Style::default().fg(text_fg())),
            ])
        }
    };

    let input = Paragraph::new(vec![line]).wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(input, content_area);
}

fn render_selector(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(tool) = app.tools.get(app.tool_index) else {
        return;
    };
    let Some((field_idx, select_options, select_index, select_filter)) = app.selecting_view()
    else {
        return;
    };
    if field_idx >= tool.fields.len() {
        return;
    }
    // 弹窗标题只需当前字段一个标签。
    let field_label = field_title(&tool.fields, field_idx);

    let option_count = select_options.len() as u16;
    let max_popup_width = area.width.saturating_sub(4);
    let min_popup_width = 60u16.min(max_popup_width);
    let longest_option_width = select_options
        .iter()
        .map(|opt| display_width(opt) as u16)
        .max()
        .unwrap_or(0);
    let popup_width = std::cmp::max(
        min_popup_width,
        longest_option_width.saturating_add(6).min(max_popup_width),
    );
    // Popup = borders(2) + filter line(1) + option rows. A floor of 5 keeps
    // the "No matches" row visible when the filter empties the list: at the
    // bare 3-row height the list area is 0 rows tall and renders nothing.
    let popup_height = (option_count + 3).max(5).min(area.height.saturating_sub(4));
    let popup = Rect::new(
        (area.width.saturating_sub(popup_width)) / 2,
        (area.height.saturating_sub(popup_height)) / 2,
        popup_width,
        popup_height,
    );

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(Line::from(popup_title_line(
            " Select: ",
            field_label,
            &[
                ("Enter/Space", "Apply"),
                ("Esc", "Cancel"),
                ("Type", "Filter"),
            ],
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(primary()))
        .style(Style::default().bg(panel()));

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let filter_line = if select_filter.is_empty() {
        tr("Filter: type to narrow large model lists").into_owned()
    } else {
        format!("{}: {select_filter}", tr("Filter"))
    };
    let filter_area = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(
        Paragraph::new(filter_line).style(Style::default().fg(muted()).bg(panel())),
        filter_area,
    );

    let list_area = Rect::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    );

    let items: Vec<ListItem> = select_options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            let is_sel = i == select_index;
            let marker = if is_sel { "➤ " } else { "  " };
            let style = if is_sel {
                Style::default().fg(primary()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(text_fg())
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    marker,
                    Style::default().fg(primary()).add_modifier(Modifier::BOLD),
                ),
                // 选项是模型 ID 等用户数据，不是语言包键：原样显示，也和
                // popup 宽度按原文计算保持同一口径。
                Span::styled(opt.as_str(), style),
            ]))
        })
        .collect();

    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(tr("No matches").into_owned())
                .style(Style::default().fg(muted()).bg(panel())),
            list_area,
        );
        return;
    }

    let mut state = ListState::default();
    state.select(Some(select_index));

    let list = List::new(items).highlight_style(Style::default());
    frame.render_stateful_widget(list, list_area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::{resolve_main_layout, tr_summary, visible_labels, FieldEntry, MainLayoutMode};

    #[test]
    fn keeps_spacer_in_compact_layout() {
        assert_eq!(resolve_main_layout(10, 9), MainLayoutMode::Compact);
    }

    #[test]
    fn requires_room_for_status_and_spacer_before_expanding() {
        assert_eq!(resolve_main_layout(10, 6), MainLayoutMode::Expanded);
        assert_eq!(resolve_main_layout(10, 7), MainLayoutMode::Compact);
    }

    #[test]
    fn handles_tiny_terminal_heights() {
        assert_eq!(resolve_main_layout(2, 1), MainLayoutMode::HeaderOnly);
        assert_eq!(resolve_main_layout(3, 1), MainLayoutMode::HeaderAndFooter);
    }

    /// 树行的 key 是 `Provider ▸ Base URL`：provider 名是用户数据，
    /// 只有后半段是界面文案。语言包的 key 必须是界面上真正显示的文本，
    /// 所以这里必须拆开查表，不能拿整串 key 去查。
    #[test]
    fn tree_label_uses_the_suffix_and_keeps_the_provider_name_verbatim() {
        let fields = vec![
            FieldEntry {
                key: "Antigravity".to_string(),
                value: "provider".to_string(),
                ..Default::default()
            },
            FieldEntry {
                key: "Antigravity ▸ Base URL".to_string(),
                value: "https://example.test".to_string(),
                ..Default::default()
            },
        ];
        let labels = visible_labels(&fields);
        assert_eq!(labels[0], "Antigravity");
        assert_eq!(labels[1], "↳ Base URL");
    }

    /// 摘要里的模型名、账号是用户数据，不能进语言包，也不能被翻译。
    #[test]
    fn summary_translates_only_the_static_prefix() {
        assert_eq!(tr_summary("Ready · gpt-5"), "Ready · gpt-5");
        assert_eq!(tr_summary("someone@example.test"), "someone@example.test");
    }

    /// 全文摘要（无数据后缀）也要可翻译；此前 "Loading usage..." 只进
    /// 了 PREFIXES 之外的死角，语言包覆盖不到。
    #[test]
    fn summary_translates_whole_string_copy() {
        assert_eq!(tr_summary("Loading usage..."), "Loading usage...");
        assert_eq!(tr_summary("Setup failed"), "Setup failed");
        assert_eq!(tr_summary("Re-auth required"), "Re-auth required");
    }
}
