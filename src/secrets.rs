//! Helpers for the three layers of password-manager integration:
//!
//! 1. **Clipboard** — copy/paste of secrets via `arboard`, cross-platform.
//! 2. **OS keychain** — read/write of secrets via `keyring` (Apple Keychain
//!    on macOS, Secret Service on Linux, Credential Manager on Windows).
//!    Used to remember the recovery key locally so future restores happen
//!    without user intervention.
//! 3. **Custom PM command** — shell out to a user-configured command
//!    (e.g. `bw get password ...`, `pass show matrix/recovery`,
//!    `op item get matrix --field password`) and use its stdout. Lets
//!    matcurses pull secrets from any password manager that exposes a
//!    CLI, without hard-coding any specific vendor.

const KEYRING_SERVICE: &str = "matcurses";

pub fn copy_to_clipboard(text: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text)?;
    Ok(())
}

pub fn store_recovery_key(
    mxid: &str,
    key: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, &format!("recovery:{mxid}"))?;
    entry.set_password(key)?;
    Ok(())
}

pub fn load_recovery_key(
    mxid: &str,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, &format!("recovery:{mxid}"))?;
    match entry.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(Box::new(e)),
    }
}

#[allow(dead_code)]
pub fn delete_recovery_key(
    mxid: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, &format!("recovery:{mxid}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(Box::new(e)),
    }
}

/// Run a user-configured shell command and return its stdout (trimmed).
/// The command string is split with shell-style word rules so things like
/// quoted arguments work as expected.
pub fn run_pm_command(cmd: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return Err("commande PM vide".into());
    }
    let parts = shell_words::split(cmd).map_err(|e| format!("parse commande : {e}"))?;
    if parts.is_empty() {
        return Err("commande PM invalide".into());
    }
    let output = std::process::Command::new(&parts[0])
        .args(&parts[1..])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("commande PM a échoué : {}", stderr.trim()).into());
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        return Err("commande PM : stdout vide".into());
    }
    Ok(s)
}
