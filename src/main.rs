mod app;
mod benchmark;
mod files;
mod ui;

use app::{App, Dialog, Message};
use crossterm::{
    cursor,
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Terminal,
    backend::{CrosstermBackend, TestBackend},
};
use std::{
    ffi::OsString,
    io::{self, IsTerminal},
    path::PathBuf,
    sync::mpsc,
    thread,
};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mdir: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "--bench") {
        if args.len() > 2 {
            return Err(io::Error::other("Usage: mdir --bench [DIRECTORY]"));
        }
        return benchmark::run(args.get(1).map(PathBuf::from).as_deref());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", app::HELP);
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("mdir {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let mut paths = Vec::new();
    let (mut ascii, mut mono, mut dual, mut snapshot, mut literal) = (
        false,
        std::env::var_os("NO_COLOR").is_some(),
        false,
        false,
        false,
    );
    for arg in args {
        if literal {
            paths.push(PathBuf::from(arg));
            continue;
        }
        match arg.to_str() {
            Some("--") => literal = true,
            Some("--ascii") => ascii = true,
            Some("--mono") => mono = true,
            Some("--dual") => dual = true,
            Some("--snapshot") => snapshot = true,
            Some(value) if value.starts_with('-') => {
                return Err(io::Error::other(format!(
                    "Unknown option {value}; use --help"
                )));
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if paths.len() > 2 {
        return Err(io::Error::other(
            "Provide at most CURRENT and WORK directories",
        ));
    }
    let current = paths.first().cloned().unwrap_or(std::env::current_dir()?);
    let work = paths.get(1).cloned().unwrap_or(current.clone());
    let mut app = App::new(current, work, ascii, mono, dual)?;
    if snapshot {
        for panel in &mut app.panels {
            panel.entries = files::scan(&panel.path)?;
            panel.rebuild();
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| ui::draw(&mut app, frame)).unwrap();
        for row in terminal.backend().buffer().content.chunks(120) {
            println!(
                "{}",
                row.iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .trim_end()
            );
        }
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "Interactive mode requires a terminal; use --help or --snapshot when redirecting",
        ));
    }
    if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
        return Err(io::Error::other("TERM=dumb cannot display this UI"));
    }
    let (sender, receiver) = mpsc::channel();
    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let input_sender = sender.clone();
    thread::spawn(move || {
        loop {
            let input = event::read();
            let failed = input.is_err();
            if input_sender.send(Message::Input(input)).is_err() || failed {
                break;
            }
        }
    });
    app.reload(0, &sender);
    app.reload(1, &sender);
    loop {
        terminal.draw(|frame| ui::draw(&mut app, frame))?;
        match receiver.recv().map_err(io::Error::other)? {
            Message::Input(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                if app.key(key, &sender) {
                    break;
                }
            }
            Message::Input(Err(error)) => return Err(error),
            Message::Loaded(index, result) => {
                let panel = &mut app.panels[index];
                panel.loading = false;
                match result {
                    Ok(entries) => {
                        panel.entries = entries;
                        panel.rebuild();
                    }
                    Err(error) => {
                        panel.entries.clear();
                        panel.rebuild();
                        app.status = format!("Read failed: {error}");
                    }
                }
                if panel.reload_pending {
                    panel.reload_pending = false;
                    app.reload(index, &sender);
                }
            }
            Message::Operated(report) => {
                app.operating = false;
                app.status = match report.result {
                    Ok(()) => format!("Done: {} of {} operations", report.completed, report.total),
                    Err(error) => format!(
                        "Stopped: {} of {} completed. {error}",
                        report.completed, report.total
                    ),
                };
                if app.quit_requested {
                    break;
                }
                for panel in &mut app.panels {
                    panel.marked.clear();
                }
                app.reload(0, &sender);
                app.reload(1, &sender);
            }
            Message::Preview(id, result) => {
                app.preview_busy = false;
                if matches!(app.dialog, Some(Dialog::Preview { id: current, .. }) if id == current)
                {
                    app.dialog = Some(Dialog::Preview {
                        id,
                        text: result.unwrap_or_else(|error| format!("Preview failed: {error}")),
                        scroll: 0,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(())
}

struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(io::stdout(), EnterAlternateScreen, cursor::Hide)?;
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));
        Ok(guard)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}
fn restore_terminal() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        LeaveAlternateScreen,
        cursor::Show,
        crossterm::style::ResetColor
    );
}
