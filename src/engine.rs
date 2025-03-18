use crate::{config, Result};
use anyhow::{anyhow, Context};
use lettre::{SmtpTransport, Transport};
use std::{collections::BTreeSet, ffi::OsStr, io::Read, ops::Deref, path::PathBuf, time::Duration};
use tracing::{debug, info};

pub(crate) struct Maildir {
    pub(crate) maildir: maildir::Maildir,
    pub(crate) emails: BTreeSet<String>,
}

impl Maildir {
    pub(crate) fn new<T: Into<PathBuf>>(path: T) -> Result<Self> {
        let maildir = maildir::Maildir::from(path.into());

        let emails = maildir
            .list_cur()
            .into_iter()
            .map(|entry| entry.and_then(|entry| Ok(String::from(entry.id()))))
            .map(|entry| entry.map_err(|err| err.into()))
            .collect::<Result<BTreeSet<String>>>()?;

        maildir.create_dirs()?;
        Ok(Self { maildir, emails })
    }

    pub(crate) fn emails(&self) -> &BTreeSet<String> {
        &self.emails
    }

    pub(crate) fn print_entries(&mut self) -> Result {
        let entries = self.emails().iter();
        for (idx, id) in entries.enumerate() {
            println!("[{idx}] {}", self.email_display(id)?);
        }
        Ok(())
    }

    pub(crate) fn email_display(&self, id: &str) -> Result<String> {
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
        Ok(format!("From: {from}, Subject: {subject}"))
    }

    pub(crate) fn revive_one(&self, revive_path: impl AsRef<OsStr>, email: &str) -> Result {
        // `maildir::Maildir` has some unfortunate choices for `From`
        // implementations.
        let path: PathBuf = revive_path.as_ref().into();
        let revived_maildir = maildir::Maildir::from(path);
        revived_maildir.create_dirs()?;
        let entry = self.find(email).ok_or(anyhow!("cound not find email"))?;
        let data = std::fs::read(entry.path()).context("Cannot read email from outbox")?;
        let id = revived_maildir.store_new(&data)?;
        revived_maildir
            .move_new_to_cur_with_flags(&id, "D")
            .context("Cannot store email to drafts folder")?;
        self.delete(entry.id())
            .context("Failed to unlink after moving")?;

        Ok(())
    }

    pub(crate) fn enqueue(&self, data: &[u8]) -> Result {
        let id = self.store_new(&data)?;
        self.move_new_to_cur(&id)?;
        Ok(())
    }
}

impl Deref for Maildir {
    type Target = maildir::Maildir;

    fn deref(&self) -> &Self::Target {
        &self.maildir
    }
}

pub(crate) fn smtp_connection(config: &config::ConfigEntry) -> Result<SmtpTransport> {
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

    let pool_config = lettre::transport::smtp::SmtpTransport::builder_dangerous(&config.smtp_host)
        .credentials(credentials)
        .tls(Tls::Wrapper(tls))
        .authentication(vec![Mechanism::Plain])
        .port(config.smtp_port)
        .timeout(Some(Duration::from_secs(30)))
        .pool_config(PoolConfig::new().max_size(1));
    let sender = pool_config.build();

    Ok(sender)
}

pub(crate) fn send(
    config: &config::Config,
    profile: Option<&str>,
    idx: Option<Vec<u32>>,
) -> Result {
    config.map(profile, |profile: &str, config: &config::ConfigEntry| {
        send_one_profile(profile, config, &idx)
    })
}

pub(crate) fn send_one_profile(
    profile: &str,
    config: &config::ConfigEntry,
    idx: &Option<Vec<u32>>,
) -> Result {
    info!("Processing {profile}");

    let out_maildir = Maildir::new(config.queue_dir.clone())?;
    let emails: Vec<_> = if let Some(idx) = idx {
        let max = out_maildir.emails().len() - 1;
        if idx.iter().all(|i| (0..(max as u32)).contains(i)) {
            return Err(anyhow!("Invalid index"));
        }

        out_maildir
            .emails()
            .iter()
            .enumerate()
            .filter_map(|(i, email)| idx.contains(&(i as u32)).then_some(email))
            .collect()
    } else {
        out_maildir.emails().iter().collect()
    };

    if emails.is_empty() {
        info!("No emails");
        return Ok(());
    }

    let sent_maildir = Maildir::new(config.sent_dir.clone())?;
    sent_maildir.create_dirs()?;

    let sender = smtp_connection(config)?;
    debug!("Testing connection");
    sender
        .test_connection()
        .context("Error while testing smtp connection")?;

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

pub(crate) fn enqueue(config: &config::ConfigEntry) -> Result {
    let stdin = std::io::stdin().lock();
    let data: Vec<u8> = stdin
        .bytes()
        .collect::<Result<Vec<_>, _>>()
        .context("Failed to read stdin")?;

    let maildir = Maildir::new(&config.queue_dir)?;
    maildir.enqueue(&data)?;
    Ok(())
}
pub(crate) fn show(config: &config::Config, profile: Option<&str>) -> Result {
    config.map(profile, show_profile)
}

pub(crate) fn show_profile(profile: &str, config: &config::ConfigEntry) -> Result {
    println!("Profile {profile}:");
    let mut out_maildir = Maildir::new(config.queue_dir.clone())?;
    out_maildir.print_entries()?;
    Ok(())
}

pub(crate) fn drop_one(config: &config::Config, profile: &str, idx: u32) -> Result {
    let config = config.config_for_profile(profile)?;
    let out_maildir = Maildir::new(config.queue_dir.clone())?;
    let email = out_maildir
        .emails()
        .iter()
        .nth(idx.try_into()?)
        .ok_or(anyhow!("Invalid index"))?;
    out_maildir.delete(email)?;
    Ok(())
}

pub(crate) fn drop_all(config: &config::Config, profile: Option<&str>) -> Result {
    // TODO: Ask for confirmation.
    config.map(profile, drop_all_one_profile)
}

pub(crate) fn drop_all_one_profile(_profile: &str, config: &config::ConfigEntry) -> Result {
    let out_maildir = Maildir::new(config.queue_dir.clone())?;
    for email in out_maildir.emails().clone() {
        out_maildir.delete(&email)?;
    }
    Ok(())
}

pub(crate) fn revive_one(config: &config::Config, profile: &str, idx: u32) -> Result {
    let config = config.config_for_profile(profile)?;

    let out_maildir = Maildir::new(config.queue_dir.clone())?;
    let email = out_maildir
        .emails()
        .iter()
        .nth(idx.try_into()?)
        .ok_or(anyhow!("Invalid index"))?;
    out_maildir.revive_one(&config.revive_dir, &email)?;
    Ok(())
}

pub(crate) fn revive_all(config: &config::Config, profile: Option<&str>) -> Result {
    config.map(profile, revive_all_one_profile)
}

pub(crate) fn revive_all_one_profile(_profile: &str, config: &config::ConfigEntry) -> Result {
    let out_maildir = Maildir::new(config.queue_dir.clone())?;
    for email in out_maildir.emails() {
        out_maildir.revive_one(&config.revive_dir, email)?;
    }
    Ok(())
}
