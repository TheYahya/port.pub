use chrono::Local;
use clap::{Parser, Subcommand};
use color_eyre::Result;
use copypasta::{ClipboardContext, ClipboardProvider};
use crossterm::event::{Event, EventStream, KeyCode};
use futures::SinkExt;
use httparse;
use portpub_shared;
use ratatui::DefaultTerminal;
use ratatui::buffer::Buffer;
use ratatui::layout::Direction;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::widgets::{
    Block, Cell, HighlightSpacing, Row, StatefulWidget, Table, TableState, Widget,
};
use ratatui::{
    Frame,
    style::Modifier,
    text::{Line, Span},
    widgets::Paragraph,
};
use std::env;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::{thread, time::Duration};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::net::TcpStream;
use tokio::task;
use tokio_stream::StreamExt;
use tokio_util::codec::Framed;

const SERVER: &str = "port.pub:4321";

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

#[derive(Debug, Clone, Default)]
struct RequestListWidget {
    state: Arc<RwLock<RequestListState>>,
}

#[derive(Debug, Default)]
struct RequestListState {
    requests: Vec<Request>,
    loading_state: LoadingState,
    top_status: String,
    sub_domain: String,
    local_port: u16,
    table_state: TableState,
}

#[derive(Debug, Clone)]
struct Request {
    time: String,
    path: String,
    method: String,
    status: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum LoadingState {
    #[default]
    Idle,
    Handling,
    Handled,
}

impl RequestListWidget {
    fn run(self: &Arc<Self>, port: u16) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            this.run_portpub(port).await;
        });
    }

    async fn run_portpub(self: Arc<Self>, port: u16) {
        self.set_local_port(port);

        let remote_socket = TcpStream::connect(SERVER)
            .await
            .expect("failed to connect to port.pub");

        let codec = portpub_shared::new_codec();
        let mut framed = Framed::new(remote_socket, codec);

        let hello_msg = match serde_json::to_string(&portpub_shared::ClientMessage::Hello) {
            Ok(msg) => msg,
            Err(e) => {
                self.set_top_state(format!("failed to marshal hello message: {}", e));
                return;
            }
        };

        if let Err(e) = framed.send(hello_msg).await {
            self.set_top_state(format!("failed to send hello message: {}", e));
            return;
        }

        while let Some(msg) = framed.next().await {
            let msg = match msg {
                Ok(msg) => msg,
                Err(e) => {
                    self.set_top_state(format!("failed to recieve new messages: {}", e));
                    return;
                }
            };

            let msg = match serde_json::from_slice::<portpub_shared::ServerMessage>(&msg) {
                Ok(msg) => msg,
                Err(e) => {
                    self.set_top_state(format!("unknown message from server: {e}"));
                    continue;
                }
            };

            match msg {
                portpub_shared::ServerMessage::Heartbeat => {}
                portpub_shared::ServerMessage::Connection(id) => {
                    let mut state = self.state.write().unwrap();
                    state.loading_state = LoadingState::Handling;

                    let this = Arc::clone(&self);

                    tokio::spawn(async move {
                        let mut remote_socket = match TcpStream::connect(SERVER).await {
                            Ok(r) => r,
                            Err(e) => {
                                this.set_top_state(format!(
                                    "failed to connecto to server ({SERVER}): {e}"
                                ));
                                return;
                            }
                        };

                        let codec = portpub_shared::new_codec();
                        let mut framed = Framed::new(&mut remote_socket, codec);

                        let accept = portpub_shared::ClientMessage::Accept(id);
                        let accept_str = match serde_json::to_string(&accept) {
                            Ok(s) => s,
                            Err(e) => {
                                this.set_top_state(format!(
                                    "failed to marshal accept message: {e}"
                                ));
                                return;
                            }
                        };

                        if let Err(e) = framed.send(accept_str).await {
                            this.set_top_state(format!("failed to send accept message: {}", e));
                            return;
                        }

                        let local_socket = match TcpStream::connect(("localhost", port)).await {
                            Ok(r) => r,
                            Err(e) => {
                                this.set_top_state(format!(
                                    "failed to connecto to local on port {port}: {e}"
                                ));
                                return;
                            }
                        };

                        let mut buf = [0; 4096];
                        let n = remote_socket.peek(&mut buf).await.unwrap();

                        let mut headers = [httparse::EMPTY_HEADER; 32];
                        let mut req = httparse::Request::new(&mut headers);
                        let _ = req.parse(&buf[..n]).unwrap();

                        let mut path = String::new();
                        let mut method = String::new();
                        if let Some(p) = req.path {
                            path = p.to_string();
                        }
                        if let Some(m) = req.method {
                            method = m.to_string();
                        }

                        let (mut s1_read, mut s1_write) = remote_socket.into_split();
                        let (s2_read, mut s2_write) = local_socket.into_split();

                        let client_to_server = tokio::spawn(async move {
                            tokio::io::copy(&mut s1_read, &mut s2_write).await
                        });

                        let server_to_client = tokio::spawn(async move {
                            let mut s2_read = PeekReader {
                                inner: s2_read,
                                status_code: None,
                            };
                            let res = tokio::io::copy(&mut s2_read, &mut s1_write).await;
                            let code = s2_read.status_code.map(|c| c);
                            (res, code)
                        });

                        let (res1, res2) = tokio::join!(client_to_server, server_to_client);

                        let status = match &res2 {
                            Ok((_, Some(code))) => code.clone(),
                            _ => 0,
                        };

                        let now = Local::now();
                        let now_string = now.format("%Y-%m-%d %H:%M:%S").to_string();
                        match (res1, res2) {
                            (Ok(Ok(_)), Ok((Ok(_), _))) => {
                                this.add_request(Request {
                                    time: now_string,
                                    method,
                                    path,
                                    status,
                                });
                            }
                            _ => {
                                this.add_request(Request {
                                    time: now_string,
                                    method,
                                    path,
                                    status: 0,
                                });
                            }
                        }
                    });
                }
                portpub_shared::ServerMessage::SubDomain(sub_domain) => {
                    self.set_subdomain(sub_domain);
                }
            };
        }
    }

    fn add_request(&self, request: Request) {
        let mut state = self.state.write().unwrap();
        state.loading_state = LoadingState::Handled;
        state.requests.extend(vec![request]);
    }

    fn set_top_state(&self, status: String) {
        self.state.write().unwrap().top_status = status;
    }

    fn set_subdomain(&self, subdomain: String) {
        self.state.write().unwrap().sub_domain = subdomain;
    }

    fn set_local_port(&self, local_port: u16) {
        self.state.write().unwrap().local_port = local_port;
    }

    fn scroll_down(&self) {
        self.state.write().unwrap().table_state.scroll_down_by(1);
    }

    fn scroll_up(&self) {
        self.state.write().unwrap().table_state.scroll_up_by(1);
    }
}

struct PeekReader<R> {
    inner: R,
    pub status_code: Option<u16>,
}

impl<R: AsyncRead + Unpin> AsyncRead for PeekReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let pre_filled = buf.filled().len();

        let result = Pin::new(&mut self.inner).poll_read(cx, buf);

        if let Poll::Ready(Ok(())) = &result {
            let new_bytes = &buf.filled()[pre_filled..];

            let mut headers = [httparse::EMPTY_HEADER; 32];
            let mut res = httparse::Response::new(&mut headers);
            match res.parse(new_bytes) {
                Ok(httparse::Status::Complete(_)) => {
                    if let Some(code) = res.code {
                        self.status_code = Some(code);
                    }
                }
                Ok(httparse::Status::Partial) => {}
                Err(_) => {}
            }
        }

        result
    }
}

impl Widget for &RequestListWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut state = self.state.write().unwrap();
        let subdomain = state.sub_domain.to_string();
        let local_port = state.local_port.to_string();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(0)])
            .split(area);

        let rows = [
            Row::new(vec![
                Cell::from("Forwarding: "),
                Cell::from(Line::from_iter([
                    Span::styled(
                        format!("http://{subdomain}.port.pub"),
                        Style::new().fg(Color::LightGreen),
                    ),
                    format!(" -> localhost:{local_port}").into(),
                ])),
            ]),
            Row::new(vec![
                Cell::from("Copy to clipboard"),
                Cell::from(Line::from_iter([Span::styled(
                    format!("<c>"),
                    Style::new().fg(Color::LightGreen),
                )])),
            ]),
        ];

        let widths = [Constraint::Length(20), Constraint::Fill(1)];
        let table = Table::new(rows, widths)
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_symbol(">>")
            .row_highlight_style(Style::new().on_blue());
        Widget::render(table, chunks[0], buf);

        let loading_state = Line::from(format!("{:?}", state.loading_state)).right_aligned();
        let block = Block::bordered()
            .title("Requests")
            .title(loading_state)
            .title_bottom("j/k to scroll, q to quit");

        let rows = state.requests.iter().rev().map(|request| {
            let status_code_color = match request.status {
                0..=399 => Color::LightGreen,
                400..=499 => Color::LightYellow,
                _ => Color::LightRed,
            };

            Row::new(vec![
                Cell::from(request.time.clone()).style(Style::default().fg(Color::Yellow)),
                Cell::from(request.status.to_string())
                    .style(Style::default().fg(status_code_color)),
                Cell::from(request.method.to_string()).style(Style::default().fg(Color::White)),
                Cell::from(request.path.clone()).style(Style::default().fg(Color::Blue)),
            ])
        });

        let widths = [
            Constraint::Length(20),
            Constraint::Max(4),
            Constraint::Max(5),
            Constraint::Fill(1),
        ];
        let table = Table::new(rows, widths)
            .block(block)
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_symbol(">>")
            .row_highlight_style(Style::new().on_dark_gray());

        StatefulWidget::render(table, chunks[1], buf, &mut state.table_state);
    }
}

impl From<&Request> for Row<'_> {
    fn from(r: &Request) -> Self {
        let r = r.clone();
        Row::new(vec![r.time, r.status.to_string(), r.method, r.path])
    }
}

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

fn is_x11_session() -> bool {
    if let Ok(session_type) = env::var("XDG_SESSION_TYPE") {
        session_type.to_lowercase() == "x11"
    } else {
        false
    }
}
