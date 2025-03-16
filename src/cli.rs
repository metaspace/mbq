use crate::engine::*;
use crate::Result;
use anyhow::anyhow;
use clap::Parser;
use std::path::PathBuf;

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
    Send {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, requires = "profile")]
        idx: Option<Vec<u32>>,
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
    #[command(name = "i")]
    Interactive,
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
            Command::Send { profile, idx } => send(&config, profile.as_deref(), idx)?,
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
            Command::Interactive => interactive::run(&config)?,
        },
        None => show(&config, None)?,
    }

    Ok(())
}

mod interactive {
    use crate::config;
    use crate::Result;
    use console::style;

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    enum Command {
        Show,
        Send,
        Revive,
        Drop,
        Quit,
    }

    impl Command {
        fn dispatch(&self, config: &config::Config) -> Result<bool> {
            match self {
                Command::Show => {
                    show(config)?;
                    Ok(false)
                }
                Command::Send => {
                    send(config)?;
                    Ok(false)
                }
                Command::Revive => todo!(),
                Command::Drop => todo!(),
                Command::Quit => Ok(true),
            }
        }
    }

    pub(crate) fn run(config: &config::Config) -> Result {
        cliclack::clear_screen()?;
        cliclack::intro(style("Mail Queue").on_cyan().black())?;

        loop {
            let done = cliclack::select("Command")
                // TODO: hold
                .item(Command::Show, "Show", "List emails in queue")
                .item(Command::Send, "Send", "Send emails")
                .item(Command::Revive, "Revive", "Move emails out of queue")
                .item(Command::Drop, "Drop", "Drop emails from queue (delete)")
                .item(Command::Quit, "Quit", "Quit application")
                .filter_mode()
                .interact()?
                .dispatch(config)?;
            if done {
                break;
            }
        }

        cliclack::outro("Bye!")?;
        Ok(())
    }

    fn select_profile(config: &config::Config) -> Result<Option<String>> {
        let mut items: Vec<_> = config
            .keys()
            .map(|k| (k.as_str(), k.as_str(), ""))
            .collect();
        items.push(("all", "All profiles", ""));

        let profile = cliclack::select("Profile")
            .items(&items)
            .filter_mode()
            .interact()?;

        if profile == "all" {
            Ok(None)
        } else {
            Ok(Some(profile.into()))
        }
    }

    fn show(config: &config::Config) -> Result {
        let profile = select_profile(config)?;
        crate::engine::show(config, profile.as_deref())?;
        Ok(())
    }

    fn send(config: &config::Config) -> Result {
        let profile = select_profile(config)?;

        if let Some(profile) = profile {
            let config = config.config_for_profile(&profile)?;
            let all = cliclack::confirm("Send all? (No to select individual)").interact()?;
            if all {
                crate::engine::send_one_profile(&profile, config, &None)?;
            } else {
                let out_maildir = crate::engine::Maildir::new(config.queue_dir.clone())?;
                let items: Vec<_> = out_maildir
                    .emails()
                    .iter()
                    .enumerate()
                    .map(|(i, email)| {
                        (
                            i as u32,
                            format!("[{i}] {}", out_maildir.email_display(email).unwrap()),
                            "",
                        )
                    })
                    .collect();

                if items.is_empty() {
                    cliclack::note("Warning ⚠️", "No messages")?;
                    return Ok(());
                }

                let selection = cliclack::multiselect("Select emails to send")
                    .items(&items)
                    .interact()?;

                crate::engine::send_one_profile(profile.as_str(), config, &Some(selection))?
            }
        } else {
            let confirm = cliclack::confirm("Really send all email?").interact()?;

            if !confirm {
                return Ok(());
            }

            crate::engine::send(config, profile.as_deref(), None)?;
        }

        Ok(())
    }
}
