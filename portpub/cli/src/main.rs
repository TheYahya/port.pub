use clap::Parser;
use color_eyre::Result;
use std::sync::Arc;

mod app;
use app::app::{App, Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let port = match cli.command {
        Some(Commands::Http { port }) => {
            println!("port: {}", port);
            port
        }
        None => {
            println!("invalid command, please run: portpub --help");
            return Ok(());
        }
    };

    color_eyre::install()?;
    let terminal = ratatui::init();
    let app_result = Arc::new(App::default()).run(terminal, port).await;
    ratatui::restore();
    app_result
}
