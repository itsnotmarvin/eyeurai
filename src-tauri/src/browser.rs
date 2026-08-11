//! Opening provider sign-in pages in the user's default browser.
//!
//! Callers must validate the URL against the provider's trusted hosts before
//! calling; this module only performs the OS hand-off.

use std::process::Stdio;

use crate::providers::error::ProviderError;

pub fn open_in_browser(value: &str) -> Result<(), ProviderError> {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("open");
        command.arg(value);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler").arg(value);
        command
    } else {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(value);
        command
    };
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| ProviderError::internal("could not open the sign-in page in your browser"))?;
    if status.success() {
        Ok(())
    } else {
        Err(ProviderError::internal(
            "the browser could not open the sign-in page",
        ))
    }
}
