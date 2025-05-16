use anyhow::{Context, Result, anyhow};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use human_bytes::human_bytes;
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_util::codec::Framed;
use tracing::{Instrument, Level, error, info, span, warn};
use uuid::Uuid;

use portpub_shared;

const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789-";
fn random_string(length: u8) -> Result<String, anyhow::Error> {
    if length == 0 {
        return Err(anyhow!("length must be greater than 0"));
    }

    let mut rng = thread_rng();

    let rand_string: String = (0..length)
        .map(|_| {
            CHARSET
                .choose(&mut rng)
                .copied()
                .map(|c| c as char)
                .ok_or(anyhow!("failed to generate random character"))
        })
        .collect::<Result<String, _>>()?;

    Ok(rand_string)
}

fn extract_subdomain(host: &str) -> Result<String> {
    let host = host.to_lowercase().replacen("host:", "", 1);
    let host = host.trim();
    let subdomain = host.split(".").next();
    match subdomain {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        _ => Err(anyhow!("no subdomain exists")),
    }
}

struct Server {
    port: u16,

    cli_conns: Arc<DashMap<String, Arc<Mutex<TcpStream>>>>,
    subdomain_conns: Arc<DashMap<String, Arc<Mutex<TcpStream>>>>,
}

impl Server {
    pub fn new(port: u16) -> Self {
        Server {
            port,
            cli_conns: Arc::new(DashMap::default()),
            subdomain_conns: Arc::new(DashMap::default()),
        }
    }

    async fn listen(self) -> Result<()> {
        let addr = SocketAddr::from(([0, 0, 0, 0], self.port));
        let listener = TcpListener::bind(addr)
            .await
            .context(format!("failed to run server on port {}", self.port))?;

        info!(port = &self.port, "server start");

        let this = Arc::new(self);
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(stream) => stream,
                Err(e) => {
                    error!("failed to accept connection: {}", e);
                    continue;
                }
            };

            let req_id = Uuid::new_v4();
            let span = span!(Level::INFO, "handle_connection", %req_id);

            let this = Arc::clone(&this);
            tokio::spawn(
                async move {
                    this.handle_connection(stream).await;
                }
                .instrument(span),
            );
        }
    }

    async fn handle_connection(&self, mut cli_stream: TcpStream) {
        let cli_conns = Arc::clone(&self.cli_conns);
        let subdomain_conns = Arc::clone(&self.subdomain_conns);
        {
            // first check for handling subdomain requests
            let mut buf = [0; 1024];
            let n = match cli_stream.peek(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    error!("err peeking request: {}", e);
                    return;
                }
            };

            let req_str = String::from_utf8_lossy(&buf[..n]);

            let host_line = req_str
                .lines()
                .find(|line| line.to_lowercase().starts_with("host:"));

            if host_line.is_some() {
                let subdomain = match extract_subdomain(host_line.unwrap()) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("no sub domain: {}", e);
                        return;
                    }
                };

                if let Some(the_cli_stream) = subdomain_conns.get(&subdomain) {
                    let mut cli_locked = the_cli_stream.lock().await;
                    let codec = portpub_shared::new_codec();
                    let mut framed = Framed::new(&mut *cli_locked, codec);

                    let id = Uuid::new_v4();
                    info!("new connection({})", id.to_string());
                    let msg = portpub_shared::ServerMessage::Connection(id);
                    let msg_str = match serde_json::to_string(&msg) {
                        Ok(msg) => msg,
                        Err(e) => {
                            error!(
                                "failed to marshal new connection({}) to json: {}",
                                id.to_string(),
                                e
                            );
                            return;
                        }
                    };

                    let stream = Arc::new(Mutex::new(cli_stream));
                    cli_conns.insert(id.to_string(), stream);

                    if let Err(e) = framed.send(msg_str).await {
                        error!(
                            "error while sending connection({}) message to cli: {}",
                            id.to_string(),
                            e
                        );
                    }
                }

                return;
            }
        }

        let codec = portpub_shared::new_codec();
        let mut framed = Framed::new(&mut cli_stream, codec);

        let Some(cli_msg) = framed.next().await else {
            error!("error receivin next frame");
            return;
        };

        let cli_msg = match cli_msg {
            Ok(msg) => msg,
            Err(e) => {
                error!("next frame error: {}", e);
                return;
            }
        };
        let cli_msg = match serde_json::from_slice::<portpub_shared::ClientMessage>(&cli_msg) {
            Ok(msg) => msg,
            Err(e) => {
                error!("failed parsing cli message: {}, message: {:?}", e, cli_msg);
                return;
            }
        };

        match cli_msg {
            portpub_shared::ClientMessage::Hello => {
                let mut sub_domain = match random_string(5) {
                    Ok(s) => s,
                    Err(e) => {
                        error!(
                            "failed to generate random subdomain, fallback to uuid: {}",
                            e
                        );
                        Uuid::new_v4().to_string()
                    }
                };
                let msg = portpub_shared::ServerMessage::SubDomain(sub_domain.clone());
                let msg_str = match serde_json::to_string(&msg) {
                    Ok(msg) => msg,
                    Err(e) => {
                        error!(
                            "failed to unmarshal sub_domain message: {}, message: {:?}",
                            e, msg
                        );
                        return;
                    }
                };

                if let Err(err) = framed.send(&msg_str).await {
                    error!(
                        "failed to send sub_domain message: {}, message: {:?}",
                        err, &msg_str
                    );
                }

                let cli_stream = Arc::new(Mutex::new(cli_stream));

                // check if subdomain already exists
                if subdomain_conns.get(&sub_domain).is_some() {
                    sub_domain = Uuid::new_v4().to_string();
                }
                subdomain_conns.insert(sub_domain, cli_stream);
            }
            portpub_shared::ClientMessage::Accept(id) => {
                info!("accepting: {}", id);
                if let Some((_, stream)) = cli_conns.remove(&id.to_string()) {
                    let mut cli_locked = stream.lock().await;
                    let (mut cli_read, mut cli_write) = (*cli_locked).split();
                    let (mut read, mut write) = cli_stream.split();
                    match tokio::select! {
                        res = io::copy(&mut cli_read, &mut write) => {
                            res
                        },
                        res = io::copy(&mut read, &mut cli_write) => {
                            res
                        },
                    } {
                        Ok(res) => info!("succefully copied streams: {}", human_bytes(res as f64)),
                        Err(e) => error!("failed to copy streams: {}", e),
                    };
                } else {
                    warn!("connection not found: {}", id);
                }
            }
        }

        info!("cli message processsed: {:?}", cli_msg);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let port: u16 = env::var("PORT")
        .context("PORT environment variable not set")?
        .parse()
        .context("PORT must be a valid integer")?;

    Server::new(port).listen().await?;

    Ok(())
}
