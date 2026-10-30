//! [`AgentNode`]: the `NodeContent` for an agent card.
//!
//! Renders a bordered card showing the agent type, a status glyph + color,
//! the model and effort, a truncated description, a tool count, the last tool
//! name, and an output token count. Visual state (status, tool tallies) is mirrored from the
//! domain model into the node via `flow.node_content_mut` on each sync, so the
//! card needs no back-reference to the `SessionModel`.

use rataflow::{NodeContent, NodeRenderContext};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::state::session::AgentStatus;

/// Fixed card dimensions for main / workflow nodes (world units).
pub const MAIN_NODE_DIMS: (f64, f64) = (30.0, 7.0);
/// Fixed card dimensions for subagent nodes (world units).
pub const SUB_NODE_DIMS: (f64, f64) = (26.0, 7.0);

/// Below this on-screen size a card has no room for any text — it renders at
/// cell level instead (solid status-colored fill). Semantic zoom: zoomed out,
/// nodes show as status cells instead of empty bordered boxes.
pub const CELL_MIN_WIDTH: u16 = 10;
/// See [`CELL_MIN_WIDTH`].
pub const CELL_MIN_HEIGHT: u16 = 3;

/// A card with fewer rows than this inside its border keeps them for what tells
/// agents apart at a glance, so the zoomed-out overview has no model row.
const MODEL_ROW_MIN_INNER_HEIGHT: u16 = 3;

/// Custom node content rendered as an agent card.
///
/// Plain `pub` fields: the graph layer reads them back and mutates them in
/// place during incremental sync (mirroring [`crate::state::session::AgentInfo`]).
#[derive(Debug, Clone)]
pub struct AgentNode {
    /// Title line — the agent type, which for the main agent is the provider's
    /// own name (`claude`, `codex`).
    pub title: String,
    /// Truncated description shown under the title.
    pub description: Option<String>,
    /// The model the agent was last observed running, as the transcript names it.
    pub model: Option<String>,
    /// The effort the agent was last asked to run at, as the transcript words it.
    pub effort: Option<String>,
    pub status: AgentStatus,
    /// Number of tool calls (for the `⚒ N tools` line).
    pub tool_count: usize,
    /// Name of the most recent tool call, if any.
    pub last_tool: Option<String>,
    pub output_tokens: u64,
    /// Interactive agents (main, forks) word `Running` as "active": we know
    /// there are recent entries, not that a task is executing.
    pub interactive: bool,
}

use crate::ui::truncate;

/// Compact human token count, e.g. `1.2k`, `34`, `2.0M`.
fn fmt_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// Fit a model id into `budget` columns: drop whole leading dash-separated
/// segments until it fits, then truncate what is left. An id's tail tells models
/// apart better than its head does, and this needs no knowledge of anyone's names.
/// Dropping stops before the remainder would hold no letters: a bare version
/// number says nothing about which model it belongs to.
///
/// One pass over the id, however many segments it has: the id comes from the
/// transcript and this runs on every frame. A dash ends any sequence a string's
/// width depends on, so widths add up across it and what is left never needs
/// measuring again.
fn fit_model(id: &str, budget: usize) -> String {
    let droppable = id
        .rfind(char::is_alphabetic)
        .map_or("", |last_letter| &id[..last_letter]);
    let mut width = id.width();
    let mut start = 0;
    for (dash, _) in droppable.match_indices('-') {
        if width <= budget {
            break;
        }
        width -= id[start..=dash].width();
        start = dash + 1;
    }
    truncate(&id[start..], budget)
}

/// The row under the title: `<model> · <effort>`, whichever of them was recorded,
/// or `None` when neither was. Only the model gives way to the width, so the effort
/// word stays whole for as long as any of the model shows.
fn model_row(model: Option<&str>, effort: Option<&str>, width: usize) -> Option<String> {
    let model = model.filter(|m| !m.is_empty());
    let effort = effort.filter(|e| !e.is_empty());
    match (model, effort) {
        (None, None) => None,
        (Some(model), None) => Some(fit_model(model, width)),
        (None, Some(effort)) => Some(truncate(effort, width)),
        (Some(model), Some(effort)) => {
            let suffix = format!(" · {effort}");
            let Some(budget) = width.checked_sub(suffix.width()) else {
                return Some(truncate(&format!("{model}{suffix}"), width));
            };
            let fitted = fit_model(model, budget);
            Some(if fitted.is_empty() {
                effort.to_string()
            } else {
                format!("{fitted}{suffix}")
            })
        }
    }
}

impl NodeContent for AgentNode {
    fn render(&self, ctx: &NodeRenderContext, buf: &mut Buffer) {
        let palette = ctx.theme.palette();
        let area = ctx.area;

        // Degenerate areas: a border needs at least 2x2; bail out gracefully.
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Single-source vocabulary + presence colors (cards/panel/inspect
        // share them; only the pulse override below is card-specific).
        let glyph_color = crate::ui::status_color(self.status, &palette);
        let status_text = crate::state::session::status_word(self.status, self.interactive);

        // Cell level (semantic zoom): too small for any text — a bordered card
        // carries no information here, so paint a solid status-colored block.
        if area.width < CELL_MIN_WIDTH || area.height < CELL_MIN_HEIGHT {
            let mut fill = Style::default().bg(glyph_color);
            if ctx.selected {
                fill = fill.add_modifier(Modifier::REVERSED);
            }
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    buf[(x, y)].set_char(' ').set_style(fill);
                }
            }
            return;
        }

        let border_color = if ctx.selected {
            palette.accent
        } else {
            palette.muted
        };
        let bg_style = Style::default().bg(palette.surface);
        let border_style = bg_style.fg(border_color);

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(bg_style);

        // Only pad when there's horizontal room to spare.
        if area.width >= 4 {
            block = block.padding(Padding::horizontal(1));
        }

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // Pulse: alive agents breathe on the shared animation clock (~1s
        // cycle at the default 120ms phase step) — the wide-shot heartbeat.
        let glyph = if self.status == AgentStatus::Running && (ctx.animation_phase / 4) % 2 == 1 {
            '○'
        } else {
            self.status.glyph()
        };
        let inner_w = inner.width as usize;

        // Title row: status glyph + agent type, bold.
        let title_budget = inner_w.saturating_sub(2); // glyph + space
        let title = truncate(&self.title, title_budget);
        let title_line = Line::from(vec![
            Span::styled(format!("{glyph} "), bg_style.fg(glyph_color)),
            Span::styled(
                title,
                bg_style.fg(palette.text).add_modifier(Modifier::BOLD),
            ),
        ]);

        // Build the candidate lines in priority order.
        let mut lines: Vec<Line> = Vec::new();
        lines.push(title_line);

        if inner.height >= MODEL_ROW_MIN_INNER_HEIGHT
            && let Some(row) = model_row(self.model.as_deref(), self.effort.as_deref(), inner_w)
        {
            lines.push(Line::from(Span::styled(row, bg_style.fg(palette.subtle))));
        }

        if let Some(desc) = self.description.as_ref().filter(|d| !d.is_empty()) {
            let desc = truncate(desc, inner_w);
            lines.push(Line::from(Span::styled(desc, bg_style.fg(palette.subtle))));
        }

        // Tools row: "⚒ N · last_tool".
        let tools_text = if let Some(last) = self.last_tool.as_ref() {
            let prefix = format!("⚒ {} · ", self.tool_count);
            let budget =
                inner_w.saturating_sub(unicode_width::UnicodeWidthStr::width(prefix.as_str()));
            format!("{prefix}{}", truncate(last, budget))
        } else {
            format!("⚒ {} tools", self.tool_count)
        };
        lines.push(Line::from(Span::styled(
            tools_text,
            bg_style.fg(palette.accent),
        )));

        // Footer row: status word + token count, separated to the edges.
        let tokens = fmt_tokens(self.output_tokens);
        let footer = if self.output_tokens > 0 {
            Line::from(vec![
                Span::styled(status_text, bg_style.fg(glyph_color)),
                Span::styled(format!("  {tokens} tok"), bg_style.fg(palette.muted)),
            ])
        } else {
            Line::from(Span::styled(status_text, bg_style.fg(glyph_color)))
        };
        lines.push(footer);

        // Each row is one cell tall, stacked from the top; render only what fits.
        for (i, line) in lines.into_iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            let rect = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
            Paragraph::new(line).style(bg_style).render(rect, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_budget() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 4), "hel…");
        assert_eq!(truncate("hello", 1), "…");
        assert_eq!(truncate("hello", 0), "");
    }

    #[test]
    fn truncate_handles_multibyte() {
        // Must not panic on a char boundary; counts columns not bytes.
        let s = "café crème brûlée";
        let out = truncate(s, 5);
        assert_eq!(out.chars().count(), 5);
    }

    #[test]
    fn truncate_measures_display_columns_not_chars() {
        use unicode_width::UnicodeWidthStr;
        // CJK chars are 2 columns wide: 6 chars = 12 columns must truncate to
        // fit a 10-column budget (chars-based counting would pass it through
        // and overflow the card).
        let s = "修复解析错误";
        let out = truncate(s, 10);
        assert!(out.width() <= 10, "{out:?} is {} columns", out.width());
        assert!(out.ends_with('…'));
        // And an exact fit is left alone.
        assert_eq!(truncate(s, 12), s);
    }

    #[test]
    fn token_formatting() {
        assert_eq!(fmt_tokens(0), "0");
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(1_500), "1.5k");
        assert_eq!(fmt_tokens(2_000_000), "2.0M");
    }

    #[test]
    fn glyphs_per_status() {
        assert_eq!(AgentStatus::Running.glyph(), '●');
        assert_eq!(AgentStatus::Done.glyph(), '✓');
        assert_eq!(AgentStatus::Failed.glyph(), '✗');
    }

    fn sample_node() -> AgentNode {
        AgentNode {
            title: "claude".into(),
            description: None,
            model: Some("claude-opus-4-8".into()),
            effort: Some("high".into()),
            status: AgentStatus::Done,
            tool_count: 3,
            last_tool: Some("Bash".into()),
            output_tokens: 1200,
            interactive: false,
        }
    }

    fn render_node(node: &AgentNode, area: ratatui::layout::Rect) -> Buffer {
        use rataflow::Theme;
        use rataflow::types::Position;

        let ctx = NodeRenderContext {
            id: "main",
            area,
            selected: false,
            dragging: false,
            position_absolute: Position::new(0.0, 0.0),
            theme: Theme::default(),
            animation_phase: 0,
        };
        let mut buf = Buffer::empty(area);
        node.render(&ctx, &mut buf);
        buf
    }

    fn render_into(area: ratatui::layout::Rect) -> Buffer {
        render_node(&sample_node(), area)
    }

    /// One row of a card rendered at the origin, inside its border and padding.
    fn inner_row(buf: &Buffer, y: u16) -> String {
        let area = buf.area;
        (area.left() + 2..area.right() - 2)
            .map(|x| buf[(x, y)].symbol())
            .collect()
    }

    /// A subagent-sized card with `model` and `effort`, read at its second row.
    fn row_of(model: Option<&str>, effort: Option<&str>) -> String {
        let node = AgentNode {
            description: Some("research".into()),
            model: model.map(str::to_string),
            effort: effort.map(str::to_string),
            ..sample_node()
        };
        let buf = render_node(&node, ratatui::layout::Rect::new(0, 0, 26, 7));
        inner_row(&buf, 2).trim_end().to_string()
    }

    #[test]
    fn fit_model_drops_leading_segments_then_truncates() {
        assert_eq!(fit_model("claude-sonnet-5-5", 15), "sonnet-5-5");
        assert_eq!(fit_model("gpt-5.6-terra", 22), "gpt-5.6-terra");
        assert_eq!(fit_model("abcdefghij", 5), "abcd…");
        // A trailing dash has no segment behind it to keep.
        assert_eq!(fit_model("abcdef-", 3), "ab…");
    }

    /// The rule stated one drop at a time, measuring what is left after each.
    fn fit_by_measuring_each_drop(id: &str, budget: usize) -> String {
        let mut fitted = id;
        while fitted.width() > budget {
            match fitted.split_once('-') {
                Some((_, tail)) if tail.chars().any(char::is_alphabetic) => fitted = tail,
                _ => break,
            }
        }
        truncate(fitted, budget)
    }

    #[test]
    fn fit_model_matches_measuring_after_each_drop() {
        // Besides plain ids: wide characters, and the sequences a string's width
        // depends on (emoji and text presentation, CRLF, ligatures, conjuncts, ZWJ),
        // placed against a dash so that a width which did not add up across it shows.
        for id in [
            "claude-sonnet-5-5",
            "gpt-5.6-terra",
            "abcdef-",
            "-",
            "--a--",
            "4-8-12",
            "a-1-b-2-",
            "模型-中文-4-五",
            "\u{1F600}-x-\u{FE0F}-y",
            "1\u{FE0F}-#-\u{FE0F}z-\u{2764}\u{FE0E}",
            "a\r-\nb-c",
            "\u{644}-\u{627}-\u{644}\u{627}-q",
            "\u{1780}\u{17D2}-\u{1780}-q",
            "a\u{0301}-\u{200D}b-\u{200D}-c",
            "\u{1F468}\u{200D}-\u{1F469}-d",
        ] {
            for budget in 0..=id.width() + 1 {
                assert_eq!(
                    fit_model(id, budget),
                    fit_by_measuring_each_drop(id, budget),
                    "{id:?} at {budget}"
                );
            }
        }
    }

    /// An id comes from the transcript and every card fits it on every frame, so
    /// its cost must not grow with the square of its length. At this size, measuring
    /// after each drop takes many minutes; the bound only tells that from one pass.
    #[test]
    fn fit_model_stays_linear_on_an_id_of_many_segments() {
        const SEGMENTS: usize = 200_000;
        let (sent, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let fitted = [
                fit_model(&"a-".repeat(SEGMENTS), 22),
                // The only letter is last, so every drop has to look for one.
                fit_model(&format!("{}a", "1-".repeat(SEGMENTS)), 22),
                // Dropping stops at the last letter, far wider than the budget.
                fit_model(
                    &format!("{}{}", "a-".repeat(SEGMENTS), "1-".repeat(SEGMENTS)),
                    22,
                ),
            ];
            sent.send(fitted).unwrap();
        });
        let fitted = received
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("fitting a long id should finish within the bound");
        assert_eq!(
            fitted,
            [
                "a-a-a-a-a-a-a-a-a-a-a-",
                "1-1-1-1-1-1-1-1-1-1-a",
                "a-1-1-1-1-1-1-1-1-1-1…",
            ]
        );
    }

    #[test]
    fn fit_model_keeps_a_part_with_letters() {
        assert_eq!(fit_model("claude-opus-4-8", 7), "opus-4…");
        // Parts without letters can still be dropped on the way to one with them.
        assert_eq!(fit_model("gpt-5.6-terra", 5), "terra");
        // An id with no letters at all has nothing to protect: it just truncates.
        assert_eq!(fit_model("4-8-12", 3), "4-…");
        assert_eq!(
            model_row(Some("claude-opus-4-8"), Some("low"), 13).as_deref(),
            Some("opus-4… · low")
        );
    }

    #[test]
    fn row_keeps_the_effort_word_whole_while_it_fits() {
        for model in ["claude-sonnet-5-5", "gpt-5.6-terra", "abcdefghij"] {
            for effort in ["low", "medium", "high"] {
                for width in effort.width() + 3..=30 {
                    let row = model_row(Some(model), Some(effort), width).unwrap();
                    assert!(row.width() <= width, "{row:?} overflows {width}");
                    assert!(row.ends_with(effort), "{row:?} cut {effort} at {width}");
                }
            }
        }
    }

    #[test]
    fn row_narrower_than_the_effort_part_truncates_whole() {
        // ` · high` is 7 columns: at 6 it cannot stand, so the row reads from the
        // start like any other truncated text.
        let row = model_row(Some("claude-sonnet-5-5"), Some("high"), 6);
        assert_eq!(row.as_deref(), Some("claud…"));
        // At exactly 7 the model has no room: the word stands without a separator.
        let row = model_row(Some("claude-sonnet-5-5"), Some("high"), 7);
        assert_eq!(row.as_deref(), Some("high"));
    }

    #[test]
    fn card_row_shows_model_and_effort_whole_when_they_fit() {
        // 15 + 7 and 13 + 9: both exactly the 22 columns inside a subagent card.
        assert_eq!(
            row_of(Some("claude-opus-4-8"), Some("high")),
            "claude-opus-4-8 · high"
        );
        assert_eq!(
            row_of(Some("gpt-5.6-terra"), Some("medium")),
            "gpt-5.6-terra · medium"
        );
    }

    #[test]
    fn card_row_shortens_the_model_and_not_the_effort() {
        assert_eq!(
            row_of(Some("claude-sonnet-5-5"), Some("high")),
            "sonnet-5-5 · high"
        );
    }

    #[test]
    fn card_row_shows_only_what_was_recorded() {
        assert_eq!(row_of(Some("claude-opus-4-8"), None), "claude-opus-4-8");
        assert_eq!(row_of(None, Some("high")), "high");
        // Neither: the description moves up, with no empty row or separator.
        assert_eq!(row_of(None, None), "research");
        assert_eq!(row_of(Some(""), Some("")), "research");
    }

    #[test]
    fn every_row_fits_at_world_size() {
        for (w, h) in [MAIN_NODE_DIMS, SUB_NODE_DIMS] {
            let node = AgentNode {
                description: Some("research".into()),
                ..sample_node()
            };
            let buf = render_node(&node, ratatui::layout::Rect::new(0, 0, w as u16, h as u16));
            assert!(inner_row(&buf, 1).contains("claude"), "title at {w}x{h}");
            assert!(inner_row(&buf, 2).contains("· high"), "model at {w}x{h}");
            assert!(inner_row(&buf, 3).contains("research"), "desc at {w}x{h}");
            assert!(inner_row(&buf, 4).contains("⚒ 3"), "tools at {w}x{h}");
            assert!(inner_row(&buf, 5).contains("done"), "footer at {w}x{h}");
        }
    }

    #[test]
    fn short_card_drops_the_footer_before_the_model_row() {
        let node = AgentNode {
            description: Some("research".into()),
            ..sample_node()
        };
        let buf = render_node(&node, ratatui::layout::Rect::new(0, 0, 26, 6));
        assert_eq!(inner_row(&buf, 2).trim_end(), "claude-opus-4-8 · high");
        assert!(inner_row(&buf, 4).contains("⚒ 3"));
        assert!(!(1..5).any(|y| inner_row(&buf, y).contains("done")));
    }

    #[test]
    fn two_row_card_draws_no_model_row() {
        let area = ratatui::layout::Rect::new(0, 0, 26, 4);
        for (description, second_row) in [(Some("research"), "research"), (None, "⚒ 3 · Bash")] {
            let bare = AgentNode {
                description: description.map(str::to_string),
                model: None,
                effort: None,
                ..sample_node()
            };
            let recorded = AgentNode {
                model: Some("claude-opus-4-8".into()),
                effort: Some("high".into()),
                ..bare.clone()
            };
            let buf = render_node(&recorded, area);
            assert!(inner_row(&buf, 1).contains("claude"));
            assert_eq!(inner_row(&buf, 2).trim_end(), second_row);
            assert_eq!(buf, render_node(&bare, area), "differs from a bare card");
        }
    }

    #[test]
    fn three_row_card_draws_the_model_row_under_the_title() {
        let area = ratatui::layout::Rect::new(0, 0, 26, 5);
        for (description, third_row) in [(Some("research"), "research"), (None, "⚒ 3 · Bash")] {
            let node = AgentNode {
                description: description.map(str::to_string),
                ..sample_node()
            };
            let buf = render_node(&node, area);
            assert!(inner_row(&buf, 1).contains("claude"));
            assert_eq!(inner_row(&buf, 2).trim_end(), "claude-opus-4-8 · high");
            assert_eq!(inner_row(&buf, 3).trim_end(), third_row);
        }
    }

    #[test]
    fn narrowest_card_stays_inside_its_border() {
        let area = ratatui::layout::Rect::new(0, 0, CELL_MIN_WIDTH, 7);
        let buf = render_into(area);
        assert_eq!(inner_row(&buf, 2), "claud…");
        assert_eq!(buf[(area.right() - 1, 2)].symbol(), "│");
    }

    #[test]
    fn card_level_renders_text() {
        let area = ratatui::layout::Rect::new(0, 0, 24, 6);
        let buf = render_into(area);
        let text: String = (0..area.width).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(
            text.contains("claude"),
            "card level must show title: {text}"
        );
    }

    #[test]
    fn cell_level_renders_status_fill() {
        use rataflow::Theme;
        use ratatui::style::Color;

        // Below the text threshold: solid status-colored fill, no border.
        let area = ratatui::layout::Rect::new(0, 0, 6, 2);
        let buf = render_into(area);
        let palette = Theme::default().palette();
        for y in 0..area.height {
            for x in 0..area.width {
                let cell = &buf[(x, y)];
                assert_eq!(cell.symbol(), " ", "cell level draws no text/border");
                assert_eq!(cell.style().bg, Some(palette.accent)); // Done = calm accent
                assert_ne!(cell.style().bg, Some(Color::Reset));
            }
        }
    }
}
