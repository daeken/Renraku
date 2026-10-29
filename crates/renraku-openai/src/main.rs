use renraku_openai::{
    AsyncOpenAIProvider, AuthClient, OpenAIProvider, ProfileSummary, logout, models, profiles,
    select_profile,
};
use renraku_vm::{
    checkpoint::CheckpointStore,
    driver::{DriverLimits, DriverStop},
    program::Program,
    runtime::Vm,
    value::Value,
};
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
    let allow_model = args.last().is_some_and(|a| a == "--allow-model-calls");
    if allow_model {
        args.pop();
    }
    if args.is_empty() || matches!(args[0].as_str(), "--help" | "-h") {
        println!(
            "usage: renraku-openai [--auth-dir <private-directory>] <command>\n\ncommands:\n  login [--new-profile]\n  status\n  select <profile-id>\n  refresh\n  logout\n  models <profile-id>\n  demo <universe.db> <receipt-directory> <CoreLib.dll> <guest.dll> <profile-id> <model> <prompt> --allow-model-calls\n  drive <universe.db> <receipt-directory> <profile-id> <model> <instructions> <provider-operations> --allow-model-calls"
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
        [command, id] if command == "models" => {
            for model in models(&directory, id.parse()?)? {
                println!("{}\t{}", model.slug, model.display_name);
            }
        }
        [
            command,
            universe,
            receipts,
            core,
            guest,
            profile,
            model,
            prompt,
        ] if command == "demo" => {
            if !allow_model {
                return Err("demo requires --allow-model-calls".into());
            }
            let profile = profile.parse()?;
            let mut store = CheckpointStore::open(universe)?;
            if store.generation() != 0 {
                return Err("universe already has a checkpoint; use drive".into());
            }
            let program = Program::load(&[&std::fs::read(core)?, &std::fs::read(guest)?])?;
            let mut vm = Vm::new(&program);
            let arguments = [
                OpenAIProvider::capability_for(profile),
                model.clone(),
                prompt.clone(),
            ]
            .into_iter()
            .map(|text| {
                vm.new_string(text.encode_utf16().collect())
                    .map(Value::Reference)
            })
            .collect::<renraku_vm::error::Result<_>>()?;
            vm.start(
                program.entry("Renraku.Guest.Simulation::OpenAIHarness")?,
                arguments,
            )?;
            store.save(&vm)?;
            runtime()?.block_on(async {
                let mut provider =
                    AsyncOpenAIProvider::open(&directory, receipts, profile, model).await?;
                drive(&mut vm, &mut store, &mut provider, 10_000, 1).await
            })?;
        }
        [
            command,
            universe,
            receipts,
            profile,
            model,
            instructions,
            operations,
        ] if command == "drive" => {
            if !allow_model {
                return Err("drive requires --allow-model-calls".into());
            }
            let instructions = instructions.parse()?;
            let operations = operations.parse()?;
            let mut store = CheckpointStore::open(universe)?;
            let saved = store.load()?;
            let mut vm = saved.snapshot.restore()?;
            runtime()?.block_on(async {
                let mut provider =
                    AsyncOpenAIProvider::open(&directory, receipts, profile.parse()?, model)
                        .await?;
                drive(&mut vm, &mut store, &mut provider, instructions, operations).await
            })?;
        }
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

fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(4)
        .build()
}
async fn drive(
    vm: &mut Vm<'_>,
    store: &mut CheckpointStore,
    provider: &mut AsyncOpenAIProvider,
    instructions: u64,
    operations: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    vm.set_model_calls_enabled(true);
    let report = vm
        .drive_model_async(
            store,
            provider,
            DriverLimits {
                instructions,
                provider_operations: operations,
                checkpoint_interval: 100,
            },
        )
        .await?;
    println!(
        "checkpoint: {}\ninstructions this run: {}\nprovider operations: {}",
        store.generation(),
        report.instructions,
        report.provider_operations
    );
    match report.stop {
        DriverStop::Completed(Some(Value::Reference(id))) => {
            let units = vm.heap().object(id)?.string_units().ok_or("guest result is not a string")?;
            println!("{}", String::from_utf16(units)?);
        }
        DriverStop::Completed(result) => println!("completed: {result:?}"),
        DriverStop::InstructionBudget => println!("paused: instruction budget exhausted; use drive to continue"),
        DriverStop::ProviderBudget { effect } => println!("waiting: {effect}; provider operation budget exhausted"),
        DriverStop::OutcomeUnknown { effect } => return Err(format!("effect {effect} is still uncertain; no local completion receipt; request was not resent").into()),
    }
    Ok(())
}
