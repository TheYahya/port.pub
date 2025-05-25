use super::request_list_widget::RequestListWidget;
use clap::{Parser, Subcommand};
use color_eyre::Result;
use copypasta::{ClipboardContext, ClipboardProvider};
use crossterm::event::{Event, EventStream, KeyCode};
use ratatui::DefaultTerminal;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::{
    Frame,
    style::Modifier,
    text::{Line, Span},
    widgets::Paragraph,
};
use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{thread, time::Duration};
use tokio::task;
use tokio_stream::StreamExt;

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    name: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    Http {
        #[clap(short, long)]
        port: u16,
    },
}

#[derive(Debug, Default)]
pub struct App {
    should_quit: AtomicBool,
    requests: Arc<RequestListWidget>,
}

impl App {
    const FRAMES_PER_SECOND: f32 = 60.0;

    pub async fn run(self: Arc<Self>, mut terminal: DefaultTerminal, port: u16) -> Result<()> {
        self.requests.run(port);

        let period = Duration::from_secs_f32(1.0 / Self::FRAMES_PER_SECOND);
        let mut interval = tokio::time::interval(period);
        let mut events = EventStream::new();

        let this = Arc::clone(&self);

        while !self.should_quit.load(Ordering::SeqCst) {
            tokio::select! {
                _ = interval.tick() => {
                    terminal.draw(|frame| this.render(frame))?;
                },
                Some(Ok(event)) = events.next() => {
                    this.handle_event(&event).await;
                }
            }
        }
        Ok(())
    }

    fn render(&self, frame: &mut Frame) {
        let vertical = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]);
        let [title_area, body_area] = vertical.areas(frame.area());

        let title_lines = vec![
            Line::from(Span::styled(
                "port.pub",
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .centered(),
        ];
        let title_paragraph = Paragraph::new(title_lines);

        frame.render_widget(title_paragraph, title_area);

        frame.render_widget(&*self.requests, body_area);
    }

    async fn handle_event(self: &Arc<Self>, event: &Event) {
        let this = Arc::clone(self);
        if let Some(key) = event.as_key_press_event() {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => this.should_quit.store(true, Ordering::SeqCst),
                KeyCode::Char('j') | KeyCode::Down => self.requests.scroll_down(),
                KeyCode::Char('k') | KeyCode::Up => self.requests.scroll_up(),
                KeyCode::Char('c') => {
                    task::spawn_blocking(move || {
                        let sub_domain = this.requests.state.read().unwrap().sub_domain.clone();
                        let domain = format!("http://{}.port.pub", sub_domain);
                        let mut clipboard = match ClipboardContext::new() {
                            Ok(c) => c,
                            // TODO: handle error properly
                            Err(_) => {
                                return;
                            }
                        };

                        match clipboard.set_contents(domain.to_owned()) {
                            Ok(_) => {}
                            // TODO: handle error properly
                            Err(_) => {
                                return;
                            }
                        }

                        if is_x11_session() {
                            // HACK: sleeping for to keep serving memory for x11
                            thread::sleep(Duration::from_secs(60 * 10));
                        }
                    });
                }
                _ => {}
            }
        }
    }
}

fn is_x11_session() -> bool {
    if let Ok(session_type) = env::var("XDG_SESSION_TYPE") {
        session_type.to_lowercase() == "x11"
    } else {
        false
    }
}
