//! `dotfiles-tui` — the human surface (ADR-001 #4): a live projection of the
//! always-fresh dotfiles state (ADR-005), inventory-first.
//!
//! v0.1: renders the derived inventory (catalog + deploy status) — the primary
//! altitude (ADR-002). The always-fresh watch loop and the change-detail diff
//! view (ADR-100) land in the next slices; for now the state is read once.

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use dotfiles_core::{DeployStatus, EntryState, Manifest, State};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, List, ListItem, ListState, Paragraph, Wrap},
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "dotfiles-tui", version, about = "Inventory projection of your dotfiles")]
struct Args {
    /// Path to the TOML manifest.
    #[arg(long, default_value = ".dotfiles-manifest.toml")]
    manifest: PathBuf,
    /// Repo root that source paths resolve against (default: manifest's dir).
    #[arg(long)]
    repo_root: Option<PathBuf>,
    /// Home dir that target paths resolve against (default: $HOME).
    #[arg(long)]
    home: Option<PathBuf>,
}

struct App {
    state: State,
    list_state: ListState,
}

impl App {
    fn new(state: State) -> Self {
        let mut list_state = ListState::default();
        if !state.entries.is_empty() {
            list_state.select(Some(0));
        }
        App { state, list_state }
    }

    fn selected(&self) -> Option<&EntryState> {
        self.list_state
            .selected()
            .and_then(|i| self.state.entries.get(i))
    }

    fn next(&mut self) {
        let n = self.state.entries.len();
        if n == 0 {
            return;
        }
        let i = self.list_state.selected().map_or(0, |i| (i + 1) % n);
        self.list_state.select(Some(i));
    }

    fn prev(&mut self) {
        let n = self.state.entries.len();
        if n == 0 {
            return;
        }
        let i = self.list_state.selected().map_or(0, |i| (i + n - 1) % n);
        self.list_state.select(Some(i));
    }
}

/// Display label + color for a deploy status (presentation only).
fn status_view(s: &DeployStatus) -> (&'static str, Color) {
    match s {
        DeployStatus::Linked => ("linked", Color::Green),
        DeployStatus::Present => ("present", Color::Green),
        DeployStatus::Missing => ("missing", Color::Yellow),
        DeployStatus::Conflict => ("conflict", Color::Red),
        DeployStatus::Broken => ("broken", Color::Red),
        DeployStatus::WrongTarget { .. } => ("wrong-target", Color::Red),
        DeployStatus::Error { .. } => ("error", Color::Red),
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let [main, footer] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(main);

    // Inventory list.
    let items: Vec<ListItem> = app
        .state
        .entries
        .iter()
        .map(|es| {
            let (label, color) = status_view(&es.status);
            let enabled = if es.entry.enabled { " " } else { "-" };
            ListItem::new(Line::from(vec![
                Span::raw(format!("{enabled} ")),
                Span::raw(format!("{:20}", es.entry.name)),
                Span::styled(format!("{label:>12}"), Style::default().fg(color)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(Block::bordered().title(" dotfiles · inventory "))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    f.render_stateful_widget(list, list_area, &mut app.list_state);

    // Detail / why pane.
    let detail = match app.selected() {
        Some(es) => {
            let (label, color) = status_view(&es.status);
            let why = es.entry.why.as_deref().unwrap_or("(no rationale recorded)");
            Text::from(vec![
                Line::from(Span::styled(
                    es.entry.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::raw(""),
                Line::raw(format!("target  {}", es.entry.target)),
                Line::raw(format!("source  {}", es.entry.path)),
                Line::raw(format!("mode    {}", es.entry.mode)),
                Line::from(vec![
                    Span::raw("status  "),
                    Span::styled(label, Style::default().fg(color)),
                ]),
                Line::raw(""),
                Line::from(Span::styled("why", Style::default().add_modifier(Modifier::DIM))),
                Line::raw(why.to_string()),
            ])
        }
        None => Text::raw("no entries"),
    };
    let para = Paragraph::new(detail)
        .block(Block::bordered().title(" why "))
        .wrap(Wrap { trim: true });
    f.render_widget(para, detail_area);

    // Footer.
    let foot = Paragraph::new(" ↑/↓ navigate · q quit ")
        .style(Style::default().add_modifier(Modifier::DIM));
    f.render_widget(foot, footer);
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> anyhow::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Down | KeyCode::Char('j') => app.next(),
                KeyCode::Up | KeyCode::Char('k') => app.prev(),
                _ => {}
            }
        }
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let src = std::fs::read_to_string(&args.manifest)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", args.manifest.display()))?;
    let manifest = Manifest::from_toml(&src)?;

    let repo_root = args
        .repo_root
        .or_else(|| args.manifest.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let home = args
        .home
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .ok_or_else(|| anyhow::anyhow!("no --home and $HOME unset"))?;

    if let Err(msg) = dotfiles_core::first_run_gate(&repo_root) {
        eprintln!("dotfiles-tui: {msg}");
        std::process::exit(2);
    }

    let state = State::derive(&manifest, &repo_root, &home);
    let mut app = App::new(state);

    let mut terminal = ratatui::init();
    let res = run(&mut terminal, &mut app);
    ratatui::restore();
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotfiles_core::{Entry, Mode};

    fn state_with(n: usize) -> State {
        State {
            entries: (0..n)
                .map(|i| EntryState {
                    entry: Entry {
                        name: format!("e{i}"),
                        path: "p".into(),
                        target: "t".into(),
                        enabled: true,
                        mode: Mode::Symlink,
                        why: None,
                    },
                    status: DeployStatus::Linked,
                })
                .collect(),
        }
    }

    #[test]
    fn navigation_wraps() {
        let mut app = App::new(state_with(3));
        assert_eq!(app.list_state.selected(), Some(0));
        app.prev();
        assert_eq!(app.list_state.selected(), Some(2));
        app.next();
        assert_eq!(app.list_state.selected(), Some(0));
    }

    #[test]
    fn navigation_on_empty_is_safe() {
        let mut app = App::new(state_with(0));
        app.next();
        app.prev();
        assert_eq!(app.list_state.selected(), None);
    }

    #[test]
    fn renders_inventory_and_why() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = App::new(state_with(2));
        app.state.entries[0].entry.name = "zsh".into();
        app.state.entries[0].entry.why = Some("shell baseline".into());

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|f| ui(f, &mut app)).unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();

        assert!(content.contains("inventory"), "list title rendered");
        assert!(content.contains("zsh"), "entry name rendered");
        assert!(content.contains("linked"), "deploy status rendered");
        assert!(content.contains("shell baseline"), "why docstring rendered");
    }
}
