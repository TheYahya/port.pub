use clap::{Parser, Subcommand};
use futures::{SinkExt, StreamExt};
use portpub_shared;
use tokio::io;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

const SERVER: &str = "port.pub:80";

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    name: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Http {
        #[clap(short, long)]
        port: u16,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let port = match cli.command {
        Some(Commands::Http { port }) => {
            println!("port: {}", port);
            port
        }
        None => {
            println!("invalid command, please run: portpub --help");
            return;
        }
    };

    let remote_socket = TcpStream::connect(SERVER)
        .await
        .expect("failed to connect to port.pub. please try again!");

    let codec = portpub_shared::new_codec();
    let mut framed = Framed::new(remote_socket, codec);

    let hello_msg = match serde_json::to_string(&portpub_shared::ClientMessage::Hello) {
        Ok(msg) => msg,
        Err(e) => {
            eprintln!("failed to create marshal hello message: {}", e);
            return;
        }
    };

    if let Err(e) = framed.send(hello_msg).await {
        eprintln!("failed to send hello message to port.pub: {}", e);
        return;
    }

    while let Some(msg) = framed.next().await {
        let msg = match msg {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!("failed to recieve new messages: {}", e);
                return;
            }
        };

        let msg = match serde_json::from_slice::<portpub_shared::ServerMessage>(&msg) {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!("unknown message from server: {e}");
                continue;
            }
        };

        match msg {
            portpub_shared::ServerMessage::Connection(id) => {
                tokio::spawn(async move {
                    let mut remote_socket = match TcpStream::connect(SERVER).await {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("failed to connecto to server ({SERVER}): {e}");
                            return;
                        }
                    };

                    let codec = portpub_shared::new_codec();
                    let mut framed = Framed::new(&mut remote_socket, codec);

                    let accept = portpub_shared::ClientMessage::Accept(id);
                    let accept_str = match serde_json::to_string(&accept) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("failed to marshal accept message: {e}");
                            return;
                        }
                    };

                    if let Err(e) = framed.send(accept_str).await {
                        eprintln!("failed to send accept message: {}", e);
                        return;
                    }

                    let mut local_socket = match TcpStream::connect(("localhost", port)).await {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("failed to connecto to local on port {port}: {e}");
                            return;
                        }
                    };

                    let (mut s1_read, mut s1_write) = remote_socket.split();
                    let (mut s2_read, mut s2_write) = local_socket.split();

                    match tokio::select! {
                        res = io::copy(&mut s1_read, &mut s2_write) => {
                          res
                        },
                        res = io::copy(&mut s2_read, &mut s1_write) => {
                            res
                        },
                    } {
                        Ok(_) => {
                            println!("request {id} proccesed");
                        }
                        Err(e) => {
                            eprintln!("failed to copy streams: {e}");
                        }
                    };
                });
            }
            portpub_shared::ServerMessage::SubDomain(sub_domain) => {
                println!("port published at: {sub_domain}.port.pub");
            }
        };
    }
}
