use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub handle: String,
    pub server: String,
    pub theme: String,
    pub default_problems: usize,
    pub default_duration: u32,
    pub default_bots: usize,
    pub open_command: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            handle: String::new(),
            server: "ws://127.0.0.1:7331".into(),
            theme: "default".into(),
            default_problems: 5,
            default_duration: 120,
            default_bots: 4,
            open_command: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(dirs::config_dir)
            .context("no config directory; set XDG_CONFIG_HOME")?;
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(dirs::data_local_dir)
            .context("no data directory; set XDG_DATA_HOME")?;
        Ok(Self {
            config: config.join("cfvirtual/config.toml"),
            data: data.join("cfvirtual"),
        })
    }
    pub fn db(&self) -> PathBuf {
        self.data.join("cfvirtual.db")
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).context("read configuration"),
        };
        let value: toml::Value = toml::from_str(&text).context("invalid config TOML")?;
        let known = [
            "handle",
            "server",
            "theme",
            "default_problems",
            "default_duration",
            "default_bots",
            "open_command",
        ]
        .into_iter()
        .collect::<HashSet<_>>();
        if let Some(table) = value.as_table() {
            for key in table.keys().filter(|k| !known.contains(k.as_str())) {
                eprintln!("Warning: ignoring unknown config key {key}");
            }
        }
        let config: Self = value.try_into().context("invalid config values")?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if !self.handle.is_empty() {
            validate_handle(&self.handle)?;
        }
        validate_server(&self.server)?;
        if !["default", "colorblind"].contains(&self.theme.as_str()) {
            bail!("theme must be default or colorblind");
        }
        if !(1..=26).contains(&self.default_problems) {
            bail!("default_problems must be 1..26");
        }
        if !(90..=1440).contains(&self.default_duration) {
            bail!("default_duration must be 90..1440 minutes");
        }
        if self.default_bots > 16 {
            bail!("default_bots must be 0..16");
        }
        Ok(())
    }

    pub fn set(&mut self, key: &str, value: &str, path: &Path) -> Result<()> {
        let mut next = self.clone();
        match key {
            "handle" => next.handle = value.into(),
            "server" => next.server = value.into(),
            "theme" => next.theme = value.into(),
            "default_problems" => {
                next.default_problems = value.parse().context("expected problem count")?
            }
            "default_duration" => {
                next.default_duration = value.parse().context("expected minutes")?
            }
            "default_bots" => next.default_bots = value.parse().context("expected bot count")?,
            "open_command" => next.open_command = value.into(),
            _ => bail!("unknown config key: {key}"),
        }
        next.validate()?;
        next.save(path)?;
        *self = next;
        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let parent = path.parent().context("config path has no parent")?;
        std::fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(".config-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(toml::to_string_pretty(self)?.as_bytes())?;
            file.sync_all()?;
            // Windows rename does not replace an existing destination. Use the
            // OS's atomic replacement primitive instead of delete-then-rename.
            atomic_replace(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.context("write config atomically")
    }
}

#[cfg(not(windows))]
fn atomic_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

#[cfg(windows)]
fn atomic_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // Both buffers are NUL-terminated and live until this synchronous call returns.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn validate_handle(handle: &str) -> Result<()> {
    if handle.is_empty()
        || handle.len() > 24
        || !handle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        bail!("invalid public handle: use 1..24 ASCII letters, digits, '.', '_' or '-'");
    }
    Ok(())
}

pub fn validate_server(server: &str) -> Result<()> {
    let parsed = url::Url::parse(server).context("invalid server URL")?;
    if !["ws", "wss"].contains(&parsed.scheme())
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        bail!("server must be ws:// or wss:// without credentials or fragment");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_is_atomic_validated_and_round_trips() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        let mut config = Config::load(&path)?;
        config.set("handle", "practice_player", &path)?;
        config.set("default_problems", "6", &path)?;
        assert!(config.set("default_duration", "89", &path).is_err());
        config.set("default_duration", "90", &path)?;
        let before = std::fs::read(&path)?;
        assert!(config.set("default_problems", "27", &path).is_err());
        assert_eq!(std::fs::read(&path)?, before);
        assert_eq!(Config::load(&path)?.default_problems, 6);
        assert_eq!(Config::load(&path)?.handle, "practice_player");
        assert_eq!(Config::default().default_duration, 120);
        Ok(())
    }
    #[test]
    fn unknown_keys_do_not_break_loading() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "future_option = true\ntheme = 'colorblind'\n")?;
        assert_eq!(Config::load(&path)?.theme, "colorblind");
        assert!(validate_server("ws://user:secret@example.invalid").is_err());
        Ok(())
    }
}
