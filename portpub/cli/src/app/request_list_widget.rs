use chrono::Local;
use futures::SinkExt;
use httparse;
use portpub_shared;
use ratatui::buffer::Buffer;
use ratatui::layout::Direction;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Cell, HighlightSpacing, Row, StatefulWidget, Table, TableState, Widget,
};
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::net::TcpStream;
use tokio_stream::StreamExt;
use tokio_util::codec::Framed;

const SERVER: &str = "port.pub:4321";

#[derive(Debug, Clone, Default)]
pub struct RequestListWidget {
    pub state: Arc<RwLock<RequestListState>>,
}

#[derive(Debug, Default)]
pub struct RequestListState {
    requests: Vec<Request>,
    loading_state: LoadingState,
    pub top_status: String,
    pub sub_domain: String,
    pub local_port: u16,
    pub table_state: TableState,
}

#[derive(Debug, Clone)]
struct Request {
    pub time: String,
    pub path: String,
    pub method: String,
    pub status: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum LoadingState {
    #[default]
    Idle,
    Handling,
    Handled,
}

impl RequestListWidget {
    pub fn run(self: &Arc<Self>, port: u16) {
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

    pub fn scroll_down(&self) {
        self.state.write().unwrap().table_state.scroll_down_by(1);
    }

    pub fn scroll_up(&self) {
        self.state.write().unwrap().table_state.scroll_up_by(1);
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
