use renraku_vm::{
    artifact::{BuildArtifact, InstallationRecord, install_once},
    checkpoint::CheckpointStore,
    compiler::{CompilationRequest, CompilationStore, CompilerToolchain},
    driver::{DriverLimits, DriverStop},
    effects::{EffectState, LocalModelProvider},
    error::{Error, Result},
    invocation::{InvocationRecord, InvocationState},
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
        "       renraku-vm install-once <universe.db> <request-ID> <bundle-directory>\n",
        "       renraku-vm installation <universe.db> <request-ID>\n",
        "       renraku-vm compile-once <universe.db> <compiler.db> <request-ID> <Renraku.Compiler.dll> <assembly-name> [--reference-view <view.json> ...] <source.cs> ...\n",
        "       renraku-vm compile-prepare <universe.db> <compiler.db> <request-ID> <Renraku.Compiler.dll> <assembly-name> [--reference-view <view.json> ...] <source.cs> ...\n",
        "       renraku-vm compile-resume <universe.db> <compiler.db> <request-ID> <Renraku.Compiler.dll>\n",
        "       renraku-vm compilation <universe.db> <compiler.db> <request-ID>\n",
        "       renraku-vm install-compilation <universe.db> <compiler.db> <compile-ID> <install-ID>\n",
        "       renraku-vm invoke <universe.db> <Type::Method> <instruction-budget> [i32:N|i64:N|object:ID|static:Type::Field|null ...]\n",
        "       renraku-vm invoke-once <universe.db> <request-ID> <Type::Method> <instruction-budget> [arguments ...]\n",
        "       renraku-vm invocation <universe.db> <request-ID>\n",
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
        Some(
            "compile-once"
            | "compile-prepare"
            | "compile-resume"
            | "compilation"
            | "install-compilation",
        ) => {
            let command = args[0].as_str();
            let valid = match command {
                "compile-once" | "compile-prepare" => (7..=70).contains(&args.len()),
                "compile-resume" | "install-compilation" => args.len() == 5,
                "compilation" => args.len() == 4,
                _ => false,
            };
            if !valid {
                return Err(Error(USAGE.into()));
            }
            if !std::path::Path::new(&args[1]).is_file() {
                return Err(Error("checkpoint does not exist".into()));
            }
            let mut universe_store = CheckpointStore::open(&args[1])?;
            let saved = universe_store.load()?;
            let vm = saved.snapshot.restore()?;
            let universe = vm.universe_id();
            let mut store = CompilationStore::open(&args[2])?;
            let compilation = match command {
                "compilation" => store.get(universe, &args[3])?,
                "install-compilation" => {
                    let result = store.get(universe, &args[3])?;
                    let artifact = result
                        .artifact
                        .ok_or_else(|| Error("compilation has no published artifact".into()))?;
                    let record = install_once(&mut universe_store, &args[4], &artifact)?;
                    print_json(&installation_json(
                        &args[4],
                        &record,
                        universe_store.generation(),
                    )?)?;
                    return Ok(());
                }
                "compile-resume" => {
                    let result = store.get(universe, &args[3])?;
                    if result.report.is_some() {
                        result
                    } else {
                        let toolchain = CompilerToolchain::open(find_dotnet()?, &args[4])?;
                        store.drive(universe, &args[3], &toolchain)?
                    }
                }
                _ => {
                    let toolchain = CompilerToolchain::open(find_dotnet()?, &args[4])?;
                    let mut index = 6;
                    let mut views = Vec::new();
                    while args.get(index).is_some_and(|arg| arg == "--reference-view") {
                        let path = args
                            .get(index + 1)
                            .ok_or_else(|| Error("reference-view requires a filename".into()))?;
                        views.push(ReferenceView::from_json(&bounded_read(
                            path,
                            4 * 1024 * 1024,
                        )?)?);
                        index += 2;
                    }
                    let sources = args[index..]
                        .iter()
                        .map(|path| {
                            let name = std::path::Path::new(path)
                                .file_name()
                                .and_then(|s| s.to_str())
                                .ok_or_else(|| Error("invalid UTF-8 source filename".into()))?
                                .to_owned();
                            Ok((name, bounded_read(path, 1024 * 1024)?))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let request = CompilationRequest::capture(
                        &saved.snapshot.program,
                        universe,
                        &toolchain,
                        args[5].clone(),
                        sources,
                        views,
                    )?;
                    let result = store.prepare(&args[3], &request)?;
                    if command == "compile-prepare" || result.report.is_some() {
                        result
                    } else {
                        store.drive(universe, &args[3], &toolchain)?
                    }
                }
            };
            let mut summary = compilation.summary()?;
            summary["requestId"] = serde_json::json!(args[3]);
            print_json(&summary)?;
            return Ok(());
        }
        Some(
            "search" | "read-code" | "export-reference" | "install" | "invoke" | "invoke-once"
            | "invocation" | "install-once" | "installation",
        ) => {
            let command = args[0].as_str();
            let valid = match command {
                "search" => (3..=4).contains(&args.len()),
                "read-code" => args.len() == 3,
                "export-reference" => (3..=34).contains(&args.len()),
                "install" => (3..=19).contains(&args.len()),
                "invoke" => (4..=132).contains(&args.len()),
                "invoke-once" => (5..=133).contains(&args.len()),
                "invocation" => args.len() == 3,
                "install-once" => args.len() == 4,
                "installation" => args.len() == 3,
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
                "read-code" => print_json(&saved.snapshot.restore()?.read_code(&args[2])?)?,
                "install-once" => {
                    let artifact = BuildArtifact::from_bundle(&args[3])?;
                    let record = install_once(&mut store, &args[2], &artifact)?;
                    print_json(&installation_json(&args[2], &record, store.generation())?)?;
                }
                "installation" => {
                    let vm = saved.snapshot.restore()?;
                    let record = vm
                        .installation(&args[2])
                        .ok_or_else(|| Error("installation request ID is not recorded".into()))?;
                    print_json(&installation_json(&args[2], record, store.generation())?)?;
                }
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
                "invocation" => {
                    let vm = saved.snapshot.restore()?;
                    let record = vm
                        .invocation(&args[2])
                        .ok_or_else(|| Error("invocation request ID is not recorded".into()))?;
                    print_json(&invocation_json(&args[2], record, &vm, store.generation()))?;
                }
                "invoke" | "invoke-once" => {
                    let mut vm = saved.snapshot.restore()?;
                    let method_index = if command == "invoke-once" { 3 } else { 2 };
                    let values = args[method_index + 2..]
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
                    let method = program.entry(&args[method_index])?;
                    let budget = number(args.get(method_index + 1), 100_000)?;
                    if command == "invoke-once" {
                        let record =
                            vm.invoke_once(&mut store, &args[2], method, values, budget, 100)?;
                        print_json(&invocation_json(&args[2], &record, &vm, store.generation()))?;
                    } else {
                        vm.start(method, values)?;
                        let outcome = vm.run_checkpointed(&mut store, budget, 100)?;
                        persistent_report(&vm, &store, outcome);
                    }
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
            for (request, record) in vm.invocations() {
                println!("invocation {request}: {record:?}");
            }
            for (request, record) in vm.installations() {
                println!(
                    "installation {}",
                    installation_json(request, record, saved.generation)?
                );
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

fn find_dotnet() -> Result<std::path::PathBuf> {
    let paths =
        std::env::var_os("PATH").ok_or_else(|| Error("dotnet host is not on PATH".into()))?;
    std::env::split_paths(&paths)
        .map(|path| {
            path.join(if cfg!(windows) {
                "dotnet.exe"
            } else {
                "dotnet"
            })
        })
        .find(|path| path.is_file())
        .ok_or_else(|| Error("dotnet host is not on PATH".into()))
}

fn invocation_json(
    request: &str,
    record: &InvocationRecord,
    vm: &Vm<'_>,
    generation: u64,
) -> serde_json::Value {
    use serde_json::json;
    let value = |v: Value| match v {
        Value::Null => json!({"kind":"null"}),
        Value::I32(v) => json!({"kind":"i32","value":v}),
        Value::I64(v) => json!({"kind":"i64","value":v.to_string()}),
        Value::Reference(id) => json!({"kind":"object","id":id.to_string()}),
    };
    let (state, result, message) = match &record.state {
        InvocationState::Running => ("running", None, None),
        InvocationState::Completed { result } => ("completed", result.map(value), None),
        InvocationState::Faulted { message } => ("faulted", None, Some(message)),
    };
    let waiting = if vm.context().id == record.execution {
        match vm.context().status {
            renraku_vm::execution::ExecutionStatus::Waiting { effect } => Some(effect.to_string()),
            _ => None,
        }
    } else {
        None
    };
    json!({"format":1,"requestId":request,"execution":record.execution.to_string(),"checkpoint":generation,
        "methodId":record.method.method.to_string(),"codeVersion":record.method.code.to_string(),
        "state":state,"result":result,"diagnostic":message,"waitingEffect":waiting})
}

fn installation_json(
    request: &str,
    record: &InstallationRecord,
    generation: u64,
) -> Result<serde_json::Value> {
    Ok(
        serde_json::json!({"format":1,"requestId":request,"state":"installed","artifactId":record.artifact.id(),
        "assemblyId":record.assembly.to_string(),"module":record.artifact.assembly_name()?,"checkpoint":generation}),
    )
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
