use renraku_openai::{
    AsyncOpenAIProvider, AuthClient, OpenAIProvider, ProfileSummary, logout, models, profiles,
    select_profile,
};
use renraku_vm::{
    agent::{AgentHost, AgentLimits, AgentStop},
    checkpoint::CheckpointStore,
    driver::{DriverLimits, DriverStop},
    program::Program,
    runtime::Vm,
    value::Value,
};
use std::{path::PathBuf, process::Command, time::Duration};
mod agent_console;
use agent_console::AgentConsole;

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
    let mut allow_model = false;
    let mut allow_web = false;
    while let Some(flag) = args.last() {
        let setting = match flag.as_str() {
            "--allow-model-calls" => &mut allow_model,
            "--allow-web-publication" => &mut allow_web,
            _ => break,
        };
        if *setting {
            return Err("duplicate authorization flag".into());
        }
        *setting = true;
        args.pop();
    }
    if args.is_empty() || matches!(args[0].as_str(), "--help" | "-h") {
        println!(
            "usage: renraku-openai [--auth-dir <private-directory>] <command>\n\ncommands:\n  login [--new-profile]\n  status\n  select <profile-id>\n  refresh\n  logout\n  models <profile-id>\n  demo <universe.db> <receipt-directory> <CoreLib.dll> <guest.dll> <profile-id> <model> <prompt> --allow-model-calls\n  drive <universe.db> <receipt-directory> <profile-id> <model> <instructions> <provider-operations> --allow-model-calls"
        );
        println!(
            "  Append --allow-web-publication to agent or agent-drive to authorize website publication separately."
        );
        println!(
            "  agent <universe.db> <receipt-directory> <compiler.db> <Renraku.Compiler.dll> <CoreLib.dll> <guest.dll> <profile-id> <model> <goal> <instructions> <provider-operations> <tool-actions> --allow-model-calls\n  agent-drive <universe.db> <receipt-directory> <compiler.db> <Renraku.Compiler.dll> <profile-id> <model> <instructions> <provider-operations> <tool-actions> --allow-model-calls"
        );
        return Ok(());
    }
    if allow_web && !matches!(args[0].as_str(), "agent" | "agent-drive") {
        return Err("--allow-web-publication is only supported by agent and agent-drive".into());
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
        [
            command,
            universe,
            receipts,
            compiler_ledger,
            compiler_driver,
            core,
            guest,
            profile,
            model,
            goal,
            instructions,
            operations,
            actions,
        ] if command == "agent" => {
            if !allow_model {
                return Err("agent requires --allow-model-calls".into());
            }
            let profile = profile.parse()?;
            let limits = agent_limits(instructions, operations, actions)?;
            let mut store = CheckpointStore::open(universe)?;
            if store.generation() != 0 {
                return Err("universe already has a checkpoint; use agent-drive".into());
            }
            let program = Program::load(&[&std::fs::read(core)?, &std::fs::read(guest)?])?;
            let mut vm = Vm::new(&program);
            AgentHost::initialize(
                &mut vm,
                &mut store,
                &OpenAIProvider::capability_for(profile),
                model,
                goal,
            )?;
            let host = AgentHost {
                universe: PathBuf::from(universe),
                compiler_ledger: PathBuf::from(compiler_ledger),
                dotnet: PathBuf::from("dotnet"),
                compiler_driver: PathBuf::from(compiler_driver),
                allow_web_publication: allow_web,
            };
            drop(vm);
            drop(store);
            let console = AgentConsole {
                auth: directory.clone(),
                universe: host.universe.clone(),
                receipts: receipts.into(),
                compiler_ledger: host.compiler_ledger.clone(),
                compiler_driver: host.compiler_driver.clone(),
                profile: profile.to_string(),
                model: model.clone(),
                allow_web_publication: allow_web,
            };
            runtime()?.block_on(run_agent(&host, &console, limits))?;
        }
        [
            command,
            universe,
            receipts,
            compiler_ledger,
            compiler_driver,
            profile,
            model,
            instructions,
            operations,
            actions,
        ] if command == "agent-drive" => {
            if !allow_model {
                return Err("agent-drive requires --allow-model-calls".into());
            }
            let profile: uuid::Uuid = profile.parse()?;
            let limits = agent_limits(instructions, operations, actions)?;
            if !std::path::Path::new(universe).is_file() {
                return Err("agent universe does not exist".into());
            }
            let host = AgentHost {
                universe: PathBuf::from(universe),
                compiler_ledger: PathBuf::from(compiler_ledger),
                dotnet: PathBuf::from("dotnet"),
                compiler_driver: PathBuf::from(compiler_driver),
                allow_web_publication: allow_web,
            };
            let console = AgentConsole {
                auth: directory.clone(),
                universe: host.universe.clone(),
                receipts: receipts.into(),
                compiler_ledger: host.compiler_ledger.clone(),
                compiler_driver: host.compiler_driver.clone(),
                profile: profile.to_string(),
                model: model.clone(),
                allow_web_publication: allow_web,
            };
            runtime()?.block_on(run_agent(&host, &console, limits))?;
        }
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

fn agent_limits(
    instructions: &str,
    operations: &str,
    actions: &str,
) -> Result<AgentLimits, Box<dyn std::error::Error>> {
    let limits = AgentLimits {
        instructions: instructions.parse()?,
        provider_operations: operations.parse()?,
        tool_actions: actions.parse()?,
    };
    if limits.instructions > 1_000_000 || limits.tool_actions > 32 {
        return Err("agent limits exceed policy (1000000 instructions, 32 tool actions)".into());
    }
    Ok(limits)
}
async fn run_agent(
    host: &AgentHost,
    console: &AgentConsole,
    limits: AgentLimits,
) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!(
        "Opening saved coding session. Progress reflects saved managed turns; saved turns may be shown again on resume."
    );
    let outcome = async {
        let mut provider = AsyncOpenAIProvider::open(
            &console.auth,
            &console.receipts,
            console.profile.parse()?,
            &console.model,
        )
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(
            host.drive_with_progress(&mut provider, limits, |event| {
                eprintln!("{}", agent_console::event_line(&event));
                Ok(())
            })
            .await?,
        )
    }
    .await;
    let report = match outcome {
        Ok(report) => report,
        Err(error) => {
            console.show_failure();
            return Err(error);
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&console.report(&report, limits))?
    );
    console.show_guidance(&report.stop, limits);
    if let AgentStop::OutcomeUnknown(effect) = report.stop {
        return Err(
            format!("model effect {effect} remains uncertain; request was not resent").into(),
        );
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
