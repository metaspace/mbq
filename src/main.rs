use tracing::debug;

mod cli;
mod config;
mod engine;

type Result<T = (), E = anyhow::Error> = anyhow::Result<T, E>;

fn main() -> Result {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let args = std::env::args()
        .reduce(|acc, e| acc + &String::from(" ") + &e)
        .unwrap();
    debug!("args: {args}");

    cli::run()?;

    Ok(())
}


