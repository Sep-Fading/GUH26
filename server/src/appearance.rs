//! How the page looks, kept on the Mac so every browser shows the same. These are display
//! choices only; nothing here grants access to anything.

// Validates and stores the closed set of choices.
use serde::{Deserialize, Serialize};
// Writes a private file through a rename.
use std::{fs, io::Write, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Appearance {
    pub theme: String,
    pub text_size: String,
    pub accent: String,
    pub compact: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { theme: "system".into(), text_size: "default".into(), accent: "blue".into(),
            compact: false }
    }
}

impl Appearance {
    /// Accepts only the choices the page offers; a value is a name, never a style.
    pub fn valid(&self) -> bool {
        matches!(self.theme.as_str(), "system" | "light" | "dark")
            && matches!(self.text_size.as_str(), "small" | "default" | "large")
            && matches!(self.accent.as_str(), "blue" | "green" | "purple" | "orange" | "pink" | "graphite" | "white")
    }
}

/// Reads the saved choices; a missing or unreadable file means the defaults.
pub fn load(path: &Path) -> Appearance {
    fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Appearance>(&text).ok())
        .filter(Appearance::valid).unwrap_or_default()
}

/// Saves valid choices in a file only this user can read.
pub fn save(path: &Path, value: &Appearance) -> std::io::Result<()> {
    if !value.valid() { return Err(std::io::ErrorKind::InvalidInput.into()); }
    let staged = path.with_extension(format!("json.{}", uuid::Uuid::new_v4()));
    let mut output = crate::platform_fs::private_file(&staged, true)?;
    output.write_all(serde_json::to_string(value)?.as_bytes())?;
    output.sync_all()?;
    drop(output);
    fs::rename(staged, path)
}
