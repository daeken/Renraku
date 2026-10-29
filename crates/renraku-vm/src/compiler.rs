//! Durable host compiler jobs. CLR processes are transient; inputs and terminal
//! artifacts/diagnostics live in a separate service ledger, not guest frames.
use crate::{
    artifact::{BuildArtifact, decode_capture, encode_capture, hash},
    error::{Error, Result, require},
    identity::UniverseId,
    image::Image,
    invocation::validate_request_id,
    program::Program,
    workbench::ReferenceView,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const PACKAGE: [&str; 5] = [
    "Renraku.Compiler.dll",
    "Renraku.Compiler.deps.json",
    "Renraku.Compiler.runtimeconfig.json",
    "Microsoft.CodeAnalysis.dll",
    "Microsoft.CodeAnalysis.CSharp.dll",
];
const MAX_INPUT: usize = 32 * 1024 * 1024;
const MAX_REPORT: usize = 512 * 1024;
const APPLICATION: i64 = 0x52454343;

fn options() -> Value {
    json!({"outputKind":"DynamicallyLinkedLibrary","optimization":"release","deterministic":true,"concurrentBuild":false,"overflowChecks":false,"allowUnsafe":false,"runtimeMetadataVersion":"v4.0.30319"})
}
fn terminal_digest(report: &[u8], artifact: Option<&[u8]>) -> String {
    let mut digest = Sha256::new();
    digest.update(b"Renraku.CompilationResult\0\x01");
    digest.update((report.len() as u64).to_le_bytes());
    digest.update(report);
    digest.update([u8::from(artifact.is_some())]);
    if let Some(artifact) = artifact {
        digest.update((artifact.len() as u64).to_le_bytes());
        digest.update(artifact);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompilerIdentity {
    package: BTreeMap<String, String>,
    host_sha256: String,
    profile: Value,
}
#[derive(Debug)]
pub struct CompilerToolchain {
    host: PathBuf,
    driver: PathBuf,
    identity: CompilerIdentity,
}
impl CompilerToolchain {
    /// The configured driver is trusted host code. Guest source is only compiled.
    pub fn open(host: impl AsRef<Path>, driver: impl AsRef<Path>) -> Result<Self> {
        let host = host.as_ref().canonicalize()?;
        let driver = driver.as_ref().canonicalize()?;
        require(
            driver
                .file_name()
                .is_some_and(|name| name == "Renraku.Compiler.dll"),
            "expected Renraku.Compiler.dll",
        )?;
        let package = package_hashes(&driver)?;
        let host_sha256 = hash(&read(&host, 32 * 1024 * 1024)?);
        let output = run_process(&host, &[driver.as_os_str(), "--identity".as_ref()])?;
        require(output.0 == 0, "compiler identity probe failed")?;
        let profile: Value = serde_json::from_slice(&output.1)
            .map_err(|_| Error("invalid compiler identity report".into()))?;
        let identity = CompilerIdentity {
            package,
            host_sha256,
            profile,
        };
        validate_identity(&identity)?;
        Ok(Self {
            host,
            driver,
            identity,
        })
    }
    pub fn identity(&self) -> &CompilerIdentity {
        &self.identity
    }
    fn verify(&self) -> Result<()> {
        require(
            package_hashes(&self.driver)? == self.identity.package
                && hash(&read(&self.host, 32 * 1024 * 1024)?) == self.identity.host_sha256,
            "compiler package/host changed after capture",
        )?;
        let actual = Self::open(&self.host, &self.driver)?;
        require(
            actual.identity == self.identity,
            "compiler runtime changed after capture",
        )
    }
}
fn validate_identity(identity: &CompilerIdentity) -> Result<()> {
    require(
        identity.package.len() == PACKAGE.len()
            && PACKAGE.iter().all(|name| {
                identity
                    .package
                    .get(*name)
                    .is_some_and(|value| valid_hash(value))
            })
            && valid_hash(&identity.host_sha256),
        "invalid compiler package identity",
    )?;
    require(
        identity.profile["format"] == 1,
        "unsupported compiler identity format",
    )?;
    for (key, file) in [
        ("driver", "Renraku.Compiler.dll"),
        ("common", "Microsoft.CodeAnalysis.dll"),
        ("csharp", "Microsoft.CodeAnalysis.CSharp.dll"),
    ] {
        require(
            identity.profile["compiler"][key]["sha256"].as_str()
                == Some(identity.package[file].as_str())
                && identity.profile["compiler"][key]["name"]
                    .as_str()
                    .is_some_and(|n| !n.is_empty()),
            "compiler profile/package mismatch",
        )?;
    }
    require(
        identity.profile["runtime"]["framework"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
            && identity.profile["runtime"]["architecture"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            && identity.profile["runtime"]["core"]["sha256"]
                .as_str()
                .is_some_and(valid_hash),
        "invalid compiler runtime identity",
    )
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn package_hashes(driver: &Path) -> Result<BTreeMap<String, String>> {
    let directory = driver
        .parent()
        .ok_or_else(|| Error("compiler has no directory".into()))?;
    PACKAGE
        .iter()
        .map(|name| {
            Ok((
                (*name).into(),
                hash(&read(&directory.join(name), 32 * 1024 * 1024)?),
            ))
        })
        .collect()
}
fn read(path: &Path, limit: usize) -> Result<Vec<u8>> {
    require(
        std::fs::metadata(path)?.is_file(),
        "compiler input must be a regular file",
    )?;
    let mut data = Vec::new();
    std::fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut data)?;
    require(data.len() <= limit, "compiler input exceeds limit")?;
    Ok(data)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompilationRequest {
    manifest: Vec<u8>,
    files: BTreeMap<String, Vec<u8>>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestManifest {
    format: u32,
    universe: String,
    name: String,
    compiler: CompilerIdentity,
    language_version: String,
    options: Value,
    sources: Vec<String>,
    views: u32,
}
impl CompilationRequest {
    pub fn capture(
        program: &Program,
        universe: UniverseId,
        toolchain: &CompilerToolchain,
        name: String,
        sources: Vec<(String, Vec<u8>)>,
        views: Vec<ReferenceView>,
    ) -> Result<Self> {
        let mut files = BTreeMap::new();
        let mut names = Vec::new();
        for (name, bytes) in sources {
            names.push(name.clone());
            require(
                files.insert(format!("sources/{name}"), bytes).is_none(),
                "duplicate source filename",
            )?;
        }
        let core = program
            .images
            .values()
            .find(|bytes| {
                Image::parse(bytes).ok().is_some_and(|image| {
                    image.string(image.row(32, 1).unwrap()[7]).unwrap() == "Renraku.CoreLib"
                })
            })
            .ok_or_else(|| Error("runtime core is missing".into()))?;
        files.insert("Renraku.CoreLib.dll".into(), core.clone());
        for (index, view) in views.iter().enumerate() {
            view.validate(program)?;
            files.insert(format!("views/{index}.json"), view.to_json()?);
        }
        let manifest = RequestManifest {
            format: 1,
            universe: universe.to_string(),
            name,
            compiler: toolchain.identity.clone(),
            language_version: "CSharp12".into(),
            options: options(),
            sources: names,
            views: views.len() as u32,
        };
        Self::from_parts(
            serde_json::to_vec(&manifest).map_err(|e| Error(e.to_string()))?,
            files,
        )
    }
    fn from_parts(manifest: Vec<u8>, files: BTreeMap<String, Vec<u8>>) -> Result<Self> {
        let request = Self { manifest, files };
        request.validate()?;
        Ok(request)
    }
    fn parsed(&self) -> Result<RequestManifest> {
        require(
            self.manifest.len() <= 128 * 1024,
            "compiler request manifest exceeds limit",
        )?;
        serde_json::from_slice(&self.manifest)
            .map_err(|_| Error("invalid compiler request manifest".into()))
    }
    pub fn universe(&self) -> Result<UniverseId> {
        let manifest = self.parsed()?;
        let id = uuid::Uuid::parse_str(&manifest.universe)
            .map_err(|_| Error("invalid compilation universe identity".into()))?;
        let id = UniverseId::from_bytes(*id.as_bytes());
        require(
            id.to_string() == manifest.universe,
            "noncanonical compilation universe identity",
        )?;
        Ok(id)
    }
    pub fn id(&self) -> Result<String> {
        Ok(hash(&self.to_bytes()?))
    }
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        encode_capture(&self.manifest, &self.files)
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let (manifest, files) = decode_capture(bytes)?;
        Self::from_parts(manifest, files)
    }
    fn validate(&self) -> Result<()> {
        let manifest = self.parsed()?;
        require(manifest.format == 1, "unsupported compiler request format")?;
        require(
            manifest.language_version == "CSharp12" && manifest.options == options(),
            "unsupported captured compiler options",
        )?;
        self.universe()?;
        validate_identity(&manifest.compiler)?;
        require(
            !manifest.name.is_empty()
                && manifest.name.len() <= 80
                && !manifest.name.eq_ignore_ascii_case("Renraku.CoreLib")
                && manifest.name.split('.').all(|p| {
                    !p.is_empty()
                        && (p.as_bytes()[0].is_ascii_alphabetic() || p.starts_with('_'))
                        && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                }),
            "invalid compiler assembly name",
        )?;
        require(
            (1..=32).contains(&manifest.sources.len())
                && manifest.views <= 16
                && self.files.len() == 1 + manifest.sources.len() + manifest.views as usize,
            "invalid compiler source/reference count",
        )?;
        require(
            self.files.values().map(Vec::len).sum::<usize>() <= MAX_INPUT,
            "compiler inputs exceed 32 MiB",
        )?;
        let core = self
            .files
            .get("Renraku.CoreLib.dll")
            .ok_or_else(|| Error("missing captured compiler core".into()))?;
        require(
            core.len() <= 16 * 1024 * 1024,
            "compiler core exceeds limit",
        )?;
        let image = Image::parse(core)?;
        require(
            image.string(image.row(32, 1)?[7])? == "Renraku.CoreLib",
            "invalid compiler core",
        )?;
        let mut names = BTreeSet::new();
        let mut total = 0;
        for name in &manifest.sources {
            require(
                !name.is_empty()
                    && name.len() <= 255
                    && !name.contains(['/', '\\', '\0'])
                    && !matches!(name.as_str(), "." | "..")
                    && names.insert(name.to_lowercase()),
                "invalid/duplicate compiler source filename",
            )?;
            let bytes = self
                .files
                .get(&format!("sources/{name}"))
                .ok_or_else(|| Error("missing compiler source".into()))?;
            require(
                bytes.len() <= 1024 * 1024 && std::str::from_utf8(bytes).is_ok(),
                "compiler source exceeds limit or is not UTF-8",
            )?;
            total += bytes.len();
        }
        require(total <= 4 * 1024 * 1024, "compiler sources exceed 4 MiB")?;
        let mut modules = BTreeSet::new();
        for index in 0..manifest.views {
            let view = ReferenceView::from_json(
                self.files
                    .get(&format!("views/{index}.json"))
                    .ok_or_else(|| Error("missing compiler reference view".into()))?,
            )?;
            require(
                view.core_sha256 == hash(core),
                "compiler view/core mismatch",
            )?;
            let schema: Value = serde_json::from_str(&view.schema_json)
                .map_err(|_| Error("invalid compiler reference schema".into()))?;
            require(
                schema["moduleName"]
                    .as_str()
                    .is_some_and(|name| modules.insert(name.to_lowercase())),
                "duplicate compiler reference module",
            )?;
        }
        Ok(())
    }
    fn verify_artifact(&self, artifact: &BuildArtifact) -> Result<()> {
        artifact.validate()?;
        let request = self.parsed()?;
        let manifest: Value = serde_json::from_slice(&artifact.manifest)
            .map_err(|_| Error("invalid compiler output manifest".into()))?;
        require(
            artifact.assembly_name()? == request.name
                && manifest["compiler"] == request.compiler.profile["compiler"]
                && manifest["options"] == request.options
                && manifest["languageVersion"] == request.language_version
                && artifact.files.get("Renraku.CoreLib.dll")
                    == self.files.get("Renraku.CoreLib.dll"),
            "compiler output identity/core mismatch",
        )?;
        let sources = manifest["sources"]
            .as_array()
            .ok_or_else(|| Error("missing compiler output sources".into()))?;
        require(
            sources.len() == request.sources.len(),
            "compiler output source count mismatch",
        )?;
        for (source, name) in sources.iter().zip(&request.sources) {
            let file = format!("sources/{name}");
            require(
                source["file"] == file && artifact.files.get(&file) == self.files.get(&file),
                "compiler output source mismatch",
            )?;
        }
        let references = manifest["references"]
            .as_array()
            .ok_or_else(|| Error("missing compiler output references".into()))?;
        require(
            references.len() == request.views as usize,
            "compiler output reference count mismatch",
        )?;
        for (index, reference) in references.iter().enumerate() {
            let file = reference["view"]["file"]
                .as_str()
                .ok_or_else(|| Error("missing compiler output view".into()))?;
            require(
                artifact.files.get(file) == self.files.get(&format!("views/{index}.json")),
                "compiler output view mismatch",
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Compilation {
    pub request: CompilationRequest,
    pub report: Option<Value>,
    pub artifact: Option<BuildArtifact>,
}
impl Compilation {
    pub fn summary(&self) -> Result<Value> {
        Ok(
            json!({"format":1,"universe":self.request.universe()?.to_string(),"inputId":self.request.id()?,
            "state":if self.artifact.is_some(){"compiled"}else if self.report.is_some(){"rejected"}else{"prepared"},
            "artifactId":self.artifact.as_ref().map(BuildArtifact::id),"report":self.report}),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompilationStage {
    Prepared,
    Started,
    Captured,
    Written,
    Committed,
}
pub struct CompilationStore {
    connection: Connection,
    directory: PathBuf,
}
impl CompilationStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        require(
            path != Path::new(":memory:"),
            "compiler ledger requires a durable file",
        )?;
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let application: i64 =
            connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if application == 0 && version == 0 {
            let tables: i64 = connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )?;
            require(tables == 0, "compiler ledger is not empty")?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch("CREATE TABLE compilation(universe BLOB NOT NULL CHECK(length(universe)=16),request_id TEXT NOT NULL,input_id TEXT NOT NULL CHECK(length(input_id)=64),inputs BLOB NOT NULL,result_id TEXT,report BLOB,artifact BLOB,PRIMARY KEY(universe,request_id),CHECK((result_id IS NULL)=(report IS NULL)),CHECK(artifact IS NULL OR report IS NOT NULL));PRAGMA application_id=1380270915;PRAGMA user_version=1;")?;
            transaction.commit()?;
        } else {
            require(
                application == APPLICATION && version == 1,
                "unsupported compiler ledger identity/schema",
            )?;
        }
        let mode: String = connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        require(
            mode.eq_ignore_ascii_case("wal"),
            "compiler ledger requires WAL",
        )?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "cache_size", 32)?;
        let directory = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()?;
        Ok(Self {
            connection,
            directory,
        })
    }
    pub fn prepare(&mut self, id: &str, request: &CompilationRequest) -> Result<Compilation> {
        validate_request_id(id)?;
        request.validate()?;
        let universe = request.universe()?;
        let inputs = request.to_bytes()?;
        let digest = request.id()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(prior) = load(&transaction, universe, id)? {
            require(
                prior.request == *request,
                "compilation request ID is bound to different inputs/compiler",
            )?;
            return Ok(prior);
        }
        let count: i64 =
            transaction.query_row("SELECT count(*) FROM compilation", [], |row| row.get(0))?;
        require(count < 1000, "compiler ledger request limit reached")?;
        transaction.execute(
            "INSERT INTO compilation VALUES(?1,?2,?3,?4,NULL,NULL,NULL)",
            params![universe.as_bytes().as_slice(), id, digest, inputs],
        )?;
        transaction.commit()?;
        Ok(Compilation {
            request: request.clone(),
            report: None,
            artifact: None,
        })
    }
    pub fn get(&self, universe: UniverseId, id: &str) -> Result<Compilation> {
        validate_request_id(id)?;
        load(&self.connection, universe, id)?
            .ok_or_else(|| Error("compilation request is not prepared".into()))
    }
    pub fn drive(
        &mut self,
        universe: UniverseId,
        id: &str,
        toolchain: &CompilerToolchain,
    ) -> Result<Compilation> {
        self.drive_with_observer(universe, id, toolchain, |_| Ok(()))
    }
    /// Prepared is already committed. A writer lock serializes service workers;
    /// SIGKILL releases it. Pending pure compilations may be rerun explicitly.
    pub fn drive_with_observer(
        &mut self,
        universe: UniverseId,
        id: &str,
        toolchain: &CompilerToolchain,
        mut observer: impl FnMut(CompilationStage) -> Result<()>,
    ) -> Result<Compilation> {
        validate_request_id(id)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut compilation = load(&transaction, universe, id)?
            .ok_or_else(|| Error("compilation request is not prepared".into()))?;
        if compilation.report.is_some() {
            return Ok(compilation);
        }
        observer(CompilationStage::Prepared)?;
        require(
            compilation.request.parsed()?.compiler == toolchain.identity,
            "configured compiler does not match prepared identity",
        )?;
        toolchain.verify()?;
        let (report, artifact) = compile(&self.directory, &compilation.request, toolchain, || {
            observer(CompilationStage::Started)
        })?;
        compilation.report = Some(report);
        compilation.artifact = artifact;
        observer(CompilationStage::Captured)?;
        let report = serde_json::to_vec(compilation.report.as_ref().unwrap())
            .map_err(|e| Error(e.to_string()))?;
        let artifact = compilation
            .artifact
            .as_ref()
            .map(BuildArtifact::to_bytes)
            .transpose()?;
        let digest = terminal_digest(&report, artifact.as_deref());
        transaction.execute(
            "UPDATE compilation SET result_id=?1,report=?2,artifact=?3 WHERE universe=?4 AND request_id=?5",
            params![digest,report, artifact, universe.as_bytes().as_slice(), id],
        )?;
        transaction.cache_flush()?;
        observer(CompilationStage::Written)?;
        transaction.commit()?;
        observer(CompilationStage::Committed)?;
        Ok(compilation)
    }
}

fn load(connection: &Connection, universe: UniverseId, id: &str) -> Result<Option<Compilation>> {
    type Row = (
        String,
        Vec<u8>,
        Option<String>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
    );
    let row:Option<Row>=connection.query_row("SELECT input_id,inputs,result_id,report,artifact FROM compilation WHERE universe=?1 AND request_id=?2",params![universe.as_bytes().as_slice(),id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
    let Some((digest, inputs, result_id, report, artifact)) = row else {
        return Ok(None);
    };
    let request = CompilationRequest::from_bytes(&inputs)?;
    require(
        request.id()? == digest && request.universe()? == universe,
        "compiler ledger input digest/scope mismatch",
    )?;
    require(
        match (&report, &result_id) {
            (None, None) => artifact.is_none(),
            (Some(report), Some(digest)) => *digest == terminal_digest(report, artifact.as_deref()),
            _ => false,
        },
        "compiler ledger terminal digest mismatch",
    )?;
    let report: Option<Value> = report
        .map(|bytes| {
            require(bytes.len() <= MAX_REPORT, "compiler report exceeds limit")?;
            serde_json::from_slice(&bytes).map_err(|_| Error("invalid compiler report".into()))
        })
        .transpose()?;
    let artifact = artifact
        .map(|bytes| BuildArtifact::from_bytes(&bytes))
        .transpose()?;
    if let Some(report) = &report {
        require(
            report["identity"] == request.parsed()?.compiler.profile
                && report["success"].as_bool() == Some(artifact.is_some()),
            "compiler ledger terminal identity/status mismatch",
        )?;
    } else {
        require(
            artifact.is_none(),
            "compiler artifact has no terminal report",
        )?;
    }
    if let Some(artifact) = &artifact {
        request.verify_artifact(artifact)?;
    }
    Ok(Some(Compilation {
        request,
        report,
        artifact,
    }))
}

struct Workspace(PathBuf);
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn compile(
    directory: &Path,
    request: &CompilationRequest,
    toolchain: &CompilerToolchain,
    started: impl FnOnce() -> Result<()>,
) -> Result<(Value, Option<BuildArtifact>)> {
    let manifest = request.parsed()?;
    let workspace = Workspace(directory.join(format!(".renraku-compiler-{}", UniverseId::fresh())));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&workspace.0)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir(&workspace.0)?;
    for (name, bytes) in &request.files {
        let path = workspace.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, bytes)?;
    }
    let output = workspace.0.join("output");
    let mut args = vec![
        toolchain.driver.as_os_str().to_owned(),
        "--core".into(),
        workspace.0.join("Renraku.CoreLib.dll").into_os_string(),
        "--output".into(),
        output.clone().into_os_string(),
        "--name".into(),
        manifest.name.clone().into(),
    ];
    for index in 0..manifest.views {
        args.extend([
            "--reference-view".into(),
            workspace
                .0
                .join(format!("views/{index}.json"))
                .into_os_string(),
        ]);
    }
    for name in &manifest.sources {
        args.push(workspace.0.join(format!("sources/{name}")).into_os_string());
    }
    let (code, stdout, _) = run_process_observed(
        &toolchain.host,
        &args.iter().map(|arg| arg.as_os_str()).collect::<Vec<_>>(),
        started,
    )?;
    require(
        matches!(code, 0..=2),
        "compiler infrastructure failure; prepared request retained",
    )?;
    let mut report: Value = serde_json::from_slice(&stdout)
        .map_err(|_| Error("invalid compiler terminal report".into()))?;
    require(
        report["identity"] == manifest.compiler.profile
            && report["success"].as_bool() == Some(code == 0),
        "compiler terminal identity/status mismatch",
    )?;
    require(
        code != 2 || report["failureKind"] == "input",
        "compiler infrastructure failure; prepared request retained",
    )?;
    require(
        package_hashes(&toolchain.driver)? == toolchain.identity.package,
        "compiler package changed during execution",
    )?;
    report
        .as_object_mut()
        .ok_or_else(|| Error("compiler report is not an object".into()))?
        .remove("outputDirectory");
    let artifact = if code == 0 {
        let artifact = BuildArtifact::from_bundle(output)?;
        request.verify_artifact(&artifact)?;
        Some(artifact)
    } else {
        None
    };
    Ok((report, artifact))
}
fn run_process(host: &Path, args: &[&std::ffi::OsStr]) -> Result<(i32, Vec<u8>, Vec<u8>)> {
    run_process_observed(host, args, || Ok(()))
}
struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn run_process_observed(
    host: &Path,
    args: &[&std::ffi::OsStr],
    started: impl FnOnce() -> Result<()>,
) -> Result<(i32, Vec<u8>, Vec<u8>)> {
    let mut child = Child(
        Command::new(host)
            .args(args)
            .env("DOTNET_SKIP_FIRST_TIME_EXPERIENCE", "1")
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
    );
    let read_pipe = |pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.take(MAX_REPORT as u64 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        })
    };
    let stdout = read_pipe(Box::new(child.0.stdout.take().unwrap()));
    let stderr = read_pipe(Box::new(child.0.stderr.take().unwrap()));
    if let Err(error) = started() {
        let _ = child.0.kill();
        let _ = child.0.wait();
        let _ = stdout.join();
        let _ = stderr.join();
        return Err(error);
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.0.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.0.kill();
            child.0.wait()?;
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout
        .join()
        .map_err(|_| Error("compiler output reader failed".into()))??;
    let stderr = stderr
        .join()
        .map_err(|_| Error("compiler diagnostic reader failed".into()))??;
    require(
        stdout.len() <= MAX_REPORT && stderr.len() <= MAX_REPORT,
        "compiler output exceeds limit",
    )?;
    let status =
        status.ok_or_else(|| Error("compiler timed out; prepared request retained".into()))?;
    Ok((status.code().unwrap_or(-1), stdout, stderr))
}
