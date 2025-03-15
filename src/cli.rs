use std::path::PathBuf;
use clap::Parser;
use anyhow::anyhow;
use crate::engine::*;
use crate::Result;

#[derive(clap::Parser)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,

    #[arg(long)]
    pub(crate) config: Option<PathBuf>,

    // For compatibility with `sendmail` - we discard this
    #[arg(short)]
    #[clap(hide = true)]
    pub(crate) ocompat: Option<String>,
}

#[derive(clap::Subcommand)]
pub(crate) enum Command {
    Enqueue {
        #[clap(long)]
        profile: String,

        // For compatibility with sendmail - we discard this
        #[clap(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        args: Vec<String>,
    },
    Show {
        #[arg(long)]
        profile: Option<String>,
    },
    SendAll {
        #[arg(long)]
        profile: Option<String>,
    },
    Revive {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, requires = "profile")]
        idx: Option<u32>,
    },
    Drop {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, requires = "profile")]
        idx: Option<u32>,
    },
}


pub(crate) fn run() -> Result {

    let args = Cli::parse();
    let config = crate::config::Config::new(args.config)?;

    match args.command {
        Some(command) => match command {
            Command::Enqueue { profile, args: _ } => {
                enqueue(config.get(&profile).ok_or(anyhow!("Unknown profile"))?)?
            }
            Command::Show { profile } => show(&config, profile.as_deref())?,
            Command::SendAll { profile } => send_all(&config, profile.as_deref())?,
            Command::Revive { profile, idx } => {
                if let Some(idx) = idx {
                    let profile = profile.expect("expected profile to be present");
                    revive_one(&config, &profile, idx)?
                } else {
                    revive_all(&config, profile.as_deref())?
                }
            }
            Command::Drop { profile, idx } => {
                if let Some(idx) = idx {
                    let profile = profile.expect("expected profile to be present");
                    drop_one(&config, &profile, idx)?
                } else {
                    drop_all(&config, profile.as_deref())?
                }
            }
        },
        None => show(&config, None)?,
    }

    Ok(())
}
