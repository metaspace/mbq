use std::ops::DerefMut;
use std::ops::Deref;
use std::fs::read;
use super::Result;
use std::path::PathBuf;
use anyhow::anyhow;
use anyhow::Context;

pub(crate) type ConfigInner = std::collections::HashMap<String, ConfigEntry>;

pub(crate) struct Config(ConfigInner);

impl Config {
    pub(crate) fn new(path: Option<PathBuf>) -> Result<Self> {
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
        let config_data =
            String::from_utf8(read(config_path).context("Failed to read config file")?)?;
        let mut config: Config = Config::from_str(config_data)?;

        // Expand shell escapes in paths
        for (_, config) in config.deref_mut() {
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

    pub(crate) fn from_str<T: AsRef<str>>(data: T) -> Result<Self> {
        Ok(Self(
            toml::from_str(data.as_ref()).context("Failed to parse config file")?,
        ))
    }

    pub(crate) fn config_for_profile(&self, profile: impl AsRef<str>) -> Result<&ConfigEntry> {
        self.0
            .get(profile.as_ref())
            .ok_or(anyhow!("Profile not found in config"))
    }

    pub(crate) fn map(&self, profile: Option<&str>, f: impl Fn(&str, &ConfigEntry) -> Result) -> Result {
        if let Some(profile) = profile {
            let config = self.config_for_profile(profile)?;
            f(profile, config)?;
        } else {
            for (profile, config) in &self.0 {
                f(&profile, &config)?;
            }
        }
        Ok(())
    }
}

impl Deref for Config {
    type Target = ConfigInner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Config {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(serde::Deserialize, Debug)]
pub(crate) struct ConfigEntry {
    pub(crate) queue_dir: PathBuf,
    pub(crate) sent_dir: PathBuf,
    pub(crate) revive_dir: PathBuf,
    pub(crate) smtp_host: String,
    pub(crate) smtp_port: u16,
    pub(crate) smtp_user: String,
    pub(crate) smtp_pass_cmd: String,
    #[serde(default)]
    pub(crate) smtp_accept_invalid_cert: bool,
}
