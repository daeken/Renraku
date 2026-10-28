use renraku_vm::{
    checkpoint::CheckpointStore,
    error::{Error, Result},
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};

fn main() {
    if let Err(error) = execute() {
        eprintln!("renraku: {error}");
        std::process::exit(1);
    }
}
fn execute() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    const USAGE: &str = "usage: renraku-vm inspect <CoreLib.dll> <guest.dll>\n       renraku-vm run <CoreLib.dll> <guest.dll> <Type::Method> [instruction-budget]\n       renraku-vm run-persistent <universe.db> <CoreLib.dll> <guest.dll> <Type::Method> [budget] [checkpoint-interval]\n       renraku-vm resume <universe.db> [budget] [checkpoint-interval]\n       renraku-vm inspect-checkpoint <universe.db>";
    if args.as_slice() == ["--help"] || args.as_slice() == ["-h"] {
        println!("{USAGE}");
        return Ok(());
    }
    match args.first().map(String::as_str) {
        Some("run-persistent") if (5..=7).contains(&args.len()) => {
            let mut store = CheckpointStore::open(&args[1])?;
            if store.generation() != 0 {
                return Err(Error(
                    "database already has a checkpoint; use resume".into(),
                ));
            }
            let core = std::fs::read(&args[2])?;
            let guest = std::fs::read(&args[3])?;
            let program = Program::load(&[&core, &guest])?;
            let mut vm = Vm::new(&program);
            vm.start(program.entry(&args[4])?, Vec::new())?;
            let outcome = vm.run_checkpointed(
                &mut store,
                number(args.get(5), 100_000)?,
                number(args.get(6), 100)?,
            )?;
            persistent_report(&vm, &store, outcome);
            return Ok(());
        }
        Some("resume") if (2..=4).contains(&args.len()) => {
            let mut store = CheckpointStore::open(&args[1])?;
            let saved = store.load()?;
            let mut vm = saved.snapshot.restore()?;
            let outcome = vm.run_checkpointed(
                &mut store,
                number(args.get(2), 100_000)?,
                number(args.get(3), 100)?,
            )?;
            persistent_report(&vm, &store, outcome);
            return Ok(());
        }
        Some("inspect-checkpoint") if args.len() == 2 => {
            let mut store = CheckpointStore::open(&args[1])?;
            let saved = store.load()?;
            let vm = saved.snapshot.restore()?;
            println!(
                "universe: {}\nexecution: {}\ncheckpoint: {}\ninstructions: {}\nstatus: {:?}",
                vm.universe_id(),
                vm.context().id,
                saved.generation,
                vm.instructions_executed(),
                vm.context().status
            );
            for frame in &vm.context().frames {
                println!("{frame:?}");
            }
            for object in vm.heap().objects() {
                println!("{object:?}");
            }
            println!("statics: {:?}", vm.static_fields());
            return Ok(());
        }
        _ => {}
    }
    let valid = match args.first().map(String::as_str) {
        Some("inspect") => args.len() == 3,
        Some("run") => (4..=5).contains(&args.len()),
        _ => false,
    };
    if !valid {
        return Err(Error(USAGE.into()));
    }
    let core = std::fs::read(&args[1])?;
    let guest = std::fs::read(&args[2])?;
    let program = Program::load(&[&core, &guest])?;
    if args[0] == "inspect" {
        let mut types: Vec<_> = program.types().collect();
        types.sort_by_key(|ty| &ty.name);
        for ty in types {
            println!("type {} [{}]", ty.name, ty.id);
            let mut methods: Vec<_> = program
                .methods()
                .filter(|method| method.owner == ty.id)
                .collect();
            methods.sort_by_key(|method| &method.name);
            for method in methods {
                println!(
                    "  {} {:?}, {} instructions, code {}",
                    method.name,
                    method.signature,
                    method.instructions.len(),
                    method.binding.code
                );
            }
        }
    } else {
        let entry = args
            .get(3)
            .ok_or_else(|| Error("missing Type::Method".into()))?;
        let budget = args
            .get(4)
            .map(|s| {
                s.parse::<u64>()
                    .map_err(|_| Error("invalid instruction budget".into()))
            })
            .transpose()?
            .unwrap_or(100_000);
        let mut vm = Vm::new(&program);
        vm.start(program.entry(entry)?, Vec::new())?;
        match vm.run(budget)? {
            RunOutcome::Completed(result) => {
                match result {
                    Some(Value::I32(v)) => println!("result: {v}"),
                    Some(Value::I64(v)) => println!("result: {v}"),
                    other => println!("result: {other:?}"),
                }
                println!("instructions: {}", vm.instructions_executed());
            }
            RunOutcome::Paused => {
                println!("paused after {} instructions", vm.instructions_executed());
                for frame in &vm.context().frames {
                    let method = program
                        .methods()
                        .find(|method| method.binding == frame.method)
                        .unwrap();
                    let ty = program.types().find(|ty| ty.id == method.owner).unwrap();
                    println!(
                        "frame {}::{} code {} {:?} args={:?} locals={:?} stack={:?}",
                        ty.name,
                        method.name,
                        frame.method.code,
                        frame.continuation,
                        frame.arguments,
                        frame.locals,
                        frame.evaluation_stack
                    );
                }
                for id in vm.retained_objects()? {
                    let object = vm.heap().object(id)?;
                    let ty = program
                        .types()
                        .find(|ty| ty.id == object.type_id())
                        .unwrap();
                    println!("object {id} type {}", ty.name);
                    for (field, value) in object.fields() {
                        let definition = program
                            .fields()
                            .find(|definition| definition.id == *field)
                            .unwrap();
                        println!("  {}: {value:?}", definition.name);
                    }
                }
                for (field, value) in vm.static_fields() {
                    let definition = program
                        .fields()
                        .find(|definition| definition.id == *field)
                        .unwrap();
                    println!("static {}: {value:?}", definition.name);
                }
            }
        }
    }
    Ok(())
}

fn number(argument: Option<&String>, default: u64) -> Result<u64> {
    argument
        .map(|s| {
            s.parse()
                .map_err(|_| Error("invalid instruction count".into()))
        })
        .transpose()
        .map(|v| v.unwrap_or(default))
}
fn persistent_report(vm: &Vm<'_>, store: &CheckpointStore, outcome: RunOutcome) {
    println!(
        "universe: {}\nexecution: {}\ncheckpoint: {}",
        vm.universe_id(),
        vm.context().id,
        store.generation()
    );
    match outcome {
        RunOutcome::Paused => println!("paused: {} frames", vm.context().frames.len()),
        RunOutcome::Completed(result) => match result {
            Some(Value::I32(v)) => println!("result: {v}"),
            Some(Value::I64(v)) => println!("result: {v}"),
            other => println!("result: {other:?}"),
        },
    }
    println!("instructions: {}", vm.instructions_executed());
}
