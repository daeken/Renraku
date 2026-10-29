use renraku_vm::{
    checkpoint::CheckpointStore,
    driver::{DriverLimits, DriverStop},
    effects::{EffectState, LocalModelProvider},
    error::{Error, Result},
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
    workbench::ReferenceView,
};

fn main() {
    if let Err(error) = execute() {
        eprintln!("renraku: {error}");
        std::process::exit(1);
    }
}
fn execute() -> Result<()> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let allow_model = args.last().is_some_and(|a| a == "--allow-model-calls");
    if allow_model {
        args.pop();
    }
    const USAGE: &str = concat!(
        "usage: renraku-vm inspect <CoreLib.dll> <guest.dll>\n",
        "       renraku-vm run <CoreLib.dll> <guest.dll> <Type::Method> [instruction-budget]\n",
        "       renraku-vm run-persistent <universe.db> <CoreLib.dll> <guest.dll> <Type::Method> [budget] [checkpoint-interval]\n",
        "       renraku-vm resume <universe.db> [budget] [checkpoint-interval]\n",
        "       renraku-vm inspect-checkpoint <universe.db>\n",
        "       renraku-vm search <universe.db> <query> [limit]\n",
        "       renraku-vm read-code <universe.db> <method-or-code-ID>\n",
        "       renraku-vm export-reference <universe.db> <type-name-or-ID> [type ...]\n",
        "       renraku-vm install <universe.db> <implementation.dll> [reference-view.json ...]\n",
        "       renraku-vm invoke <universe.db> <Type::Method> <instruction-budget> [i32:N|i64:N|object:ID|static:Type::Field|null ...]\n",
        "       renraku-vm queue-model <universe.db> <prompt>\n",
        "       renraku-vm dispatch-model <universe.db> <provider.db>\n",
        "       renraku-vm reconcile-model <universe.db> <provider.db>\n",
        "       renraku-vm drive-local <universe.db> <provider.db> <instruction-budget> <provider-operation-budget> [checkpoint-interval] --allow-model-calls\n",
        "Append --allow-model-calls to run, run-persistent, or resume to enable local model calls.",
    );
    if args.as_slice() == ["--help"] || args.as_slice() == ["-h"] {
        println!("{USAGE}");
        return Ok(());
    }
    if allow_model
        && !matches!(
            args.first().map(String::as_str),
            Some("run" | "run-persistent" | "resume" | "drive-local")
        )
    {
        return Err(Error(USAGE.into()));
    }
    match args.first().map(String::as_str) {
        Some("search" | "read-code" | "export-reference" | "install" | "invoke") => {
            let command = args[0].as_str();
            let valid = match command {
                "search" => (3..=4).contains(&args.len()),
                "read-code" => args.len() == 3,
                "export-reference" => (3..=34).contains(&args.len()),
                "install" => (3..=19).contains(&args.len()),
                "invoke" => (4..=132).contains(&args.len()),
                _ => false,
            };
            if !valid {
                return Err(Error(USAGE.into()));
            }
            if !std::path::Path::new(&args[1]).is_file() {
                return Err(Error("checkpoint does not exist".into()));
            }
            let mut store = CheckpointStore::open(&args[1])?;
            let saved = store.load()?;
            let program = &saved.snapshot.program;
            match command {
                "search" => print_json(
                    &program.search_symbols(
                        &args[2],
                        usize::try_from(number(args.get(3), 20)?)
                            .map_err(|_| Error("invalid search limit".into()))?,
                    )?,
                )?,
                "read-code" => print_json(&program.read_code(&args[2])?)?,
                "export-reference" => {
                    let roots = args[2..]
                        .iter()
                        .map(|name| program.find_type(name))
                        .collect::<Result<Vec<_>>>()?;
                    println!(
                        "{}",
                        String::from_utf8(program.reference_view(&roots)?.to_json()?).unwrap()
                    );
                }
                "install" => {
                    let image = bounded_read(&args[2], 16 * 1024 * 1024)?;
                    let views = args[3..]
                        .iter()
                        .map(|path| ReferenceView::from_json(&bounded_read(path, 4 * 1024 * 1024)?))
                        .collect::<Result<Vec<_>>>()?;
                    let linked = program.link_modules(&[&image], &views)?;
                    let vm = saved
                        .snapshot
                        .restore()?
                        .install_program(&mut store, &linked)?;
                    print_json(
                        &serde_json::json!({"installed":true,"universe":vm.universe_id().to_string(),"checkpoint":store.generation()}),
                    )?;
                }
                "invoke" => {
                    let mut vm = saved.snapshot.restore()?;
                    let values = args[4..]
                        .iter()
                        .map(|arg| {
                            if arg == "null" {
                                return Ok(Value::Null);
                            }
                            let (kind, value) = arg
                                .split_once(':')
                                .ok_or_else(|| Error("invalid invocation argument".into()))?;
                            match kind {
                                "i32" => Ok(Value::I32(
                                    value
                                        .parse()
                                        .map_err(|_| Error("invalid i32 argument".into()))?,
                                )),
                                "i64" => Ok(Value::I64(
                                    value
                                        .parse()
                                        .map_err(|_| Error("invalid i64 argument".into()))?,
                                )),
                                "object" => vm
                                    .heap()
                                    .objects()
                                    .find(|object| object.id().to_string() == value)
                                    .map(|object| Value::Reference(object.id()))
                                    .ok_or_else(|| Error("unknown object argument".into())),
                                "static" => {
                                    let fields: Vec<_> = program
                                        .fields()
                                        .filter(|field| {
                                            let owner = program
                                                .types()
                                                .find(|ty| ty.id == field.owner)
                                                .unwrap();
                                            field.is_static
                                                && format!("{}::{}", owner.name, field.name)
                                                    == value
                                        })
                                        .collect();
                                    if fields.len() != 1 {
                                        return Err(Error(
                                            "static argument is missing or ambiguous".into(),
                                        ));
                                    }
                                    vm.static_fields().get(&fields[0].id).cloned().ok_or_else(
                                        || Error("static argument has no storage".into()),
                                    )
                                }
                                _ => Err(Error("unknown invocation argument kind".into())),
                            }
                        })
                        .collect::<Result<Vec<_>>>()?;
                    vm.start(program.entry(&args[2])?, values)?;
                    let outcome =
                        vm.run_checkpointed(&mut store, number(args.get(3), 100_000)?, 100)?;
                    persistent_report(&vm, &store, outcome);
                }
                _ => unreachable!(),
            }
            return Ok(());
        }
        Some("drive-local") if (5..=6).contains(&args.len()) => {
            if !allow_model {
                return Err(Error("drive-local requires --allow-model-calls".into()));
            }
            let budget = |index: usize, name: &str| {
                args[index]
                    .parse::<u64>()
                    .map_err(|_| Error(format!("invalid {name} budget")))
            };
            let limits = DriverLimits {
                instructions: budget(3, "instruction")?,
                provider_operations: budget(4, "provider operation")?,
                checkpoint_interval: number(args.get(5), 100)?,
            };
            let mut store = CheckpointStore::open(&args[1])?;
            let saved = store.load()?;
            let mut vm = saved.snapshot.restore()?;
            vm.set_model_calls_enabled(true);
            let mut provider = LocalModelProvider::open(&args[2])?;
            let report = vm.drive_model(&mut store, &mut provider, limits)?;
            let outcome = match report.stop {
                DriverStop::Completed(result) => RunOutcome::Completed(result),
                DriverStop::InstructionBudget => RunOutcome::Paused,
                DriverStop::ProviderBudget { effect } | DriverStop::OutcomeUnknown { effect } => {
                    RunOutcome::Waiting(effect)
                }
            };
            persistent_report(&vm, &store, outcome);
            println!(
                "instructions this run: {}\nprovider operations: {}",
                report.instructions, report.provider_operations
            );
            match report.stop {
                DriverStop::Completed(_) => {}
                DriverStop::InstructionBudget => println!("stopped: instruction budget exhausted"),
                DriverStop::ProviderBudget { .. } => {
                    println!("stopped: provider operation budget exhausted")
                }
                DriverStop::OutcomeUnknown { effect } => {
                    return Err(Error(format!(
                        "effect {effect} still has an uncertain outcome; no receipt available; guest remains waiting"
                    )));
                }
            }
            return Ok(());
        }
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
            vm.set_model_calls_enabled(allow_model);
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
            vm.set_model_calls_enabled(allow_model);
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
            for effect in vm.effects() {
                println!("effect: {effect:?}");
            }
            return Ok(());
        }
        Some("queue-model") if args.len() == 3 => {
            let mut store = CheckpointStore::open(&args[1])?;
            let saved = store.load()?;
            let mut vm = saved.snapshot.restore()?;
            let request = vm.new_string(args[2].encode_utf16().collect())?;
            let id = vm.queue_model_request(request)?;
            store.save(&vm)?;
            println!("queued model request: {id}");
            return Ok(());
        }
        Some("dispatch-model" | "reconcile-model") if args.len() == 3 => {
            let mut store = CheckpointStore::open(&args[1])?;
            let saved = store.load()?;
            let mut vm = saved.snapshot.restore()?;
            let mut provider = LocalModelProvider::open(&args[2])?;
            let reconcile = args[0] == "reconcile-model";
            let ids: Vec<_> = vm
                .effects()
                .filter(|effect| {
                    if reconcile {
                        matches!(
                            effect.state,
                            EffectState::InFlight | EffectState::OutcomeUnknown { .. }
                        )
                    } else {
                        effect.state == EffectState::Prepared
                    }
                })
                .map(|effect| effect.id)
                .collect();
            if !reconcile {
                let uncertain = vm
                    .effects()
                    .filter(|effect| matches!(effect.state, EffectState::OutcomeUnknown { .. }))
                    .count();
                if uncertain > 0 {
                    eprintln!(
                        "{uncertain} model request(s) have uncertain outcomes; use reconcile-model"
                    );
                }
            }
            if ids.is_empty() {
                println!(
                    "no {} model requests",
                    if reconcile { "uncertain" } else { "prepared" }
                );
            }
            for id in ids {
                let response = if reconcile {
                    vm.reconcile_model(&mut store, id, &mut provider)?
                } else {
                    Some(vm.dispatch_model(&mut store, id, &mut provider)?)
                };
                if let Some(response) = response {
                    let units = vm.heap().object(response)?.string_units().unwrap();
                    match String::from_utf16(units) {
                        Ok(text) => println!("effect {id}: {text:?}"),
                        Err(_) => println!("effect {id} UTF-16: {units:x?}"),
                    }
                } else {
                    return Err(Error(format!(
                        "effect {id} still has an uncertain outcome; no receipt available"
                    )));
                }
            }
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
        vm.set_model_calls_enabled(allow_model);
        vm.start(program.entry(entry)?, Vec::new())?;
        match vm.run(budget)? {
            RunOutcome::Waiting(effect) => println!("waiting for model request: {effect}"),
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

fn print_json(value: &serde_json::Value) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| Error(e.to_string()))?
    );
    Ok(())
}
fn bounded_read(path: &str, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error("input exceeds its size limit".into()));
    }
    Ok(bytes)
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
        RunOutcome::Waiting(effect) => println!("waiting for model request: {effect}"),
        RunOutcome::Paused => println!("paused: {} frames", vm.context().frames.len()),
        RunOutcome::Completed(result) => match result {
            Some(Value::I32(v)) => println!("result: {v}"),
            Some(Value::I64(v)) => println!("result: {v}"),
            other => println!("result: {other:?}"),
        },
    }
    println!("instructions: {}", vm.instructions_executed());
}
