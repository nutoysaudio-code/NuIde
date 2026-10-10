//! TUI de NuCoder: chat con streaming, estado del router y rotación.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use serde_json::{json, Value};

use crate::agent::{self, Agent, AgentEvent};
use crate::config::{Config, TASKS};

/// Una línea del transcript.
struct ChatLine {
    text: String,
    style: Style,
}

/// Estado de la aplicación.
pub struct App {
    pub config: Arc<Config>,
    pub cwd: PathBuf,
    pub task: String,
    pub free_only: bool,
    lines: Vec<ChatLine>,
    input: String,
    cursor: usize,
    scroll: u16,
    stick: bool,
    running: bool,
    stream_line: Option<usize>,
    history: Vec<String>,
    history_idx: Option<usize>,
    conversation: Vec<Value>,
    cancel: Arc<AtomicBool>,
    tx: Sender<AgentEvent>,
    rx: Receiver<AgentEvent>,
    model: String,
    quit: bool,
}

impl App {
    pub fn new(config: Arc<Config>, cwd: PathBuf, task: String, free_only: bool) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let model = config
            .default_model
            .clone()
            .unwrap_or_else(|| "auto (router)".to_string());
        let mut app = Self {
            config,
            cwd,
            task,
            free_only,
            lines: Vec::new(),
            input: String::new(),
            cursor: 0,
            scroll: 0,
            stick: true,
            running: false,
            stream_line: None,
            history: Vec::new(),
            history_idx: None,
            conversation: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            model,
            quit: false,
        };
        app.info("NuCoder — coder con router de modelos (gratis primero).");
        app.info("Enter para enviar · /refactor /debug /explain /test cambian la tarea · /free alterna gratis · /help · Ctrl+C sale.");
        app
    }

    fn push(&mut self, text: String, style: Style) {
        self.lines.push(ChatLine { text, style });
        if self.lines.len() > 1000 {
            self.lines.remove(0);
        }
        self.stick = true;
    }

    fn info(&mut self, t: impl Into<String>) {
        self.push(
            format!("· {}", t.into()),
            Style::default().fg(Color::DarkGray),
        );
    }

    fn on_agent_event(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::Routed { label, free } => {
                self.model = label.clone();
                self.stream_line = None;
                let tag = if free { "FREE" } else { "pago" };
                self.push(
                    format!("· modelo [{tag}]: {label}  (tarea: {})", self.task),
                    Style::default().fg(Color::Magenta),
                );
            }
            AgentEvent::Delta(t) => {
                self.stick = true;
                match self.stream_line {
                    Some(i) if i < self.lines.len() => self.lines[i].text.push_str(&t),
                    _ => {
                        self.lines.push(ChatLine {
                            text: t,
                            style: Style::default().fg(Color::Green),
                        });
                        self.stream_line = Some(self.lines.len() - 1);
                    }
                }
            }
            AgentEvent::ToolStart { name, args } => {
                self.stream_line = None;
                let a = preview(&args, 120);
                self.push(format!("🔧 {name} {a}"), Style::default().fg(Color::Cyan));
            }
            AgentEvent::ToolResult { name, text } => {
                self.stream_line = None;
                self.push(
                    format!("   ↳ {name}: {}", preview(&text, 200)),
                    Style::default().fg(Color::DarkGray),
                );
            }
            AgentEvent::Notice(m) => {
                self.stream_line = None;
                self.push(format!("· {m}"), Style::default().fg(Color::Yellow));
            }
            AgentEvent::Error(m) => {
                self.stream_line = None;
                self.push(format!("✗ {m}"), Style::default().fg(Color::Red));
            }
            AgentEvent::Done { conversation } => {
                self.stream_line = None;
                self.running = false;
                self.conversation = conversation;
            }
        }
    }

    fn submit(&mut self) {
        let raw = self.input.trim().to_string();
        if raw.is_empty() {
            return;
        }
        self.input.clear();
        self.cursor = 0;
        self.history.push(raw.clone());
        self.history_idx = None;

        // Comandos de barra.
        if raw.starts_with('/') {
            let (cmd, rest) = split_cmd(&raw);
            if TASKS.contains(&cmd.as_str()) {
                self.task = cmd.clone();
                self.info(format!("tarea → {cmd}"));
                if rest.is_empty() {
                    return;
                }
                self.send(rest);
                return;
            }
            match cmd.as_str() {
                "free" => {
                    self.free_only = !self.free_only;
                    self.info(format!(
                        "modo gratis: {}",
                        if self.free_only { "ON" } else { "OFF" }
                    ));
                    return;
                }
                "clear" => {
                    self.lines.clear();
                    return;
                }
                "quit" | "exit" => {
                    self.quit = true;
                    return;
                }
                "help" => {
                    self.info("/refactor /debug /explain /test /architect /complete · /free · /clear · /quit");
                    return;
                }
                _ => {}
            }
        }
        self.send(raw);
    }

    fn send(&mut self, prompt: String) {
        if self.running {
            self.info("esperá a que termine el turno actual");
            return;
        }
        self.push(
            format!("> {prompt}"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
        if self.conversation.is_empty() {
            self.conversation = agent::new_conversation(&agent::system_prompt(&self.cwd), &prompt);
        } else {
            self.conversation
                .push(json!({ "role": "user", "content": prompt }));
        }
        self.running = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        let agent = Agent {
            config: self.config.clone(),
            cwd: self.cwd.clone(),
            task: self.task.clone(),
            free_only: self.free_only,
            cancel: self.cancel.clone(),
        };
        let tx = self.tx.clone();
        let conv = std::mem::take(&mut self.conversation);
        std::thread::spawn(move || agent.run(conv, tx));
    }

    fn on_key(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => self.quit = true,
            (_, KeyCode::Esc) if self.running => {
                self.cancel.store(true, Ordering::Relaxed);
                self.info("cancelando…");
            }
            _ if self.running => {
                // Mientras corre, permitimos scroll y Ctrl+C.
                match key.code {
                    KeyCode::Up => self.scroll_up(1),
                    KeyCode::Down => self.scroll_down(1),
                    KeyCode::PageUp => self.scroll_up(10),
                    KeyCode::PageDown => self.scroll_down(10),
                    _ => {}
                }
            }
            (_, KeyCode::Enter) => self.submit(),
            (_, KeyCode::Char(c)) => {
                self.input.insert(self.cursor, c);
                self.cursor += 1;
            }
            (_, KeyCode::Backspace) => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.input.remove(self.cursor);
                }
            }
            (_, KeyCode::Delete) => {
                if self.cursor < self.input.chars().count() {
                    self.input.remove(self.cursor);
                }
            }
            (_, KeyCode::Left) => self.cursor = self.cursor.saturating_sub(1),
            (_, KeyCode::Right) => self.cursor = (self.cursor + 1).min(self.input.chars().count()),
            (_, KeyCode::Home) => self.cursor = 0,
            (_, KeyCode::End) => self.cursor = self.input.chars().count(),
            (_, KeyCode::Up) => self.history_prev(),
            (_, KeyCode::Down) => self.history_next(),
            (_, KeyCode::PageUp) => self.scroll_up(10),
            (_, KeyCode::PageDown) => self.scroll_down(10),
            _ => {}
        }
    }

    fn scroll_up(&mut self, n: u16) {
        self.stick = false;
        self.scroll = self.scroll.saturating_sub(n);
    }

    fn scroll_down(&mut self, n: u16) {
        self.scroll = self.scroll.saturating_add(n);
        // Si llegamos al fondo, volvemos a "pegar".
        self.stick = false;
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_idx {
            Some(0) => 0,
            Some(i) => i - 1,
            None => self.history.len() - 1,
        };
        self.history_idx = Some(idx);
        self.input = self.history[idx].clone();
        self.cursor = self.input.chars().count();
    }

    fn history_next(&mut self) {
        let Some(i) = self.history_idx else {
            return;
        };
        if i + 1 >= self.history.len() {
            self.history_idx = None;
            self.input.clear();
            self.cursor = 0;
        } else {
            self.history_idx = Some(i + 1);
            self.input = self.history[i + 1].clone();
            self.cursor = self.input.chars().count();
        }
    }
}

fn split_cmd(raw: &str) -> (String, String) {
    let mut it = raw.trim_start_matches('/').splitn(2, char::is_whitespace);
    let cmd = it.next().unwrap_or("").to_lowercase();
    let rest = it.next().unwrap_or("").trim().to_string();
    (cmd, rest)
}

fn preview(s: &str, max: usize) -> String {
    let one = s.replace('\n', " ");
    if one.chars().count() <= max {
        one
    } else {
        let mut t: String = one.chars().take(max).collect();
        t.push('…');
        t
    }
}

/// Corre la TUI hasta que el usuario sale.
pub fn run(mut app: App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    res
}

fn event_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    while !app.quit {
        // Drenar eventos del agente antes de dibujar.
        while let Ok(ev) = app.rx.try_recv() {
            app.on_agent_event(ev);
        }
        terminal.draw(|f| draw(f, app))?;

        if event::poll(Duration::from_millis(60))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key);
                }
            }
        }
    }
    Ok(())
}

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let area = f.size();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, app, chunks[0]);
    draw_messages(f, app, chunks[1]);
    draw_input(f, app, chunks[2]);
    draw_footer(f, app, chunks[3]);
}

fn draw_header(f: &mut ratatui::Frame, app: &App, area: ratatui::layout::Rect) {
    let free = if app.free_only { " · SOLO GRATIS" } else { "" };
    let state = if app.running { " · trabajando…" } else { "" };
    let text = format!(
        " NuCoder  │ modelo: {} │ tarea: {}{}{}",
        app.model, app.task, free, state
    );
    let style = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    f.render_widget(Paragraph::new(text).style(style), area);
}

fn draw_messages(f: &mut ratatui::Frame, app: &mut App, area: ratatui::layout::Rect) {
    let block = Block::default().borders(Borders::ALL).title(" chat ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let width = inner.width.max(1) as usize;
    let mut rows: Vec<Line> = Vec::new();
    for cl in &app.lines {
        for seg in wrap(&cl.text, width) {
            rows.push(Line::from(Span::styled(seg, cl.style)));
        }
    }
    let total = rows.len() as u16;
    let view = inner.height;
    let max_off = total.saturating_sub(view);
    let offset = if app.stick {
        max_off
    } else {
        app.scroll.min(max_off)
    };
    if app.stick {
        app.scroll = max_off;
    }
    let visible: Vec<Line> = rows
        .into_iter()
        .skip(offset as usize)
        .take(view as usize)
        .collect();
    f.render_widget(Paragraph::new(visible), inner);
}

fn draw_input(f: &mut ratatui::Frame, app: &App, area: ratatui::layout::Rect) {
    let block = Block::default().borders(Borders::ALL).title(" prompt ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let text = format!("> {}", app.input);
    let style = if app.running {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::Yellow)
    };
    f.render_widget(Paragraph::new(text).style(style), inner);

    if !app.running {
        let x = inner.x + 2 + app.cursor as u16;
        let y = inner.y;
        if x < inner.x + inner.width {
            f.set_cursor(x, y);
        }
    }
}

fn draw_footer(f: &mut ratatui::Frame, app: &App, area: ratatui::layout::Rect) {
    let hint = if app.running {
        "Esc cancelar · Ctrl+C salir"
    } else {
        "Enter enviar · /refactor /debug /explain /test · /free · /clear · Ctrl+C salir"
    };
    f.render_widget(
        Paragraph::new(format!(" {hint}")).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

/// Word-wrap simple por ancho (en caracteres).
fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    for raw in text.split('\n') {
        if raw.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut len = 0usize;
        for word in raw.split(' ') {
            let wl = word.chars().count();
            if len == 0 {
                line.push_str(word);
                len = wl;
            } else if len + 1 + wl <= width {
                line.push(' ');
                line.push_str(word);
                len += 1 + wl;
            } else {
                out.push(std::mem::take(&mut line));
                line.push_str(word);
                len = wl;
            }
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_corta_por_palabras() {
        let r = wrap("hola mundo cruel", 8);
        assert_eq!(r[0], "hola");
        assert!(r.len() >= 3);
    }

    #[test]
    fn split_cmd_con_resto() {
        let (c, r) = split_cmd("/refactor arreglá esto");
        assert_eq!(c, "refactor");
        assert_eq!(r, "arreglá esto");
    }
}
