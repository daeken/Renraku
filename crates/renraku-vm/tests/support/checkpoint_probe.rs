//! Subprocess used by integration tests to force termination at commit boundaries.
use renraku_vm::{
    artifact::{BuildArtifact, install_once_with_observer},
    checkpoint::{CheckpointStore, CommitStage},
    compiler::{CompilationStage, CompilationStore, CompilerToolchain},
    error::{Error, Result},
    identity::UniverseId,
    program::Program,
    runtime::{RunOutcome, Vm},
};
use std::io::{Read, Write};

fn main() {
    if let Err(error) = run() {
        eprintln!("crash probe: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("compile") if args.len() == 7 => {
            let universe = UniverseId::from_bytes(
                *uuid::Uuid::parse_str(&args[2])
                    .map_err(|_| Error("invalid compile probe universe".into()))?
                    .as_bytes(),
            );
            let mut store = CompilationStore::open(&args[1])?;
            let toolchain = CompilerToolchain::open(&args[4], &args[5])?;
            let stop = match args[6].as_str() {
                "prepared" => CompilationStage::Prepared,
                "started" => CompilationStage::Started,
                "captured" => CompilationStage::Captured,
                "written" => CompilationStage::Written,
                "committed" => CompilationStage::Committed,
                _ => return Err(Error("invalid compile probe stage".into())),
            };
            store.drive_with_observer(universe, &args[3], &toolchain, |stage| {
                if stage == stop {
                    println!("READY {stage:?}");
                    std::io::stdout().flush()?;
                    std::io::stdin().read_exact(&mut [0u8; 1])?;
                }
                Ok(())
            })?;
            Ok(())
        }
        Some("install") if args.len() == 5 => {
            let mut store = CheckpointStore::open(&args[1])?;
            let artifact = BuildArtifact::from_bundle(&args[3])?;
            let stop = match args[4].as_str() {
                "prepared" => CommitStage::Prepared,
                "written" => CommitStage::Written,
                "committed" => CommitStage::Committed,
                _ => return Err(Error("invalid installation crash stage".into())),
            };
            install_once_with_observer(&mut store, &args[2], &artifact, |stage| {
                if stage == stop {
                    println!("READY {stage:?}");
                    std::io::stdout().flush()?;
                    std::io::stdin().read_exact(&mut [0u8; 1])?;
                }
                Ok(())
            })?;
            Ok(())
        }
        Some("invoke") if args.len() == 4 => {
            let mut store = CheckpointStore::open(&args[1])?;
            let saved = store.load()?;
            let mut vm = saved.snapshot.restore()?;
            vm.invoke_once(
                &mut store,
                &args[2],
                saved
                    .snapshot
                    .program
                    .entry("Renraku.Guest.Simulation::StaticRoundTrip")?,
                vec![],
                0,
                1,
            )?;
            vm.run(1000)?;
            let stop = match args[3].as_str() {
                "prepared" => CommitStage::Prepared,
                "written" => CommitStage::Written,
                "committed" => CommitStage::Committed,
                _ => return Err(Error("invalid invocation crash stage".into())),
            };
            store.save_with_observer(&vm, |stage| {
                if stage == stop {
                    println!("READY {stage:?}");
                    std::io::stdout().flush()?;
                    std::io::stdin().read_exact(&mut [0u8; 1])?;
                }
                Ok(())
            })?;
            Ok(())
        }
        Some("prepare") if matches!(args.len(), 6 | 7) => {
            let core = std::fs::read(&args[2])?;
            let guest = std::fs::read(&args[3])?;
            let program = Program::load(&[&core, &guest])?;
            let mut vm = Vm::new(&program);
            let name = args.get(6).map(String::as_str).unwrap_or("DurableRun");
            vm.start(
                program.entry(&format!("Renraku.Guest.Simulation::{name}"))?,
                vec![],
            )?;
            drive(&mut vm, &args[1], &args[4], &args[5])
        }
        Some("resume") if args.len() == 4 => {
            let mut store = CheckpointStore::open(&args[1])?;
            let saved = store.load()?;
            let mut vm = saved.snapshot.restore()?;
            drop(store);
            drive(&mut vm, &args[1], &args[2], &args[3])
        }
        _ => Err(Error("invalid crash probe arguments".into())),
    }
}
fn drive(vm: &mut Vm<'_>, path: &str, steps: &str, stop: &str) -> Result<()> {
    let steps = steps
        .parse()
        .map_err(|_| Error("invalid crash probe budget".into()))?;
    if vm.run(steps)? != RunOutcome::Paused {
        return Err(Error("crash probe completed too soon".into()));
    }
    let stop = match stop {
        "prepared" => CommitStage::Prepared,
        "written" => CommitStage::Written,
        "committed" => CommitStage::Committed,
        _ => return Err(Error("unknown crash probe stage".into())),
    };
    let mut store = CheckpointStore::open(path)?;
    store.save_with_observer(vm, |stage| {
        if stage == stop {
            println!(
                "READY {stage:?} {} {} {} {}",
                vm.universe_id(),
                vm.context().id,
                vm.instructions_executed(),
                vm.context().frames.len()
            );
            std::io::stdout().flush()?;
            // The parent keeps stdin open and kills this process at the gate.
            std::io::stdin().read_exact(&mut [0u8; 1])?;
        }
        Ok(())
    })?;
    Ok(())
}
