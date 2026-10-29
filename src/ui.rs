use crate::{
    app::{App, Dialog, HELP, InputKind, Panel},
    files::{self, Operation},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthChar;

const ASCII_BORDER: symbols::border::Set = symbols::border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

pub fn draw(app: &mut App, frame: &mut Frame) {
    let area = frame.area();
    let base = if app.mono {
        Style::default()
    } else {
        Style::default().fg(Color::White).bg(Color::Blue)
    };
    let accent = if app.mono {
        base.bold()
    } else {
        base.fg(Color::Yellow).bold()
    };
    let selected = if app.mono {
        base.reversed()
    } else {
        base.fg(Color::Black).bg(Color::Cyan).bold()
    };
    let border = if app.ascii {
        ASCII_BORDER
    } else {
        symbols::border::DOUBLE
    };
    frame.render_widget(Block::default().style(base), area);
    app.small = area.width < 40 || area.height < 12;
    if app.small {
        frame.render_widget(
            Paragraph::new(
                "mDir 2026\nResize to at least 40 x 12\nq / Ctrl-C exits; Esc cancels dialog",
            ),
            area,
        );
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(4),
        Constraint::Length(2),
        Constraint::Length(2),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" mDir ", selected),
            Span::styled(" / 2026  ", accent),
            Span::raw(" FILES, AT YOUR FINGERTIPS"),
            Span::styled(if app.dual { "   DUAL" } else { "   CLASSIC" }, accent),
        ])),
        rows[0],
    );
    let current = &app.panels[app.active];
    let work = &app.panels[1 - app.active];
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(" Path  ", accent),
                Span::raw(display_path(&current.path)),
            ]),
            Line::from(vec![
                Span::styled(" Work  ", accent),
                Span::raw(display_path(&work.path)),
            ]),
        ]),
        rows[1],
    );
    app.page_rows = rows[2].height.saturating_sub(2).max(1) as usize;
    let count = app
        .columns
        .min((rows[2].width.saturating_sub(2) as usize / 18).max(1));
    // Navigation uses the same effective column count as the current viewport.
    if !app.dual {
        app.columns = count;
    }
    if app.dual {
        let panels = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(rows[2]);
        for (index, panel) in app.panels.iter_mut().enumerate() {
            draw_panel(
                frame,
                panel,
                panels[index],
                1,
                base,
                if index == app.active {
                    selected
                } else {
                    base.reversed()
                },
                border,
                if index == app.active {
                    " CURRENT "
                } else {
                    " WORK "
                },
            );
        }
    } else {
        draw_panel(
            frame,
            &mut app.panels[app.active],
            rows[2],
            count,
            base,
            selected,
            border,
            " FILES ",
        );
    }
    let panel = &app.panels[app.active];
    let filename = panel
        .current()
        .map(|e| files::safe_text(&e.label).replace(['\n', '\t'], " "))
        .unwrap_or_default();
    let detail = format!(
        " {} / {}  |  {} marked  |  {}{}  |  {}",
        if panel.visible.is_empty() {
            0
        } else {
            panel.selected + 1
        },
        panel.visible.len(),
        panel.marked.len(),
        if panel.extension_sort { "EXT" } else { "NAME" },
        if panel.reverse { " DESC" } else { " ASC" },
        filename
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(detail, accent)),
            Line::from(files::safe_text(&app.status).replace(['\n', '\t'], " ")),
        ]),
        rows[3],
    );
    let footer = if area.width >= 90 {
        " F1 Help   F2 Rename   F3 View   F5 Copy   F6 Move   F7 Mkdir   F8 Delete   F10 Folders"
    } else {
        " ? help  e rename  p view  c copy  m move  n mkdir  d delete"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(footer, accent)),
            Line::from(
                " Tab Work   / Filter   Space Mark   w Layout   g Path   r Refresh   q Quit",
            ),
        ]),
        rows[4],
    );

    if let Some(dialog) = &app.dialog {
        let (title, text, scroll) = match dialog {
            Dialog::Filter => (
                " FILTER | Enter keeps · Esc clears ",
                panel.filter.clone(),
                0,
            ),
            Dialog::Input { kind, text } => (
                match kind {
                    InputKind::Mkdir => " NEW FOLDER | Enter reviews · Esc cancels ",
                    InputKind::Rename(_) => " RENAME | New name · Enter reviews · Esc cancels ",
                    InputKind::Goto => " GO TO PATH | Enter opens · Esc cancels ",
                },
                text.clone(),
                0,
            ),
            Dialog::Help(scroll) => (
                " HELP | arrows/PgDn scroll · q/Esc closes ",
                HELP.into(),
                *scroll,
            ),
            Dialog::Preview { text, scroll, .. } => (
                " PREVIEW · 64 KiB | arrows/PgDn · q/Esc closes ",
                text.clone(),
                *scroll,
            ),
            Dialog::Confirm { operations, scroll } => {
                let mut text = format!(
                    "{} operation(s). Existing destinations are refused.\nDelete is permanent; directories must be empty.\nReview all paths below. Up/Down/PgDn scroll.\n\n",
                    operations.len()
                );
                for (i, operation) in operations.iter().enumerate() {
                    text.push_str(&format!("{}. {}\n\n", i + 1, describe(operation)));
                }
                (" CONFIRM | y executes · n/Esc cancels ", text, *scroll)
            }
        };
        let popup = Rect::new(area.x + 2, area.y + 2, area.width - 4, area.height - 4);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border)
            .title(title)
            .style(base);
        let inner = block.inner(popup);
        let lines = wrap_cells(&text, inner.width.max(1) as usize);
        app.dialog_max = lines
            .len()
            .saturating_sub(inner.height as usize)
            .min(u16::MAX as usize) as u16;
        frame.render_widget(Clear, popup);
        frame.render_widget(block, popup);
        let visible: Vec<Line> = lines
            .into_iter()
            .skip(scroll.min(app.dialog_max) as usize)
            .take(inner.height as usize)
            .map(Line::from)
            .collect();
        frame.render_widget(Paragraph::new(visible).style(base), inner);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_panel(
    frame: &mut Frame,
    panel: &mut Panel,
    area: Rect,
    columns: usize,
    base: Style,
    selected: Style,
    border: symbols::border::Set,
    label: &str,
) {
    let title = format!(
        "{label} {}{}{} ",
        panel.visible.len(),
        if panel.loading { " · reading..." } else { "" },
        if panel.folders_only {
            " · folders only"
        } else {
            ""
        }
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border)
        .title(title)
        .style(base);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = inner.height as usize;
    if rows == 0 {
        return;
    }
    let capacity = rows * columns;
    panel.offset = panel.selected / capacity * capacity;
    if panel.visible.is_empty() {
        frame.render_widget(
            Paragraph::new(if panel.loading {
                " Reading directory..."
            } else {
                " (empty or no matches)"
            }),
            inner,
        );
        return;
    }
    let cells =
        Layout::horizontal(vec![Constraint::Ratio(1, columns as u32); columns]).split(inner);
    for (column, rect) in cells.iter().enumerate() {
        for row in 0..rows {
            let position = panel.offset + column * rows + row;
            let Some(&index) = panel.visible.get(position) else {
                break;
            };
            let entry = &panel.entries[index];
            let marked = panel.marked.contains(&entry.name);
            let kind = if entry.directory {
                "/"
            } else if entry.symlink {
                "@"
            } else {
                " "
            };
            let text = format!(
                "{}{} {}",
                if marked { "*" } else { " " },
                kind,
                entry.label.replace(['\n', '\t'], " ")
            );
            let style = if position == panel.selected {
                selected
            } else if marked {
                base.fg(Color::Yellow).bold()
            } else if entry.directory {
                base.bold()
            } else {
                base
            };
            let width = rect
                .width
                .saturating_sub(if column + 1 < columns { 1 } else { 0 });
            frame.render_widget(
                Paragraph::new(text).style(style),
                Rect::new(rect.x, rect.y + row as u16, width, 1),
            );
        }
        if column + 1 < columns && rect.width > 0 {
            for row in 0..rows {
                frame.render_widget(
                    Paragraph::new(if border == ASCII_BORDER { "|" } else { "│" }),
                    Rect::new(rect.right() - 1, rect.y + row as u16, 1, 1),
                );
            }
        }
    }
}

pub fn display_path(path: &std::path::Path) -> String {
    files::safe_text(&path.to_string_lossy())
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}
fn describe(operation: &Operation) -> String {
    match operation {
        Operation::Copy {
            source,
            destination,
        } => format!(
            "COPY FILE\n{}\nTO {}",
            display_path(source),
            display_path(destination)
        ),
        Operation::Move {
            source,
            destination,
        } => format!(
            "MOVE / RENAME\n{}\nTO {}",
            display_path(source),
            display_path(destination)
        ),
        Operation::Delete(path) => format!("PERMANENT DELETE\n{}", display_path(path)),
        Operation::Mkdir(path) => format!("CREATE DIRECTORY\n{}", display_path(path)),
    }
}
fn wrap_cells(text: &str, width: usize) -> Vec<String> {
    let safe = files::safe_text(text).replace('\t', "    ");
    let mut lines = Vec::new();
    for line in safe.split('\n') {
        let mut current = String::new();
        let mut cells = 0;
        for character in line.chars() {
            let length = character.width().unwrap_or(0);
            if cells + length > width && !current.is_empty() {
                lines.push(current);
                current = String::new();
                cells = 0;
            }
            current.push(character);
            cells += length;
        }
        lines.push(current);
    }
    lines
}
