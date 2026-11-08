//! Ratatui widgets + ANSI-free plain frames for dump / tests.
//!
//! Every screen draws from the top-left as plain text: `label  value` status
//! rows, one thin line, the list, and a one-line key hint. No frame, no
//! backdrop, no logo, no centering. All plain_* dumps stay ANSI-free for CI
//! and show the same lines the live screen draws.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use super::state::{
    MenuState, PaletteEntry, PaletteState, PickerState, SettingRow, SettingsState, Tone,
};
use super::table::{self, PickTable, Styled};
use super::theme;
use crate::term::{label_lines, text_width, truncate, LabelLine};

/// The thin line under the status rows is never wider than this.
const RULE_MAX: usize = 60;

/// Plain dumps never lay out narrower than this.
const DUMP_MIN_COLS: usize = 20;

/// `label  value` header strings as rows. A line with no double space has no
/// label and prints whole.
fn status_pairs(header: &[String]) -> Vec<(&str, String)> {
    header
        .iter()
        .map(|h| match h.split_once("  ") {
            Some((k, v)) => (k.trim(), v.trim_start().to_string()),
            None => ("", h.clone()),
        })
        .collect()
}

/// Status rows as (label, value) text cut to `width`, via the shared
/// `label_lines` layout so every screen aligns the same way.
fn status_text(header: &[String], width: usize) -> Vec<(String, String)> {
    if header.is_empty() {
        return vec![(String::new(), "—".into())];
    }
    let pairs = status_pairs(header);
    let labeled: Vec<(&str, String)> = pairs
        .iter()
        .filter(|(k, _)| !k.is_empty())
        .cloned()
        .collect();
    let mut laid = label_lines(&[&labeled], Some(width))
        .into_iter()
        .filter_map(|l| match l {
            LabelLine::Row { label, value } => Some((label, value)),
            LabelLine::Divider(_) => None,
        });
    pairs
        .iter()
        .map(|(k, v)| {
            if k.is_empty() {
                (String::new(), truncate(v, width))
            } else {
                laid.next().unwrap_or_default()
            }
        })
        .collect()
}

fn status_lines(header: &[String], width: usize) -> Vec<Line<'static>> {
    status_text(header, width)
        .into_iter()
        .map(|(label, value)| {
            if label.is_empty() {
                Line::from(Span::styled(value, theme::white()))
            } else {
                Line::from(vec![
                    Span::styled(label, theme::muted()),
                    Span::raw("  "),
                    Span::styled(value, theme::white()),
                ])
            }
        })
        .collect()
}

fn status_plain(header: &[String], width: usize) -> Vec<String> {
    status_text(header, width)
        .into_iter()
        .map(|(label, value)| {
            if label.is_empty() {
                value
            } else {
                format!("{label}  {value}")
            }
        })
        .collect()
}

fn rule_text(width: usize) -> String {
    "─".repeat(width.min(RULE_MAX))
}

fn rule(width: u16) -> Paragraph<'static> {
    Paragraph::new(Span::styled(rule_text(width as usize), theme::muted()))
}

fn hint(text: &str, width: u16) -> Paragraph<'static> {
    Paragraph::new(Span::styled(truncate(text, width as usize), theme::muted()))
}

fn title_line(title: &str) -> Line<'static> {
    Line::from(Span::styled(title.to_string(), theme::title()))
}

/// The top-left block a screen draws in: full width, as tall as its content.
fn top_left(area: Rect, height: u16) -> Rect {
    Rect::new(area.x, area.y, area.width, height.min(area.height).max(1))
}

fn split(area: Rect, rows: &[u16], grow: usize) -> Vec<Rect> {
    let constraints: Vec<Constraint> = rows
        .iter()
        .enumerate()
        .map(|(i, h)| {
            if i == grow {
                Constraint::Min(*h)
            } else {
                Constraint::Length(*h)
            }
        })
        .collect();
    Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area)
        .to_vec()
}

/// Column where values start: two spaces past the widest label.
fn value_column(labels: impl Iterator<Item = usize>) -> usize {
    labels.max().unwrap_or(0) + 2
}

// Picker: title, header, search, line, list, gap, hint.
const PK_HEAD: usize = 0;
const PK_SEARCH: usize = 1;
const PK_RULE: usize = 2;
const PK_LIST: usize = 3;
const PK_HINT: usize = 5;

fn picker_chunks(area: Rect, state: &PickerState) -> Vec<Rect> {
    split(
        area,
        &[1 + state.header.len() as u16, 1, 1, 1, 1, 1],
        PK_LIST,
    )
}

pub fn render_picker(frame: &mut Frame, state: &PickerState) {
    if let Some(t) = &state.table {
        return render_table_picker(frame, state, t);
    }
    let area = frame.area();
    let chunks = picker_chunks(area, state);
    let width = area.width as usize;

    let mut head = vec![title_line(&state.title)];
    for h in &state.header {
        head.push(Line::from(Span::styled(truncate(h, width), theme::muted())));
    }
    frame.render_widget(Paragraph::new(head), chunks[PK_HEAD]);

    let search = Paragraph::new(Line::from(vec![
        Span::styled("⌕ ", theme::accent()),
        Span::styled(state.query.clone(), theme::white()),
        Span::styled("█", theme::accent()),
    ]));
    frame.render_widget(search, chunks[PK_SEARCH]);
    frame.render_widget(rule(area.width), chunks[PK_RULE]);

    let filtered = state.filtered();
    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, (_, label))| {
            let selected = i == state.cursor;
            let icon = item_icon(label);
            let room = width.saturating_sub(2 + text_width(icon));
            ListItem::new(Line::from(vec![
                Span::styled(if selected { "❯ " } else { "  " }, theme::accent()),
                Span::styled(icon, item_icon_style(label)),
                Span::styled(
                    truncate(label, room),
                    if selected {
                        theme::selected()
                    } else {
                        theme::white()
                    },
                ),
            ]))
        })
        .collect();
    let mut list_state = ListState::default()
        .with_selected(Some(state.cursor.min(filtered.len().saturating_sub(1))));
    frame.render_stateful_widget(List::new(items), chunks[PK_LIST], &mut list_state);
    frame.render_widget(hint(state.hint(), area.width), chunks[PK_HINT]);
}

// Table picker: title, header, search, line, column titles, list, gap, then
// detail, gap and legend when the terminal is tall enough, then keys.
const TB_SEARCH: usize = 1;
const TB_RULE: usize = 2;
const TB_COLS: usize = 3;
const TB_LIST: usize = 4;
const TB_DETAIL: usize = 6;
const TB_LEGEND: usize = 8;
/// The list keeps at least this many rows before the detail block hides.
const TB_LIST_MIN: u16 = 5;

fn detail_height(t: &PickTable) -> u16 {
    t.details.iter().map(Vec::len).max().unwrap_or(0) as u16
}

/// Layout rects, and whether the detail block and legend fit.
fn table_chunks(area: Rect, state: &PickerState, t: &PickTable) -> (Vec<Rect>, bool) {
    let head = 1 + state.header.len() as u16;
    let full = [head, 1, 1, 1, TB_LIST_MIN, 1, detail_height(t), 1, 1, 1];
    if area.height >= full.iter().sum::<u16>() {
        (split(area, &full, TB_LIST), true)
    } else {
        (split(area, &[head, 1, 1, 1, 1, 1, 1], TB_LIST), false)
    }
}

/// First filtered row on screen, rows shown, and whether more sit below.
fn table_window(state: &PickerState, rows: usize) -> (usize, usize, bool) {
    let n = state.filtered().len();
    if n <= rows {
        return (0, n, false);
    }
    let (start, vis) = palette_window(state.cursor, n, rows.saturating_sub(1).max(1));
    (start, vis, start + vis < n)
}

fn styled_line(styled: &Styled) -> Line<'static> {
    Line::from(
        styled
            .iter()
            .map(|(text, tone)| Span::styled(text.clone(), tone_style(*tone)))
            .collect::<Vec<_>>(),
    )
}

fn render_table_picker(frame: &mut Frame, state: &PickerState, t: &PickTable) {
    let area = frame.area();
    let width = area.width as usize;
    let (chunks, detail) = table_chunks(area, state, t);

    let mut head = vec![title_line(&state.title)];
    for h in &state.header {
        head.push(Line::from(Span::styled(truncate(h, width), theme::muted())));
    }
    frame.render_widget(Paragraph::new(head), chunks[0]);

    let mut search = vec![Span::styled("⌕ ", theme::accent())];
    if state.query.is_empty() {
        search.push(Span::styled("█", theme::accent()));
        search.push(Span::styled(" type to search", theme::muted()));
    } else {
        search.push(Span::styled(state.query.clone(), theme::white()));
        search.push(Span::styled("█", theme::accent()));
    }
    frame.render_widget(Paragraph::new(Line::from(search)), chunks[TB_SEARCH]);
    frame.render_widget(rule(area.width), chunks[TB_RULE]);

    let widths = table::layout(t, width);
    frame.render_widget(
        Paragraph::new(Span::styled(
            truncate(&format!("  {}", table::header(t, &widths)), width),
            theme::muted(),
        )),
        chunks[TB_COLS],
    );

    let filtered = state.filtered();
    let list = chunks[TB_LIST];
    let mut lines: Vec<Line> = Vec::new();
    if filtered.is_empty() {
        lines.push(Line::from(Span::styled("  no matches", theme::muted())));
    }
    let (start, vis, more) = table_window(state, list.height as usize);
    for (i, (item, _)) in filtered.iter().enumerate().skip(start).take(vis) {
        let selected = i == state.cursor;
        let mut spans = vec![Span::styled(
            if selected { "❯ " } else { "  " },
            theme::accent(),
        )];
        for (n, (text, tone)) in table::row(t, *item, &widths).into_iter().enumerate() {
            let style = match (n, selected) {
                (0, true) => theme::selected(),
                (0, false) => theme::white(),
                (_, true) => theme::on_selected(tone_style(tone)),
                (_, false) => tone_style(tone),
            };
            spans.push(Span::styled(text, style));
        }
        lines.push(Line::from(spans));
    }
    if more {
        lines.push(Line::from(Span::styled("  ↓ more below", theme::muted())));
    }
    frame.render_widget(Paragraph::new(lines), list);

    if detail {
        let current = filtered.get(state.cursor).map(|(i, _)| *i);
        let rows: Vec<Line> = current
            .and_then(|i| t.details.get(i))
            .into_iter()
            .flatten()
            .map(|l| styled_line(&table::fit(l, width)))
            .collect();
        frame.render_widget(Paragraph::new(rows), chunks[TB_DETAIL]);
        frame.render_widget(
            Paragraph::new(styled_line(&table::fit(&t.legend, width))),
            chunks[TB_LEGEND],
        );
    }
    frame.render_widget(hint(state.hint(), area.width), chunks[chunks.len() - 1]);
}

// Menu: title, status, line, items, gap, hint.
const MN_TITLE: usize = 0;
const MN_STATUS: usize = 1;
const MN_RULE: usize = 2;
const MN_LIST: usize = 3;
const MN_HINT: usize = 5;

fn menu_chunks(area: Rect, state: &MenuState) -> Vec<Rect> {
    let status = state.header.len().max(1) as u16;
    let items = state.items.len().max(1) as u16;
    let rows = [1, status, 1, items, 1, 1];
    let height = rows.iter().sum();
    split(top_left(area, height), &rows, MN_LIST)
}

pub fn render_menu(frame: &mut Frame, state: &MenuState) {
    let area = frame.area();
    let chunks = menu_chunks(area, state);
    let width = area.width as usize;
    frame.render_widget(Paragraph::new(title_line(&state.title)), chunks[MN_TITLE]);
    frame.render_widget(
        Paragraph::new(status_lines(&state.header, width)),
        chunks[MN_STATUS],
    );
    frame.render_widget(rule(area.width), chunks[MN_RULE]);

    let items: Vec<ListItem> = state
        .items
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let selected = i == state.cursor;
            let icon = item_icon(label);
            let room = width.saturating_sub(2 + text_width(icon));
            ListItem::new(Line::from(vec![
                Span::styled(if selected { "❯ " } else { "  " }, theme::accent()),
                Span::styled(icon, item_icon_style(label)),
                Span::styled(
                    truncate(label, room),
                    if selected {
                        theme::selected()
                    } else {
                        theme::white()
                    },
                ),
            ]))
        })
        .collect();
    let mut list_state = ListState::default().with_selected(Some(state.cursor));
    frame.render_stateful_widget(List::new(items), chunks[MN_LIST], &mut list_state);
    frame.render_widget(hint(state.hint(), area.width), chunks[MN_HINT]);
}

// Palette: status, line, input, results, gap, hint.
const PL_STATUS: usize = 0;
const PL_RULE: usize = 1;
const PL_INPUT: usize = 2;
const PL_RESULTS: usize = 3;
const PL_HINT: usize = 5;

/// Command palette (the launcher): account rows, a line, the filter input,
/// then agents and actions grouped under plain headers.
pub fn render_palette(frame: &mut Frame, state: &PaletteState) {
    let area = frame.area();
    let (chunks, visible) = palette_chunks(area, state);
    let width = area.width as usize;

    frame.render_widget(
        Paragraph::new(status_lines(&state.header, width)),
        chunks[PL_STATUS],
    );
    frame.render_widget(rule(area.width), chunks[PL_RULE]);

    let input = Paragraph::new(Line::from(vec![
        Span::styled("❯ ", theme::accent()),
        Span::styled(state.query.clone(), theme::white()),
        Span::styled("█", theme::accent()),
    ]));
    frame.render_widget(input, chunks[PL_INPUT]);

    let filtered = state.filtered();
    if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("no matches", theme::muted())),
            chunks[PL_RESULTS],
        );
    } else {
        let rows = palette_rows(state, &filtered, visible, width);
        frame.render_widget(Paragraph::new(rows), chunks[PL_RESULTS]);
    }
    frame.render_widget(hint(state.hint(), area.width), chunks[PL_HINT]);
}

/// Number of distinct groups among the first `visible` filtered entries —
/// each renders one header line inside the result area.
fn palette_groups(state: &PaletteState, filtered: &[usize], visible: usize) -> usize {
    let (start, vis) = palette_window(state.cursor, filtered.len(), visible.max(1));
    let mut groups: Vec<&str> = Vec::new();
    for &entry_i in filtered.iter().skip(start).take(vis) {
        let g = state.entries[entry_i].group.as_str();
        if g.is_empty() {
            continue;
        }
        if !groups.contains(&g) {
            groups.push(g);
        }
    }
    groups.len()
}

/// One palette row as text parts: marker, icon, label padded to the value
/// column, detail cut to the width.
fn palette_row_parts(
    entry: &PaletteEntry,
    selected: bool,
    label_col: usize,
    width: usize,
) -> (&'static str, &'static str, String, String) {
    let marker = if selected { "❯ " } else { "  " };
    let icon = item_icon(&entry.label);
    let lead = 2 + text_width(icon);
    let label = truncate(&entry.label, width.saturating_sub(lead));
    let pad = label_col.saturating_sub(text_width(&label)).max(2);
    let padded = format!("{label}{}", " ".repeat(pad));
    let room = width.saturating_sub(lead + text_width(&padded));
    let detail = if entry.detail.is_empty() || room == 0 {
        String::new()
    } else {
        truncate(&entry.detail, room)
    };
    (marker, icon, padded, detail)
}

fn palette_label_col(state: &PaletteState, filtered: &[usize]) -> usize {
    value_column(
        filtered
            .iter()
            .map(|&i| text_width(&state.entries[i].label)),
    )
}

/// Build the result lines, inserting a group header whenever it changes and
/// lining every detail up in one column.
fn palette_rows(
    state: &PaletteState,
    filtered: &[usize],
    visible: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let (start, vis) = palette_window(state.cursor, filtered.len(), visible.max(1));
    let label_col = palette_label_col(state, filtered);
    let mut rows: Vec<Line> = Vec::new();
    let mut last_group: Option<&str> = None;
    for (row_i, &entry_i) in filtered.iter().skip(start).take(vis).enumerate() {
        let entry: &PaletteEntry = &state.entries[entry_i];
        if last_group != Some(entry.group.as_str()) {
            if last_group.is_some() {
                rows.push(Line::from(""));
            }
            if !entry.group.is_empty() {
                rows.push(Line::from(Span::styled(
                    truncate(&format!("  {}", entry.group), width),
                    theme::muted(),
                )));
            }
            last_group = Some(&entry.group);
        }
        let selected = start + row_i == state.cursor;
        let (marker, icon, label, detail) = palette_row_parts(entry, selected, label_col, width);
        rows.push(Line::from(vec![
            Span::styled(
                marker,
                if selected {
                    theme::accent()
                } else {
                    theme::muted()
                },
            ),
            Span::styled(icon, item_icon_style(&entry.label)),
            Span::styled(
                label,
                if selected {
                    theme::selected()
                } else {
                    theme::white()
                },
            ),
            Span::styled(detail, theme::muted()),
        ]));
    }
    rows
}

// Settings: title, status, tabs, line, rows, gap, hint.
const ST_TITLE: usize = 0;
const ST_STATUS: usize = 1;
const ST_TABS: usize = 2;
const ST_RULE: usize = 3;
const ST_ROWS: usize = 4;
const ST_HINT: usize = 6;

fn settings_chunks(area: Rect, state: &SettingsState) -> Vec<Rect> {
    let status = state.header.len().max(1) as u16;
    let tabs = if state.tabs.len() > 1 { 1 } else { 0 };
    let rows_h = state.rows.len().max(1) as u16;
    let rows = [1, status, tabs, 1, rows_h, 1, 1];
    let height = rows.iter().sum();
    split(top_left(area, height), &rows, ST_ROWS)
}

/// Settings screen: grouped rows, each value in one column after the labels.
pub fn render_settings(frame: &mut Frame, state: &SettingsState) {
    let area = frame.area();
    let chunks = settings_chunks(area, state);
    let width = area.width as usize;
    frame.render_widget(Paragraph::new(title_line(&state.title)), chunks[ST_TITLE]);
    frame.render_widget(
        Paragraph::new(status_lines(&state.header, width)),
        chunks[ST_STATUS],
    );
    if state.tabs.len() > 1 {
        frame.render_widget(Paragraph::new(tab_bar_line(state)), chunks[ST_TABS]);
    }
    frame.render_widget(rule(area.width), chunks[ST_RULE]);

    let label_col = settings_label_col(state);
    let lines: Vec<Line> = state
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| settings_row_line(row, i == state.cursor, label_col, width))
        .collect();
    frame.render_widget(Paragraph::new(lines), chunks[ST_ROWS]);
    frame.render_widget(hint(state.hint(), area.width), chunks[ST_HINT]);
}

fn tab_bar_line(state: &SettingsState) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    for (i, name) in state.tabs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", theme::muted()));
        }
        if i == state.tab {
            spans.push(Span::styled(format!("[{name}]"), theme::brand()));
        } else {
            spans.push(Span::styled(name.clone(), theme::muted()));
        }
    }
    Line::from(spans)
}

fn settings_label_col(state: &SettingsState) -> usize {
    value_column(state.rows.iter().filter_map(|r| match r {
        SettingRow::Entry { label, .. } => Some(text_width(label)),
        _ => None,
    }))
}

fn tone_style(tone: Tone) -> ratatui::style::Style {
    match tone {
        Tone::Normal => theme::white(),
        Tone::Good => theme::success(),
        Tone::Model => theme::model(),
        Tone::Warn => theme::warn(),
        Tone::Muted => theme::muted(),
    }
}

/// One settings row as text: marker, label padded to the value column, value
/// cut to the width.
fn settings_row_text(
    label: &str,
    value: &str,
    selected: bool,
    label_col: usize,
    width: usize,
) -> (&'static str, String, String) {
    let marker = if selected { "❯ " } else { "  " };
    let label = truncate(label, width.saturating_sub(2));
    let pad = label_col.saturating_sub(text_width(&label)).max(2);
    let padded = format!("{label}{}", " ".repeat(pad));
    let room = width.saturating_sub(2 + text_width(&padded));
    (marker, padded, truncate(value, room))
}

fn settings_row_line(
    row: &SettingRow,
    selected: bool,
    label_col: usize,
    width: usize,
) -> Line<'static> {
    match row {
        SettingRow::Section(name) => Line::from(Span::styled(
            truncate(&format!("  {}", name.to_ascii_uppercase()), width),
            theme::muted(),
        )),
        SettingRow::Gap => Line::from(""),
        SettingRow::Entry { label, value, tone } => {
            let (marker, label, value) =
                settings_row_text(label, value, selected, label_col, width);
            Line::from(vec![
                Span::styled(
                    marker,
                    if selected {
                        theme::accent()
                    } else {
                        theme::muted()
                    },
                ),
                Span::styled(
                    label,
                    if selected {
                        theme::selected()
                    } else {
                        theme::white()
                    },
                ),
                Span::styled(value, tone_style(*tone)),
            ])
        }
    }
}

fn in_rect(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x
        && row >= r.y
        && col < r.x.saturating_add(r.width)
        && row < r.y.saturating_add(r.height)
}

/// Live palette shows as many filtered rows as the current height allows.
/// The window follows the cursor so Enter never selects a row that is off-screen.
/// Group headers and the gap lines between groups count against the height.
fn palette_visible(area: Rect, state: &PaletteState, status: u16) -> (usize, usize, usize) {
    let filtered = state.filtered();
    let avail = area.height.saturating_sub(status + 4).max(1) as usize;
    let mut visible = filtered.len().min(avail).max(1);
    for _ in 0..4 {
        let groups = palette_groups(state, &filtered, visible);
        let overhead = 2 * groups.saturating_sub(1);
        let capped = filtered.len().min(avail.saturating_sub(overhead).max(1));
        if capped == visible {
            return (visible, groups, overhead);
        }
        visible = capped;
    }
    let groups = palette_groups(state, &filtered, visible);
    (visible, groups, 2 * groups.saturating_sub(1))
}

fn palette_window(cursor: usize, n: usize, max: usize) -> (usize, usize) {
    if n == 0 {
        return (0, 0);
    }
    let vis = n.min(max.max(1));
    let cursor = cursor.min(n - 1);
    let start = if n <= vis {
        0
    } else {
        cursor.saturating_sub(vis - 1).min(n - vis)
    };
    (start, vis)
}

fn palette_chunks(area: Rect, state: &PaletteState) -> (Vec<Rect>, usize) {
    let status = state.header.len().max(1) as u16;
    let (visible, _groups, overhead) = palette_visible(area, state, status);
    let results = (visible.max(1) + overhead) as u16;
    let rows = [status, 1, 1, results, 1, 1];
    (split(area, &rows, PL_RESULTS), visible)
}

/// Filtered-row index under the cell, skipping group headers and gaps.
pub fn hit_palette(area: Rect, state: &PaletteState, col: u16, row: u16) -> Option<usize> {
    let (chunks, visible) = palette_chunks(area, state);
    let results = chunks[PL_RESULTS];
    if !in_rect(results, col, row) {
        return None;
    }
    let dy = (row - results.y) as usize;
    palette_hit_map(state, visible).get(dy).copied().flatten()
}

fn palette_hit_map(state: &PaletteState, visible: usize) -> Vec<Option<usize>> {
    let filtered = state.filtered();
    let (start, vis) = palette_window(state.cursor, filtered.len(), visible.max(1));
    let mut map = Vec::new();
    let mut last_group: Option<&str> = None;
    for (row_i, &entry_i) in filtered.iter().skip(start).take(vis).enumerate() {
        let entry = &state.entries[entry_i];
        if last_group != Some(entry.group.as_str()) {
            if last_group.is_some() {
                map.push(None);
            }
            if !entry.group.is_empty() {
                map.push(None);
            }
            last_group = Some(entry.group.as_str());
        }
        map.push(Some(start + row_i));
    }
    map
}

pub fn hit_menu(area: Rect, state: &MenuState, col: u16, row: u16) -> Option<usize> {
    let list = menu_chunks(area, state)[MN_LIST];
    if !in_rect(list, col, row) {
        return None;
    }
    let i = (row - list.y) as usize;
    (i < state.items.len()).then_some(i)
}

pub fn hit_picker(area: Rect, state: &PickerState, col: u16, row: u16) -> Option<usize> {
    if let Some(t) = &state.table {
        let list = table_chunks(area, state, t).0[TB_LIST];
        if !in_rect(list, col, row) {
            return None;
        }
        let (start, vis, _) = table_window(state, list.height as usize);
        let i = (row - list.y) as usize;
        return (i < vis).then_some(start + i);
    }
    let list = picker_chunks(area, state)[PK_LIST];
    if !in_rect(list, col, row) {
        return None;
    }
    let i = (row - list.y) as usize;
    (i < state.filtered().len()).then_some(i)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsHit {
    Row(usize),
    Tab(usize),
}

pub fn hit_settings(area: Rect, state: &SettingsState, col: u16, row: u16) -> Option<SettingsHit> {
    let chunks = settings_chunks(area, state);
    if state.tabs.len() > 1 {
        if let Some(tab) = settings_tab_at(state, chunks[ST_TABS], col, row) {
            return Some(SettingsHit::Tab(tab));
        }
    }
    let list = chunks[ST_ROWS];
    if !in_rect(list, col, row) {
        return None;
    }
    let i = (row - list.y) as usize;
    match state.rows.get(i) {
        Some(SettingRow::Entry { .. }) => Some(SettingsHit::Row(i)),
        _ => None,
    }
}

fn settings_tab_at(state: &SettingsState, bar: Rect, col: u16, row: u16) -> Option<usize> {
    if !in_rect(bar, col, row) {
        return None;
    }
    let mut x = bar.x;
    for (i, name) in state.tabs.iter().enumerate() {
        if i > 0 {
            x = x.saturating_add(2);
        }
        let label_w = if i == state.tab {
            name.chars().count() + 2
        } else {
            name.chars().count()
        } as u16;
        if col >= x && col < x.saturating_add(label_w) {
            return Some(i);
        }
        x = x.saturating_add(label_w);
    }
    None
}

/// Geometric row prefix (› for launch rows). No emoji.
pub fn item_icon(label: &str) -> &'static str {
    let l = label.to_ascii_lowercase();
    let stem = l.trim_end_matches('…').trim();
    if label.starts_with("Launch")
        || stem == "agent"
        || crate::registry::LAUNCHERS.contains(&stem)
        || l.contains("claude")
        || l.contains("codex")
    {
        "› "
    } else {
        ""
    }
}

fn item_icon_style(label: &str) -> ratatui::style::Style {
    let l = label.to_ascii_lowercase();
    if label.starts_with("Launch") {
        theme::success()
    } else if l.contains("quit") || l.contains("done") {
        theme::muted()
    } else if l.contains("credit") {
        theme::model()
    } else {
        theme::accent()
    }
}

fn dump_width(cols: usize) -> usize {
    cols.max(DUMP_MIN_COLS)
}

/// ANSI-free plain frame for `--dump-tui` and unit tests.
pub fn plain_picker_lines(state: &PickerState, cols: usize) -> Vec<String> {
    if let Some(t) = &state.table {
        return plain_table_picker_lines(state, t, cols);
    }
    let width = dump_width(cols);
    let mut lines = vec![truncate(&state.title, width)];
    for h in &state.header {
        lines.push(truncate(h, width));
    }
    lines.push(truncate(&format!("search: {}", state.query), width));
    lines.push(rule_text(width));
    let filtered = state.filtered();
    if filtered.is_empty() {
        lines.push(truncate("  (no matches)", width));
    } else {
        for (i, (_, label)) in filtered.iter().enumerate() {
            let marker = if i == state.cursor { "❯" } else { " " };
            lines.push(truncate(&format!("{marker} {label}"), width));
        }
    }
    lines.push(String::new());
    lines.push(truncate(state.hint(), width));
    lines
}

fn plain_table_picker_lines(state: &PickerState, t: &PickTable, cols: usize) -> Vec<String> {
    let width = dump_width(cols);
    let mut lines = vec![truncate(&state.title, width)];
    for h in &state.header {
        lines.push(truncate(h, width));
    }
    lines.push(truncate(&format!("search: {}", state.query), width));
    lines.push(rule_text(width));
    let widths = table::layout(t, width);
    lines.push(truncate(&format!("  {}", table::header(t, &widths)), width));
    let filtered = state.filtered();
    if filtered.is_empty() {
        lines.push(truncate("  no matches", width));
    }
    for (i, (item, _)) in filtered.iter().enumerate() {
        let marker = if i == state.cursor { "❯" } else { " " };
        let row = table::plain(&table::row(t, *item, &widths));
        lines.push(format!("{marker} {}", row.trim_end()));
    }
    lines.push(String::new());
    if let Some((item, _)) = filtered.get(state.cursor) {
        for l in t.details.get(*item).into_iter().flatten() {
            lines.push(table::plain(&table::fit(l, width)));
        }
        lines.push(String::new());
    }
    lines.push(table::plain(&table::fit(&t.legend, width)));
    lines.push(truncate(state.hint(), width));
    lines
}

pub fn plain_menu_lines(state: &MenuState, cols: usize) -> Vec<String> {
    let width = dump_width(cols);
    let mut lines = vec![truncate(&state.title, width)];
    lines.extend(status_plain(&state.header, width));
    lines.push(rule_text(width));
    for (i, label) in state.items.iter().enumerate() {
        let marker = if i == state.cursor { "❯" } else { " " };
        lines.push(truncate(&format!("{marker} {label}"), width));
    }
    lines.push(String::new());
    lines.push(truncate(state.hint(), width));
    lines
}

pub fn plain_picker_frame(state: &PickerState, cols: usize) -> String {
    let mut lines = plain_picker_lines(state, cols);
    lines.push(String::new());
    lines.join("\n")
}

pub fn plain_menu_frame(state: &MenuState, cols: usize) -> String {
    let mut lines = plain_menu_lines(state, cols);
    lines.push(String::new());
    lines.join("\n")
}

/// The launcher as plain lines: the same rows the live palette draws, with
/// every filtered entry (a dump is a snapshot, not a viewport).
pub fn plain_palette_lines(state: &PaletteState, cols: usize) -> Vec<String> {
    let width = dump_width(cols);
    let mut lines = status_plain(&state.header, width);
    lines.push(rule_text(width));
    lines.push(truncate(&format!("❯ {}", state.query), width));

    let filtered = state.filtered();
    if filtered.is_empty() {
        lines.push(truncate("  (no matches)", width));
    } else {
        let cursor_row = state.cursor.min(filtered.len().saturating_sub(1));
        let label_col = palette_label_col(state, &filtered);
        let mut last_group: Option<&str> = None;
        for (row_i, &entry_i) in filtered.iter().enumerate() {
            let entry = &state.entries[entry_i];
            if last_group != Some(entry.group.as_str()) {
                if last_group.is_some() {
                    lines.push(String::new());
                }
                if !entry.group.is_empty() {
                    lines.push(truncate(&format!("  {}", entry.group), width));
                }
                last_group = Some(&entry.group);
            }
            let (marker, icon, label, detail) =
                palette_row_parts(entry, row_i == cursor_row, label_col, width);
            lines.push(
                format!("{marker}{icon}{label}{detail}")
                    .trim_end()
                    .to_string(),
            );
        }
    }
    lines.push(String::new());
    lines.push(truncate(state.hint(), width));
    lines
}

pub fn plain_palette_frame(state: &PaletteState, cols: usize) -> String {
    let mut lines = plain_palette_lines(state, cols);
    lines.push(String::new());
    lines.join("\n")
}

/// ANSI-free settings frame for `--dump-tui` and unit tests.
pub fn plain_settings_lines(state: &SettingsState, cols: usize) -> Vec<String> {
    let width = dump_width(cols);
    let mut lines = vec![truncate(&state.title, width)];
    lines.extend(status_plain(&state.header, width));
    if state.tabs.len() > 1 {
        let bar = state
            .tabs
            .iter()
            .enumerate()
            .map(|(i, name)| {
                if i == state.tab {
                    format!("[{name}]")
                } else {
                    name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(truncate(&bar, width));
    }
    lines.push(rule_text(width));
    let label_col = settings_label_col(state);
    for (i, row) in state.rows.iter().enumerate() {
        lines.push(match row {
            SettingRow::Section(name) => {
                truncate(&format!("  {}", name.to_ascii_uppercase()), width)
            }
            SettingRow::Gap => String::new(),
            SettingRow::Entry { label, value, .. } => {
                let (marker, label, value) =
                    settings_row_text(label, value, i == state.cursor, label_col, width);
                format!("{marker}{label}{value}").trim_end().to_string()
            }
        });
    }
    lines.push(String::new());
    lines.push(truncate(state.hint(), width));
    lines
}

pub fn plain_settings_frame(state: &SettingsState, cols: usize) -> String {
    let mut lines = plain_settings_lines(state, cols);
    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every dumped line starts near column 0 and fits the width: the
    /// owner's plain top-left rule, with no frame characters.
    fn assert_plain(frame: &str, cols: usize) {
        assert!(!frame.contains('\u{1b}'), "must be ANSI-free: {frame}");
        for line in frame.lines() {
            assert!(
                text_width(line) <= cols,
                "line wider than {cols}: {line:?}\n{frame}"
            );
            let indent = line.chars().take_while(|c| *c == ' ').count();
            assert!(indent <= 4, "indented {indent}: {line:?}\n{frame}");
            for c in ['╭', '╮', '╰', '╯', '│', '├', '┤'] {
                assert!(!line.contains(c), "frame char {c} in {line:?}");
            }
        }
    }

    #[test]
    fn palette_window_keeps_cursor_visible() {
        // WHY: Enter must select the highlighted row, not a row below the fold.
        assert_eq!(palette_window(0, 20, 10), (0, 10));
        assert_eq!(palette_window(15, 20, 10), (6, 10));
        assert_eq!(palette_window(19, 20, 10), (10, 10));
        assert_eq!(palette_window(0, 3, 10), (0, 3));
        assert_eq!(palette_window(0, 0, 10), (0, 0));
    }

    #[test]
    fn dump_menu_is_plain_text_from_the_top_left() {
        let state = MenuState::new(
            "AnyRouter",
            vec!["account  default".into()],
            vec!["Launch claude".into(), "Config".into(), "Quit".into()],
        );
        for cols in [32, 80] {
            let frame = plain_menu_frame(&state, cols);
            assert_plain(&frame, cols);
            assert!(frame.starts_with("AnyRouter\n"), "{frame}");
            assert!(frame.contains("account  default"), "{frame}");
            assert!(frame.contains("❯ Launch claude"), "{frame}");
            assert!(
                frame.contains("  Config") && frame.contains("  Quit"),
                "{frame}"
            );
        }
    }

    #[test]
    fn dump_picker_shows_search() {
        let mut state = PickerState::new("Model", vec!["a".into(), "b".into()], Some(0));
        state.query = "a".into();
        let frame = plain_picker_frame(&state, 80);
        assert!(frame.contains("search: a"));
        assert_plain(&frame, 80);
    }

    #[test]
    fn icons_cover_launcher_actions() {
        assert_eq!(item_icon("Launch claude").trim(), "›");
        assert_eq!(item_icon("claude").trim(), "›");
        assert_eq!(item_icon("grok").trim(), "›");
        assert_eq!(item_icon("Config"), "");
        assert_eq!(item_icon("Quit"), "");
        assert_eq!(item_icon("model…"), "");
        assert!(!item_icon("claude").contains("⚡"));
    }

    fn launcher_state() -> PaletteState {
        PaletteState::new(
            vec![
                "account  duyet · me@example.co".into(),
                "credits  $12.50".into(),
            ],
            vec![
                PaletteEntry::new(
                    "claude",
                    "stealth/ox-alpha · …wxyz",
                    "launch",
                    "Launch claude",
                ),
                PaletteEntry::new("codex", "auto · …wxyz", "launch", "Launch codex"),
                PaletteEntry::new("model…", "for claude", "configure · claude", "Switch model"),
                PaletteEntry::new("quit", "esc works too", "more", "Quit"),
            ],
        )
    }

    #[test]
    fn dump_palette_is_status_rows_a_line_and_the_agent_list() {
        let state = launcher_state();
        let frame = plain_palette_frame(&state, 80);
        assert_plain(&frame, 80);
        let lines: Vec<&str> = frame.lines().collect();
        // WHY: account and credits first, from row 0, then one thin line.
        assert_eq!(lines[0], "account  duyet · me@example.co", "{frame}");
        assert_eq!(lines[1], "credits  $12.50", "{frame}");
        assert!(lines[2].chars().all(|c| c == '─'), "{frame}");
        assert!(
            !frame.contains(crate::term::TUI_MARK_LINES[1].trim()),
            "no logo:\n{frame}"
        );
        // Details line up in one column after the longest label.
        let col = |needle: &str| {
            let line = lines.iter().find(|l| l.contains(needle)).unwrap();
            line[..line.find(needle).unwrap()].chars().count()
        };
        assert_eq!(col("stealth/ox-alpha"), col("auto · …wxyz"), "{frame}");
        assert!(frame.contains("❯ › claude"), "{frame}");
        assert!(frame.contains("  configure · claude"), "{frame}");
        assert_eq!(*lines.last().unwrap(), hint_text(), "{frame}");
    }

    fn hint_text() -> &'static str {
        super::super::keys::hint_line(super::super::keys::Surface::Palette)
    }

    #[test]
    fn dump_palette_cuts_long_details_at_narrow_widths() {
        let mut state = launcher_state();
        state.entries[0].detail = "x".repeat(300);
        for cols in [40, 80, 200] {
            let frame = plain_palette_frame(&state, cols);
            assert_plain(&frame, cols);
            assert!(frame.contains('…'), "{cols}: long detail not cut:\n{frame}");
        }
    }

    #[test]
    fn dump_settings_is_plain_and_grouped() {
        use super::super::state::{SettingRow, SettingsState, Tone};
        let state = SettingsState::new(
            "Config",
            vec!["account  duyet · me@example.co".into()],
            vec![
                SettingRow::Section("Account".into()),
                SettingRow::Entry {
                    label: "account".into(),
                    value: "duyet".into(),
                    tone: Tone::Normal,
                },
                SettingRow::Gap,
                SettingRow::Section("Model".into()),
                SettingRow::Entry {
                    label: "default".into(),
                    value: "auto".into(),
                    tone: Tone::Model,
                },
            ],
        )
        .with_tabs(vec!["general".into(), "claude".into(), "codex".into()], 0);
        let frame = plain_settings_frame(&state, 80);
        assert_plain(&frame, 80);
        assert!(frame.starts_with("Config\n"), "{frame}");
        assert!(frame.contains("[general]"), "{frame}");
        assert!(
            frame.contains("ACCOUNT") && frame.contains("MODEL"),
            "{frame}"
        );
        assert!(frame.contains("❯ account  duyet"), "{frame}");
        // Blank line between ACCOUNT and MODEL sections.
        let dumped: Vec<&str> = frame.lines().collect();
        let acct = dumped.iter().position(|l| l.contains("ACCOUNT")).unwrap();
        let model = dumped.iter().position(|l| l.contains("MODEL")).unwrap();
        assert!(
            model > acct + 1,
            "expected padding between sections:\n{frame}"
        );
        // Values share one column after the labels, not the far right edge.
        assert!(frame.contains("  default  auto"), "{frame}");
        assert_plain(&plain_settings_frame(&state, 30), 30);
    }

    #[test]
    fn palette_click_skips_group_headers_and_hits_rows() {
        let state = PaletteState::new(
            vec!["account  default".into(), "credits  -".into()],
            vec![
                PaletteEntry::new("claude", "x", "launch", "Launch claude"),
                PaletteEntry::new("codex", "y", "launch", "Launch codex"),
                PaletteEntry::new("model…", "z", "configure", "Switch model"),
            ],
        );
        let area = Rect::new(0, 0, 80, 40);
        let (chunks, _) = palette_chunks(area, &state);
        let results = chunks[PL_RESULTS];
        // Status rows, the line and the input sit above the results.
        assert_eq!(results.y, 4);
        let x = results.x + 2;
        assert_eq!(hit_palette(area, &state, x, results.y), None);
        assert_eq!(hit_palette(area, &state, x, results.y + 1), Some(0));
        assert_eq!(hit_palette(area, &state, x, results.y + 2), Some(1));
        assert_eq!(hit_palette(area, &state, x, results.y + 3), None);
        assert_eq!(hit_palette(area, &state, x, results.y + 4), None);
        assert_eq!(hit_palette(area, &state, x, results.y + 5), Some(2));
        assert_eq!(hit_palette(area, &state, 0, 0), None);
    }

    #[test]
    fn menu_click_maps_row_index() {
        let state = MenuState::new(
            "AnyRouter",
            vec!["account  default".into()],
            vec!["Launch claude".into(), "Config".into(), "Quit".into()],
        );
        let area = Rect::new(0, 0, 80, 24);
        // Title, one status row and the line: the list starts on row 3.
        let y = 3;
        assert_eq!(hit_menu(area, &state, 2, y), Some(0));
        assert_eq!(hit_menu(area, &state, 2, y + 1), Some(1));
        assert_eq!(hit_menu(area, &state, 2, y + 2), Some(2));
        assert_eq!(hit_menu(area, &state, 2, y + 3), None);
        assert_eq!(hit_menu(area, &state, 0, 0), None);
    }

    #[test]
    fn live_palette_draws_from_the_top_left_without_a_frame() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        for cols in [40u16, 80, 200] {
            let mut term = Terminal::new(TestBackend::new(cols, 30)).unwrap();
            let state = launcher_state();
            term.draw(|f| render_palette(f, &state)).unwrap();
            let buf = term.backend().buffer().clone();
            let row = |y: u16| -> String {
                (0..cols)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect()
            };
            assert!(row(0).starts_with("account  duyet"), "{cols}: {:?}", row(0));
            assert!(
                row(1).starts_with("credits  $12.50"),
                "{cols}: {:?}",
                row(1)
            );
            for y in 0..30 {
                let r = row(y);
                assert!(
                    !r.contains('│') && !r.contains('╭'),
                    "{cols}: frame on row {y}: {r:?}"
                );
            }
        }
    }
}
