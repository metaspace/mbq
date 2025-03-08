use anyhow::{anyhow, Context, Result};
use clap::Parser;
use lettre::{SmtpTransport, Transport};
use std::{collections::BTreeSet, fs::read, io::Read, ops::Deref, path::PathBuf, time::Duration};
//use tap::prelude::*;
use tracing::{debug, info};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let args = std::env::args()
        .reduce(|acc, e| acc + &String::from(" ") + &e)
        .unwrap();
    debug!("args: {args}");

    let args = Cli::parse();
    let config = get_config(args.config)?;

    match args.command {
        Some(command) => match command {
            Command::Enqueue { profile, args: _ } => {
                enqueue(config.get(&profile).ok_or(anyhow!("Unknown profile"))?)?
            }
            Command::Show => show(&config)?,
            Command::SendAll => send_all(&config)?,
            Command::ReviveAll => revive_all(&config)?,
            Command::DropAll => drop_all(&config)?,
        },
        None => show(&config)?,
    }

    Ok(())
}

fn smtp_connection(config: &ConfigEntry) -> Result<SmtpTransport> {
    use lettre::transport::smtp::authentication::{Credentials, Mechanism};
    use lettre::transport::smtp::client::{Tls, TlsParameters};
    use lettre::transport::smtp::PoolConfig;
    let pass = std::process::Command::new("bash")
        .arg("-c")
        .arg(&config.smtp_pass_cmd)
        .output()?
        .stdout;
    let pass = String::from_utf8(pass)?.trim().to_owned();

    let credentials = Credentials::new(config.smtp_user.to_owned(), pass);

    let tls = TlsParameters::builder(config.smtp_host.clone())
        .dangerous_accept_invalid_certs(config.smtp_accept_invalid_cert)
        .build()?;

    let sender = lettre::transport::smtp::SmtpTransport::builder_dangerous(&config.smtp_host)
        .credentials(credentials)
        .tls(Tls::Wrapper(tls))
        .authentication(vec![Mechanism::Plain])
        .port(config.smtp_port)
        .timeout(Some(Duration::from_secs(30)))
        .pool_config(PoolConfig::new().max_size(1))
        .build();

    Ok(sender)
}

fn send_all(config: &Config) -> Result<()> {
    for (profile, config) in config {
        send_all_one_profile(profile, config)?;
    }
    Ok(())
}

fn send_all_one_profile(profile: &str, config: &ConfigEntry) -> Result<()> {
    info!("Processing {profile}");

    let sender = smtp_connection(config)?;
    debug!("Testing connection");
    sender.test_connection()?;

    let out_maildir = Maildir::new(config.queue_dir.clone())?;
    let emails = out_maildir.get_emails().clone();

    let sent_maildir = Maildir::new(config.sent_dir.clone())?;
    sent_maildir.create_dirs()?;

    info!("Sending all emails");
    for email in emails {
        let entry = out_maildir
            .find(&email)
            .ok_or(anyhow!("Error, could not find email"))?;
        let email_path = entry.path();
        let bytes = std::fs::read(&email_path)?;
        let email_headers = mail_parser::MessageParser::default()
            .parse_headers(&bytes)
            .ok_or(anyhow!("Failed to parse headers"))?;
        let from = email_headers
            .from()
            .ok_or(anyhow!("No From field"))?
            .first()
            .ok_or(anyhow!("No from address"))?
            .address()
            .ok_or(anyhow!("No address in From field"))?;
        let to: Vec<_> = email_headers
            .to()
            .ok_or(anyhow!("No To field"))?
            .iter()
            .map(|a| a.address().ok_or(anyhow!("No address in To field")))
            .collect::<Result<_>>()?;
        let cc: Vec<_> = email_headers.cc().map_or(Ok(Vec::default()), |cc| {
            cc.iter()
                .map(|a| a.address().ok_or(anyhow!("No address in cc field")))
                .collect::<Result<Vec<_>>>()
        })?;
        let bcc: Vec<_> = email_headers.bcc().map_or(Ok(Vec::default()), |bcc| {
            bcc.iter()
                .map(|a| a.address().ok_or(anyhow!("No address in bcc field")))
                .collect::<Result<Vec<_>>>()
        })?;

        let recipients = to
            .into_iter()
            .chain(cc.into_iter())
            .chain(bcc.into_iter())
            .map(|a| a.parse::<lettre::Address>().map_err(|e| e.into()))
            .collect::<Result<_>>()?;

        let envelope = lettre::address::Envelope::new(Some(from.parse()?), recipients)?;
        info!("Sending email");
        sender
            .send_raw(&envelope, &bytes)
            .context("Failed to send email")?;

        out_maildir.move_to(&email, &sent_maildir)?;
    }

    Ok(())
}

fn enqueue(config: &ConfigEntry) -> Result<()> {
    let stdin = std::io::stdin().lock();
    let data: Vec<u8> = stdin
        .bytes()
        .collect::<Result<Vec<_>, _>>()
        .context("Failed to read stdin")?;

    let maildir = maildir::Maildir::from(config.queue_dir.clone());
    maildir.create_dirs()?;

    let id = maildir.store_new(&data)?;
    maildir.move_new_to_cur(&id)?;

    Ok(())
}

struct Maildir {
    maildir: maildir::Maildir,
    emails: BTreeSet<String>,
}

impl Maildir {
    fn new<T: Into<PathBuf>>(path: T) -> Result<Self> {
        let maildir = maildir::Maildir::from(path.into());

        let emails = maildir
            .list_cur()
            .into_iter()
            .map(|entry| entry.and_then(|entry| Ok(String::from(entry.id()))))
            .map(|entry| entry.map_err(|err| err.into()))
            .collect::<Result<BTreeSet<String>>>()?;

        Ok(Self { maildir, emails })
    }

    fn get_emails(&self) -> &BTreeSet<String> {
        &self.emails
    }

    fn print_entries(&mut self) -> Result<()> {
        let entries = self.get_emails().iter();
        for (idx, id) in entries.enumerate() {
            let mut email = self
                .maildir
                .find(&id)
                .ok_or(anyhow!("Failed to find email"))?;
            let parsed = email.parsed()?;
            let from = parsed
                .get_headers()
                .into_iter()
                .find(|name| name.get_key_ref() == "From")
                .ok_or(anyhow!("Email has no from field"))?
                .get_value_utf8()?;
            let subject = parsed
                .get_headers()
                .into_iter()
                .find(|name| name.get_key_ref() == "Subject")
                .ok_or(anyhow!("Email has no from field"))?
                .get_value_utf8()?;
            println!("[{idx}] From: {from}, Subject: {subject}")
        }
        Ok(())
    }
}

impl Deref for Maildir {
    type Target = maildir::Maildir;

    fn deref(&self) -> &Self::Target {
        &self.maildir
    }
}

fn show(config: &Config) -> Result<()> {
    for (profile, config) in config {
        println!("Profile {profile}:");
        let mut out_maildir = Maildir::new(config.queue_dir.clone())?;
        out_maildir.print_entries()?;
    }
    Ok(())
}

fn drop_all(config: &Config) -> Result<()> {
    for (_profile, config) in config {
        let out_maildir = Maildir::new(config.queue_dir.clone())?;
        for email in out_maildir.get_emails().clone() {
            out_maildir.delete(&email)?;
        }
    }
    Ok(())
}

fn get_config(path: Option<PathBuf>) -> Result<Config> {
    let config_path = if let Some(p) = path {
        p
    } else {
        let mut p: PathBuf =
            directories_next::ProjectDirs::from("dk.metaspace", "Metaspace", "mbq")
                .ok_or(anyhow!("Failed to locate config dir"))?
                .config_dir()
                .into();
        p.push("config");
        p
    };
    let config_data = String::from_utf8(read(config_path).context("Failed to read config file")?)?;
    let mut config: Config = toml::from_str(&config_data).context("Failed to parse config file")?;

    // Expand shell escapes in paths
    for (_, config) in &mut config {
        config.queue_dir = shellexpand::full(
            config
                .queue_dir
                .to_str()
                .ok_or(anyhow!("Failed to parse queue_dir as utf8"))?,
        )?
        .into_owned()
        .into();
        config.revive_dir = shellexpand::full(
            config
                .revive_dir
                .to_str()
                .ok_or(anyhow!("Failed to parse revive_dir as utf8"))?,
        )?
        .into_owned()
        .into();
        config.sent_dir = shellexpand::full(
            config
                .sent_dir
                .to_str()
                .ok_or(anyhow!("Failed to parse sent_dir as utf8"))?,
        )?
        .into_owned()
        .into();
    }
    Ok(config)
}

fn revive_all(config: &Config) -> Result<()> {
    for (_profile, config) in config {
        let out_maildir = maildir::Maildir::from(config.queue_dir.clone());
        let revived_maildir = maildir::Maildir::from(config.revive_dir.clone());
        revived_maildir.create_dirs()?;

        for entry in out_maildir.list_cur() {
            let entry = entry?;
            let data = std::fs::read(entry.path()).context("Cannot read email from outbox")?;
            let id = revived_maildir.store_new(&data)?;
            revived_maildir
                .move_new_to_cur_with_flags(&id, "D")
                .context("Cannot store email to drafts folder")?;
            out_maildir
                .delete(entry.id())
                .context("Failed to unlink after moving")?;
        }
    }

    Ok(())
}

#[derive(clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long)]
    config: Option<PathBuf>,

    // For compatibility with `sendmail` - we discard this
    #[arg(short)]
    #[clap(hide = true)]
    ocompat: Option<String>,
}

#[derive(clap::Subcommand)]
enum Command {
    Enqueue {
        #[clap(long)]
        profile: String,

        // For compatibility with sendmail - we discard this
        #[clap(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        args: Vec<String>,
    },
    Show,
    SendAll,
    ReviveAll,
    DropAll,
}

type Config = std::collections::HashMap<String, ConfigEntry>;

#[derive(serde::Deserialize, Debug)]
struct ConfigEntry {
    queue_dir: PathBuf,
    sent_dir: PathBuf,
    revive_dir: PathBuf,
    smtp_host: String,
    smtp_port: u16,
    smtp_user: String,
    smtp_pass_cmd: String,
    #[serde(default)]
    smtp_accept_invalid_cert: bool,
}
