#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::var_os("TERMIUS_ASKPASS_ADDR").is_some() {
        std::process::exit(askpass());
    }
    if let Some(code) = termius_image_paste_lib::update_entry() {
        std::process::exit(code);
    }
    termius_image_paste_lib::run();
}

// OpenSSH starts this executable with redirected stdout. The only secret is
// received once over a short-lived authenticated loopback connection.
fn askpass() -> i32 {
    use std::io::{Read, Write};
    let prompt = std::env::args().nth(1).unwrap_or_default();
    if !password_prompt(&prompt) {
        return 1;
    }
    let result = (|| -> std::io::Result<()> {
        let addr: std::net::SocketAddr = std::env::var("TERMIUS_ASKPASS_ADDR")
            .unwrap_or_default()
            .parse()
            .map_err(|_| std::io::ErrorKind::InvalidInput)?;
        if !addr.ip().is_loopback() {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        let token = std::env::var("TERMIUS_ASKPASS_TOKEN").unwrap_or_default();
        if token.is_empty() || token.len() > 128 || token.contains(['\r', '\n']) {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        let mut stream =
            std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(5))?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        stream.write_all(format!("{token}\n").as_bytes())?;
        let mut reply = zeroize::Zeroizing::new(Vec::new());
        stream.take(8193).read_to_end(&mut reply)?;
        if reply.is_empty() || reply.len() > 8192 || !reply.ends_with(b"\n") {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        std::io::stdout().write_all(&reply)?;
        std::io::stdout().flush()
    })();
    if result.is_ok() {
        0
    } else {
        1
    }
}

fn password_prompt(prompt: &str) -> bool {
    let prompt = prompt.trim().to_ascii_lowercase();
    prompt == "password:" || (prompt.ends_with("'s password:") && !prompt.contains(['\n', '\r']))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_plain_password_prompts() {
        assert!(password_prompt("Password: "));
        assert!(password_prompt("codebuild@host's password: "));
        for prompt in [
            "Enter passphrase for key:",
            "Verification code:",
            "OTP password:",
            "yes/no?",
            "Password:\nVerification code:",
        ] {
            assert!(!password_prompt(prompt));
        }
    }
}
