use crate::files::{self, Entry, Operation};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    thread,
};

pub const HELP: &str = "mDir 2026 · a modern Mdir-inspired file manager\n\nUsage: mdir [--dual] [--ascii] [--mono] [CURRENT] [WORK]\n       mdir --bench [DIRECTORY] | --snapshot | --version\n\nNAVIGATE\n  Up/Down or j/k: cursor     Left/Right: column (classic view)\n  Enter/l: open folder       Backspace/h/.: parent\n  Home/End, PgUp/PgDn        \\: volume root\n  Tab / Alt-W: switch current and work directory\n  g: enter a path (also Windows drives and UNC paths)\n  w: classic columns / dual panels    1..6: column count\n  /: live filename filter; Enter keeps, Esc clears\n  o: name/extension sort    s: reverse    r: refresh\n  Alt-N/E: name/extension sort\n  Space: mark/unmark and advance    u: clear marks\n  F10/t: folders only (MCD-style navigation), again: all\n\nFILES\n  p/v / Alt-V / F3: preview (64 KiB maximum)\n  c / Alt-C / Ins / F5: copy marked files to work directory\n  m / Alt-M / F6: move to work directory (same filesystem)\n  e / Alt-R / F2: rename selected item\n  n / F7: create directory\n  d / Alt-D / Del / F8: PERMANENT delete\n  All changes require a separate y confirmation. Esc cancels.\n  Confirmation/preview: Up/Down, PgUp/PgDn, Home/End scroll.\n  Copy regular files; move files/folders; delete files/empty folders.\n  Links, recursive copy/delete, cross-volume moves are refused.\n\nQUIT\n  q / Ctrl-C: exit; during copy Ctrl-C cancels, cleans up, exits\n  F1/?: help   NO_COLOR or --mono: monochrome\n\nStandalone macOS/Linux/Windows binary. No Nerd Font required.\nOriginal Mdir F2-F9 launchers are modernized; F10 navigates folders.\nNot an official Mdir release. No shell commands run on Enter.";

pub struct App {
    pub panels: [Panel; 2],
    pub active: usize,
    pub status: String,
    pub dialog: Option<Dialog>,
    pub operating: bool,
    pub ascii: bool,
    pub mono: bool,
    pub dual: bool,
    pub columns: usize,
    pub cancelled: Arc<AtomicBool>,
    pub quit_requested: bool,
    pub preview_busy: bool,
    pub operation_worker: Option<thread::JoinHandle<()>>,
    pub preview_id: u64,
    pub dialog_max: u16,
    pub page_rows: usize,
    pub small: bool,
}
impl App {
    pub fn new(
        current: PathBuf,
        work: PathBuf,
        ascii: bool,
        mono: bool,
        dual: bool,
    ) -> io::Result<Self> {
        Ok(Self {
            panels: [Panel::new(current)?, Panel::new(work)?],
            active: 0,
            status: "Ready · Space marks files · ? help".into(),
            dialog: None,
            operating: false,
            ascii,
            mono,
            dual,
            columns: 3,
            cancelled: Arc::new(AtomicBool::new(false)),
            quit_requested: false,
            preview_busy: false,
            operation_worker: None,
            preview_id: 0,
            dialog_max: 0,
            page_rows: 20,
            small: false,
        })
    }
    pub fn reload(&mut self, index: usize, sender: &Sender<Message>) {
        let panel = &mut self.panels[index];
        if panel.loading {
            panel.reload_pending = true;
            return;
        }
        panel.loading = true;
        let path = panel.path.clone();
        let sender = sender.clone();
        thread::spawn(move || {
            let _ = sender.send(Message::Loaded(index, files::scan(&path)));
        });
    }
    pub fn key(&mut self, mut key: KeyEvent, sender: &Sender<Message>) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.operating {
                self.cancelled.store(true, Ordering::Relaxed);
                self.quit_requested = true;
                return false;
            }
            return true;
        }
        if self.operating {
            if key.code == KeyCode::Esc {
                self.cancelled.store(true, Ordering::Relaxed);
            }
            self.status = "Working · Esc cancels copy · Ctrl-C cancels then exits".into();
            return false;
        }
        if self.small {
            if key.code == KeyCode::Esc {
                self.dialog = None;
            }
            return key.code == KeyCode::Char('q');
        }
        if let Some(dialog) = self.dialog.take() {
            self.dialog_key(dialog, key, sender);
            return false;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Char('n') | KeyCode::Char('e') => {
                    let panel = &mut self.panels[self.active];
                    panel.extension_sort = key.code == KeyCode::Char('e');
                    panel.rebuild();
                    return false;
                }
                KeyCode::Char('w') => key.code = KeyCode::Tab,
                KeyCode::Char('r') => key.code = KeyCode::Char('e'),
                KeyCode::Char('v') => key.code = KeyCode::Char('p'),
                KeyCode::Char(c @ '1'..='6') => key.code = KeyCode::Char(c),
                KeyCode::Char('c' | 'm' | 'd') => {}
                _ => return false,
            }
        } else if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            return false;
        }
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Tab => {
                self.active = 1 - self.active;
                return false;
            }
            KeyCode::Char('w') => {
                self.dual = !self.dual;
                return false;
            }
            KeyCode::Char(c @ '1'..='6') => {
                self.columns = c.to_digit(10).unwrap() as usize;
                self.dual = false;
                return false;
            }
            KeyCode::F(1) | KeyCode::Char('?') => {
                self.dialog = Some(Dialog::Help(0));
                return false;
            }
            KeyCode::Char('g') => {
                self.dialog = Some(Dialog::Input {
                    kind: InputKind::Goto,
                    text: String::new(),
                });
                return false;
            }
            _ => {}
        }
        let other_path = self.panels[1 - self.active].path.clone();
        let panel = &mut self.panels[self.active];
        if panel.loading {
            self.status = "Reading directory · Tab switches · q exits".into();
            return false;
        }
        let max = panel.visible.len().saturating_sub(1);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => panel.selected = (panel.selected + 1).min(max),
            KeyCode::Up | KeyCode::Char('k') => panel.selected = panel.selected.saturating_sub(1),
            KeyCode::Right if !self.dual => {
                panel.selected = (panel.selected + self.page_rows).min(max)
            }
            KeyCode::Left if !self.dual => {
                panel.selected = panel.selected.saturating_sub(self.page_rows)
            }
            KeyCode::PageDown => {
                panel.selected = (panel.selected
                    + self.page_rows * if self.dual { 1 } else { self.columns })
                .min(max)
            }
            KeyCode::PageUp => {
                panel.selected = panel
                    .selected
                    .saturating_sub(self.page_rows * if self.dual { 1 } else { self.columns })
            }
            KeyCode::Home => panel.selected = 0,
            KeyCode::End => panel.selected = max,
            KeyCode::Char(' ') => {
                if let Some(entry) = panel.current() {
                    let name = entry.name.clone();
                    if !panel.marked.remove(&name) {
                        panel.marked.insert(name);
                    }
                    panel.selected = (panel.selected + 1).min(max);
                }
            }
            KeyCode::Char('u') => panel.marked.clear(),
            KeyCode::Char('/') => self.dialog = Some(Dialog::Filter),
            KeyCode::Char('s') => {
                panel.reverse = !panel.reverse;
                panel.rebuild();
            }
            KeyCode::Char('o') => {
                panel.extension_sort = !panel.extension_sort;
                panel.rebuild();
            }
            KeyCode::F(10) | KeyCode::Char('t') => {
                panel.folders_only = !panel.folders_only;
                panel.rebuild();
            }
            KeyCode::Char('r') => self.reload(self.active, sender),
            KeyCode::Backspace | KeyCode::Char('h') | KeyCode::Char('.') | KeyCode::Left => {
                if let Some(parent) = panel.path.parent() {
                    let path = parent.to_path_buf();
                    self.navigate(path, sender);
                }
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                if let Some(entry) = panel.current() {
                    if entry.directory {
                        let path = panel.path.join(&entry.name);
                        self.navigate(path, sender);
                    } else {
                        self.status = "p / F3 previews a file; Enter opens folders".into();
                    }
                }
            }
            KeyCode::Char('\\') => {
                let root = panel
                    .path
                    .ancestors()
                    .last()
                    .unwrap_or(&panel.path)
                    .to_path_buf();
                self.navigate(root, sender);
            }
            KeyCode::Char('n') | KeyCode::F(7) => {
                self.dialog = Some(Dialog::Input {
                    kind: InputKind::Mkdir,
                    text: String::new(),
                })
            }
            KeyCode::Char('e') | KeyCode::F(2) => {
                if let Some(entry) = panel.current() {
                    self.dialog = Some(Dialog::Input {
                        kind: InputKind::Rename(panel.path.join(&entry.name)),
                        text: String::new(),
                    });
                }
            }
            KeyCode::Char('p' | 'v') | KeyCode::F(3) => {
                if self.preview_busy {
                    self.status = "Previous preview is still reading".into();
                    return false;
                }
                if let Some(entry) = panel.current() {
                    self.preview_busy = true;
                    self.preview_id += 1;
                    let id = self.preview_id;
                    let path = panel.path.join(&entry.name);
                    let sender = sender.clone();
                    self.dialog = Some(Dialog::Preview {
                        id,
                        text: "Reading preview...".into(),
                        scroll: 0,
                    });
                    thread::spawn(move || {
                        let _ = sender.send(Message::Preview(id, files::preview(&path)));
                    });
                }
            }
            KeyCode::Char('c' | 'm' | 'd')
            | KeyCode::Insert
            | KeyCode::Delete
            | KeyCode::F(5 | 6 | 8) => {
                let names: Vec<OsString> = if panel.marked.is_empty() {
                    panel
                        .current()
                        .map(|e| vec![e.name.clone()])
                        .unwrap_or_default()
                } else {
                    panel.marked.iter().cloned().collect()
                };
                let operations = names
                    .into_iter()
                    .map(|name| {
                        let source = panel.path.join(&name);
                        let destination = other_path.join(&name);
                        match key.code {
                            KeyCode::Char('d') | KeyCode::Delete | KeyCode::F(8) => {
                                Operation::Delete(source)
                            }
                            KeyCode::Char('m') | KeyCode::F(6) => Operation::Move {
                                source,
                                destination,
                            },
                            _ => Operation::Copy {
                                source,
                                destination,
                            },
                        }
                    })
                    .collect::<Vec<_>>();
                if !operations.is_empty() {
                    self.dialog = Some(Dialog::Confirm {
                        operations,
                        scroll: 0,
                    });
                }
            }
            _ => {}
        }
        false
    }
    fn dialog_key(&mut self, dialog: Dialog, key: KeyEvent, sender: &Sender<Message>) {
        match dialog {
            Dialog::Confirm {
                operations,
                mut scroll,
            } => match key.code {
                KeyCode::Char('y') => {
                    self.operating = true;
                    self.cancelled.store(false, Ordering::Relaxed);
                    self.status = format!("Working on {} item(s)...", operations.len());
                    let cancelled = self.cancelled.clone();
                    let sender = sender.clone();
                    self.operation_worker = Some(thread::spawn(move || {
                        let total = operations.len();
                        let mut completed = 0;
                        let mut result = Ok(());
                        for operation in operations {
                            match files::perform_cancellable(&operation, &cancelled) {
                                Ok(_) => completed += 1,
                                Err(error) => {
                                    result = Err(format!("{error}"));
                                    break;
                                }
                            }
                        }
                        let _ = sender.send(Message::Operated(BatchReport {
                            completed,
                            total,
                            result,
                        }));
                    }));
                }
                KeyCode::Esc | KeyCode::Char('n') => self.status = "Cancelled".into(),
                code => {
                    scroll_key(&mut scroll, code, self.dialog_max, self.page_rows);
                    self.dialog = Some(Dialog::Confirm { operations, scroll });
                }
            },
            Dialog::Preview {
                id,
                text,
                mut scroll,
            } => {
                if !matches!(key.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter) {
                    scroll_key(&mut scroll, key.code, self.dialog_max, self.page_rows);
                    self.dialog = Some(Dialog::Preview { id, text, scroll });
                }
            }
            Dialog::Help(mut scroll) => {
                if !matches!(key.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter) {
                    scroll_key(&mut scroll, key.code, self.dialog_max, self.page_rows);
                    self.dialog = Some(Dialog::Help(scroll));
                }
            }
            Dialog::Filter => {
                let panel = &mut self.panels[self.active];
                match key.code {
                    KeyCode::Esc => {
                        panel.filter.clear();
                        panel.rebuild();
                    }
                    KeyCode::Enter => {}
                    code => {
                        match code {
                            KeyCode::Backspace => {
                                panel.filter.pop();
                            }
                            KeyCode::Char(c) if !c.is_control() => panel.filter.push(c),
                            _ => {}
                        }
                        panel.rebuild();
                        self.dialog = Some(Dialog::Filter);
                    }
                }
            }
            Dialog::Input { kind, mut text } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if matches!(kind, InputKind::Goto) {
                        let expanded = if text == "~" {
                            std::env::var_os("HOME")
                                .or_else(|| std::env::var_os("USERPROFILE"))
                                .map(PathBuf::from)
                        } else {
                            None
                        };
                        let path =
                            expanded.unwrap_or_else(|| self.panels[self.active].path.join(&text));
                        self.navigate(path, sender);
                    } else if !single_name(&text) {
                        self.status = "Enter one non-empty name, without path separators".into();
                        self.dialog = Some(Dialog::Input { kind, text });
                    } else {
                        let destination = self.panels[self.active].path.join(text);
                        let operation = match kind {
                            InputKind::Rename(source) => Operation::Move {
                                source,
                                destination,
                            },
                            _ => Operation::Mkdir(destination),
                        };
                        self.dialog = Some(Dialog::Confirm {
                            operations: vec![operation],
                            scroll: 0,
                        });
                    }
                }
                code => {
                    match code {
                        KeyCode::Backspace => {
                            text.pop();
                        }
                        KeyCode::Char(c) if !c.is_control() => text.push(c),
                        _ => {}
                    }
                    self.dialog = Some(Dialog::Input { kind, text });
                }
            },
        }
    }
    fn navigate(&mut self, path: PathBuf, sender: &Sender<Message>) {
        if self.panels[self.active].loading {
            self.status = "Wait for the current directory scan".into();
            return;
        }
        // Only normalize lexically here; read_dir errors arrive through the worker.
        let path = if path.is_absolute() {
            path
        } else {
            self.panels[self.active].path.join(path)
        };
        let panel = &mut self.panels[self.active];
        let mut normalized = PathBuf::new();
        for part in path.components() {
            match part {
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                std::path::Component::CurDir => {}
                part => normalized.push(part.as_os_str()),
            }
        }
        panel.path = normalized;
        panel.entries.clear();
        panel.visible.clear();
        panel.marked.clear();
        panel.filter.clear();
        panel.selected = 0;
        panel.offset = 0;
        self.reload(self.active, sender);
    }
}

pub struct Panel {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub visible: Vec<usize>,
    pub selected: usize,
    pub filter: String,
    pub reverse: bool,
    pub extension_sort: bool,
    pub folders_only: bool,
    pub marked: BTreeSet<OsString>,
    pub loading: bool,
    pub reload_pending: bool,
    pub offset: usize,
}
impl Panel {
    fn new(path: PathBuf) -> io::Result<Self> {
        let path = fs::canonicalize(path)?;
        if !path.is_dir() {
            return Err(io::Error::other("Starting path must be a directory"));
        }
        Ok(Self {
            path,
            entries: Vec::new(),
            visible: Vec::new(),
            selected: 0,
            filter: String::new(),
            reverse: false,
            extension_sort: false,
            folders_only: false,
            marked: BTreeSet::new(),
            loading: false,
            reload_pending: false,
            offset: 0,
        })
    }
    pub fn rebuild(&mut self) {
        let selected_name = self.current().map(|entry| entry.name.clone());
        let query = self.filter.to_lowercase();
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.key.contains(&query) && (!self.folders_only || e.directory))
            .map(|(i, _)| i)
            .collect();
        self.visible.sort_unstable_by(|&a, &b| {
            let a = &self.entries[a];
            let b = &self.entries[b];
            let order = if self.extension_sort {
                a.extension.cmp(&b.extension).then(a.key.cmp(&b.key))
            } else {
                a.key.cmp(&b.key)
            }
            .then(a.name.cmp(&b.name));
            b.directory
                .cmp(&a.directory)
                .then(if self.reverse { order.reverse() } else { order })
        });
        self.selected = selected_name
            .and_then(|name| {
                self.visible
                    .iter()
                    .position(|&i| self.entries[i].name == name)
            })
            .unwrap_or(self.selected.min(self.visible.len().saturating_sub(1)));
        self.marked
            .retain(|name| self.entries.iter().any(|entry| &entry.name == name));
    }
    pub fn current(&self) -> Option<&Entry> {
        self.visible
            .get(self.selected)
            .and_then(|&i| self.entries.get(i))
    }
}

pub enum Dialog {
    Filter,
    Input {
        kind: InputKind,
        text: String,
    },
    Confirm {
        operations: Vec<Operation>,
        scroll: u16,
    },
    Preview {
        id: u64,
        text: String,
        scroll: u16,
    },
    Help(u16),
}
pub enum InputKind {
    Mkdir,
    Rename(PathBuf),
    Goto,
}
pub enum Message {
    Input(io::Result<Event>),
    Loaded(usize, io::Result<Vec<Entry>>),
    Operated(BatchReport),
    Preview(u64, io::Result<String>),
}
pub struct BatchReport {
    pub completed: usize,
    pub total: usize,
    pub result: Result<(), String>,
}
fn single_name(name: &str) -> bool {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return false;
    }
    let mut parts = Path::new(name).components();
    matches!(parts.next(), Some(std::path::Component::Normal(_))) && parts.next().is_none()
}
fn scroll_key(scroll: &mut u16, key: KeyCode, max: u16, rows: usize) {
    let page = rows.min(u16::MAX as usize) as u16;
    *scroll = match key {
        KeyCode::Down | KeyCode::Char('j') => scroll.saturating_add(1).min(max),
        KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
        KeyCode::PageDown | KeyCode::Char(' ') => scroll.saturating_add(page).min(max),
        KeyCode::PageUp => scroll.saturating_sub(page),
        KeyCode::Home => 0,
        KeyCode::End => max,
        _ => *scroll,
    };
}

impl Drop for App {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(worker) = self.operation_worker.take() {
            let _ = worker.join();
        }
    }
}
