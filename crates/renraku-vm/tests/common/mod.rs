use renraku_vm::{identity::MethodId, program::Program};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

pub struct Fixture {
    pub core: Vec<u8>,
    pub guest: Vec<u8>,
    pub reference: BTreeMap<String, i64>,
    pub output: PathBuf,
}
pub fn fixtures() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let output = root
            .join("target")
            .join(format!("guest-tests-{}", std::process::id()));
        let result = Command::new("dotnet")
            .args(["run", "--project"])
            .arg(root.join("tools/GuestCompiler"))
            .arg("--")
            .arg(&root)
            .arg(&output)
            .output()
            .expect("integration tests require the .NET 10 SDK");
        assert!(
            result.status.success(),
            "guest compiler failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let reference = std::fs::read_to_string(output.join("reference-results.txt"))
            .unwrap()
            .lines()
            .map(|line| {
                let (name, value) = line.split_once('=').unwrap();
                (name.into(), value.parse().unwrap())
            })
            .collect();
        Fixture {
            core: std::fs::read(output.join("Renraku.CoreLib.dll")).unwrap(),
            guest: std::fs::read(output.join("Renraku.Simulation.dll")).unwrap(),
            reference,
            output,
        }
    })
}
pub fn program() -> Program {
    let fixture = fixtures();
    Program::load(&[&fixture.core, &fixture.guest]).unwrap()
}
pub fn entry(program: &Program, name: &str) -> MethodId {
    program
        .entry(&format!("Renraku.Guest.Simulation::{name}"))
        .unwrap()
}
