use renraku_openai::{AuthClient, ProfileSummary, logout, profiles, select_profile};
use std::{path::PathBuf, process::Command, time::Duration};

fn main() {
    if let Err(error) = run() {
        eprintln!("renraku-openai: {error}");
        std::process::exit(1);
    }
}
fn show(profile: &ProfileSummary) {
    println!(
        "profile: {}{}",
        profile.id,
        if profile.active { " (active)" } else { "" }
    );
    println!(
        "signed in: {}\nChatGPT plan permission: {}\nneeds sign-in: {}",
        profile.signed_in, profile.plan_usage, profile.needs_sign_in
    );
    if let Some(expires) = profile.expires_at {
        println!("access expiry (Unix seconds): {expires}");
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let mut directory = std::env::var_os("RENRAKU_AUTH_DIR").map(PathBuf::from);
    if args.first().is_some_and(|a| a == "--auth-dir") {
        if args.len() < 3 {
            return Err("--auth-dir requires a directory and command".into());
        }
        directory = Some(PathBuf::from(&args[1]));
        args.drain(..2);
    }
    if args.is_empty() || matches!(args[0].as_str(), "--help" | "-h") {
        println!(
            "usage: renraku-openai [--auth-dir <private-directory>] login [--new-profile]\n       renraku-openai [--auth-dir <private-directory>] status\n       renraku-openai [--auth-dir <private-directory>] select <profile-id>\n       renraku-openai [--auth-dir <private-directory>] refresh\n       renraku-openai [--auth-dir <private-directory>] logout"
        );
        return Ok(());
    }
    let directory = directory
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME").map(|p| PathBuf::from(p).join("renraku/openai"))
        })
        .or_else(|| {
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config/renraku/openai"))
        })
        .ok_or("set RENRAKU_AUTH_DIR to a private directory")?;
    match args.as_slice() {
        [command] if command == "status" => {
            let saved = profiles(directory)?;
            if saved.is_empty() {
                println!("No ChatGPT profiles. Run renraku-openai login.");
            }
            for profile in saved {
                show(&profile);
            }
        }
        [command] if command == "refresh" => show(&AuthClient::open(directory)?.refresh()?),
        [command, id] if command == "select" => show(&select_profile(directory, id.parse()?)?),
        [command] if command == "logout" => {
            let confirmed = logout(directory)?;
            println!("Active profile credentials cleared.");
            if !confirmed {
                println!(
                    "Remote revocation was not confirmed. You can disconnect Renraku in ChatGPT Settings."
                );
            }
        }
        [command] | [command, _]
            if command == "login" && (args.len() == 1 || args[1] == "--new-profile") =>
        {
            let summary = AuthClient::open(directory)?.login(
                args.len() == 2,
                Duration::from_secs(300),
                |url| {
                    println!("Continue with ChatGPT in your browser:\n{url}");
                    #[cfg(target_os = "macos")]
                    let result = Command::new("open").arg(url.as_str()).status();
                    #[cfg(not(target_os = "macos"))]
                    let result = Command::new("xdg-open").arg(url.as_str()).status();
                    if !result.is_ok_and(|status| status.success()) {
                        println!("Open the link above manually to continue.");
                    }
                    Ok(())
                },
            )?;
            show(&summary);
            if !summary.plan_usage {
                println!("Signed in, but ChatGPT plan usage was not granted.");
            }
        }
        _ => return Err("unknown command; use --help".into()),
    }
    Ok(())
}
