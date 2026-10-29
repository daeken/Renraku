//! Captured compiler bundles and atomic installation receipts.
use crate::{
    checkpoint::{CheckpointStore, CommitStage},
    error::{Error, Result, require},
    identity::AssemblyId,
    image::Image,
    invocation::validate_request_id,
    runtime::Vm,
    workbench::ReferenceView,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Component, Path},
};

pub const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_INSTALLATIONS: usize = 128;
const MAX_MANIFEST: usize = 128 * 1024;

pub(crate) fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildArtifact {
    pub(crate) manifest: Vec<u8>,
    pub(crate) files: BTreeMap<String, Vec<u8>>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallationRecord {
    pub assembly: AssemblyId,
    pub artifact: BuildArtifact,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRecord {
    file: String,
    bytes: usize,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilerIdentity {
    name: String,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReferenceRecord {
    schema_hash: String,
    image: FileRecord,
    view: FileRecord,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    format: u32,
    compiler: BTreeMap<String, CompilerIdentity>,
    language_version: String,
    options: Value,
    core: FileRecord,
    assembly: FileRecord,
    sources: Vec<FileRecord>,
    #[serde(default)]
    references: Vec<ReferenceRecord>,
}
fn filename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.contains(['/', '\\', '\0'])
        && !matches!(value, "." | "..")
}
impl Manifest {
    fn parse(bytes: &[u8]) -> Result<Self> {
        require(
            bytes.len() <= MAX_MANIFEST,
            "build manifest exceeds its size limit",
        )?;
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|_| Error("invalid/unsupported build manifest".into()))?;
        require(
            (manifest.format == 1 && manifest.references.is_empty())
                || (manifest.format == 2 && !manifest.references.is_empty()),
            "unsupported build manifest format",
        )?;
        require(
            manifest.language_version == "CSharp12"
                && manifest.options
                    == json!({"outputKind":"DynamicallyLinkedLibrary","optimization":"release","deterministic":true,"concurrentBuild":false,"overflowChecks":false,"allowUnsafe":false,"runtimeMetadataVersion":"v4.0.30319"}),
            "unsupported compiler options",
        )?;
        require(
            manifest
                .compiler
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                == ["common", "csharp", "driver"]
                && manifest
                    .compiler
                    .values()
                    .all(|c| !c.name.is_empty() && c.name.len() <= 1024 && valid_hash(&c.sha256)),
            "invalid compiler identities",
        )?;
        require(
            manifest.core.file == "Renraku.CoreLib.dll"
                && filename(&manifest.assembly.file)
                && manifest.assembly.file.ends_with(".dll")
                && manifest.assembly.file != manifest.core.file,
            "invalid core/implementation filenames",
        )?;
        require(
            (1..=32).contains(&manifest.sources.len()) && manifest.references.len() <= 16,
            "invalid source/reference count",
        )?;
        for source in &manifest.sources {
            require(
                source.file.strip_prefix("sources/").is_some_and(filename)
                    && source.bytes <= 1024 * 1024,
                "invalid source filename/size",
            )?;
        }
        require(
            manifest.sources.iter().map(|s| s.bytes).sum::<usize>() <= 4 * 1024 * 1024,
            "source inputs exceed 4 MiB",
        )?;
        for reference in &manifest.references {
            require(
                reference
                    .image
                    .file
                    .strip_prefix("references/")
                    .is_some_and(filename)
                    && reference
                        .view
                        .file
                        .strip_prefix("references/")
                        .is_some_and(filename)
                    && reference.view.bytes <= 4 * 1024 * 1024
                    && valid_hash(&reference.schema_hash),
                "invalid reference record",
            )?;
        }
        let mut names = BTreeSet::new();
        let mut total = 0usize;
        for record in manifest.records() {
            require(
                record.bytes <= 16 * 1024 * 1024
                    && valid_hash(&record.sha256)
                    && names.insert(record.file.to_lowercase()),
                "duplicate/invalid build file record",
            )?;
            total = total
                .checked_add(record.bytes)
                .ok_or_else(|| Error("artifact size overflow".into()))?;
            require(
                total <= MAX_ARTIFACT_BYTES,
                "artifact exceeds its aggregate size limit",
            )?;
        }
        Ok(manifest)
    }
    fn records(&self) -> impl Iterator<Item = &FileRecord> {
        [&self.core, &self.assembly]
            .into_iter()
            .chain(&self.sources)
            .chain(self.references.iter().flat_map(|r| [&r.image, &r.view]))
    }
}

impl BuildArtifact {
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        encode_capture(&self.manifest, &self.files)
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let (manifest, files) = decode_capture(bytes)?;
        Self::from_files(manifest, files)
    }
    pub fn from_files(manifest: Vec<u8>, files: BTreeMap<String, Vec<u8>>) -> Result<Self> {
        let artifact = Self { manifest, files };
        artifact.validate()?;
        Ok(artifact)
    }
    /// Capture declared files, never follow a manifest path outside the bundle.
    /// This is a trusted host build boundary, not a hostile filesystem sandbox.
    pub fn from_bundle(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref();
        require(
            std::fs::symlink_metadata(directory)?.is_dir(),
            "bundle must be a regular directory",
        )?;
        let manifest = read_file(directory, "renraku-build.json", MAX_MANIFEST)?;
        let parsed = Manifest::parse(&manifest)?;
        let files = parsed
            .records()
            .map(|record| {
                Ok((
                    record.file.clone(),
                    read_file(directory, &record.file, record.bytes)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        Self::from_files(manifest, files)
    }
    pub fn id(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"Renraku.BuildArtifact\0\x01");
        let mut add = |bytes: &[u8]| {
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes);
        };
        add(&self.manifest);
        for (name, bytes) in &self.files {
            add(name.as_bytes());
            add(bytes);
        }
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    pub fn validate(&self) -> Result<()> {
        let manifest = Manifest::parse(&self.manifest)?;
        require(
            self.files.len() == manifest.records().count(),
            "missing/extra captured build files",
        )?;
        for record in manifest.records() {
            let data = self
                .files
                .get(&record.file)
                .ok_or_else(|| Error("missing captured build file".into()))?;
            require(
                data.len() == record.bytes && hash(data) == record.sha256,
                "captured build file length/hash mismatch",
            )?;
        }
        for source in &manifest.sources {
            require(
                std::str::from_utf8(&self.files[&source.file]).is_ok(),
                "source is not UTF-8",
            )?;
        }
        let core = Image::parse(&self.files[&manifest.core.file])?;
        require(
            core.string(core.row(32, 1)?[7])? == "Renraku.CoreLib",
            "invalid captured core identity",
        )?;
        let image = Image::parse(&self.files[&manifest.assembly.file])?;
        let name = image.string(image.row(32, 1)?[7])?;
        require(
            manifest.assembly.file == format!("{name}.dll") && !image.is_reference_assembly()?,
            "captured implementation is a facade or has a different module identity",
        )?;
        let mut scopes = BTreeSet::from(["Renraku.CoreLib".to_owned()]);
        for reference in &manifest.references {
            let view = ReferenceView::from_json(&self.files[&reference.view.file])?;
            require(
                view.schema_hash == reference.schema_hash
                    && view.core_sha256 == manifest.core.sha256,
                "captured reference schema/core hash mismatch",
            )?;
            let schema: Value = serde_json::from_str(&view.schema_json)
                .map_err(|_| Error("invalid reference schema".into()))?;
            let name = schema["moduleName"]
                .as_str()
                .ok_or_else(|| Error("missing reference module name".into()))?;
            let facade = Image::parse(&self.files[&reference.image.file])?;
            require(
                reference.image.file == format!("references/{name}.dll")
                    && reference.view.file == format!("references/{name}.view.json")
                    && facade.string(facade.row(32, 1)?[7])? == name
                    && facade.is_reference_assembly()?
                    && scopes.insert(name.into()),
                "invalid/duplicate captured reference module",
            )?;
        }
        for row in image.rows(35) {
            require(
                scopes.contains(image.string(row[6])?),
                "implementation uses an uncaptured compiler reference",
            )?;
        }
        Ok(())
    }
    pub fn assembly_name(&self) -> Result<String> {
        let manifest = Manifest::parse(&self.manifest)?;
        Ok(manifest.assembly.file.strip_suffix(".dll").unwrap().into())
    }
    fn implementation(&self) -> Result<&[u8]> {
        let manifest = Manifest::parse(&self.manifest)?;
        self.files
            .get(&manifest.assembly.file)
            .map(Vec::as_slice)
            .ok_or_else(|| Error("missing implementation".into()))
    }
    fn views(&self) -> Result<Vec<ReferenceView>> {
        let manifest = Manifest::parse(&self.manifest)?;
        manifest
            .references
            .iter()
            .map(|r| ReferenceView::from_json(&self.files[&r.view.file]))
            .collect()
    }
    fn core(&self) -> Result<&[u8]> {
        self.files
            .get("Renraku.CoreLib.dll")
            .map(Vec::as_slice)
            .ok_or_else(|| Error("missing core".into()))
    }
}

pub(crate) fn encode_capture(
    manifest: &[u8],
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<u8>> {
    require(
        manifest.len() <= MAX_MANIFEST
            && files.len() <= 66
            && files.values().map(Vec::len).sum::<usize>() <= MAX_ARTIFACT_BYTES,
        "captured input exceeds its limits",
    )?;
    let mut bytes = b"RENFILES\x01".to_vec();
    let add = |out: &mut Vec<u8>, data: &[u8]| {
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
    };
    add(&mut bytes, manifest);
    bytes.extend((files.len() as u32).to_le_bytes());
    for (name, data) in files {
        require(name.len() <= 512, "captured filename exceeds limit")?;
        add(&mut bytes, name.as_bytes());
        add(&mut bytes, data);
    }
    Ok(bytes)
}
pub(crate) type CapturedFiles = (Vec<u8>, BTreeMap<String, Vec<u8>>);
pub(crate) fn decode_capture(bytes: &[u8]) -> Result<CapturedFiles> {
    use crate::cil::Reader;
    require(
        bytes.len() <= MAX_ARTIFACT_BYTES + MAX_MANIFEST + 66 * 520 + 32,
        "captured input exceeds its size limit",
    )?;
    fn blob<'a>(reader: &mut Reader<'a>, limit: usize) -> Result<&'a [u8]> {
        let length = reader.u32()? as usize;
        require(length <= limit, "captured field exceeds limit")?;
        reader.take(length)
    }
    let mut reader = Reader::new(bytes);
    require(
        reader.take(9)? == b"RENFILES\x01",
        "unknown captured input format",
    )?;
    let manifest = blob(&mut reader, MAX_MANIFEST)?.to_vec();
    let count = reader.u32()?;
    require(count <= 66, "too many captured files")?;
    let mut files = BTreeMap::new();
    let mut total = 0;
    for _ in 0..count {
        let name = std::str::from_utf8(blob(&mut reader, 512)?)
            .map_err(|_| Error("invalid captured filename UTF-8".into()))?
            .to_owned();
        let data = blob(&mut reader, MAX_ARTIFACT_BYTES)?;
        total += data.len();
        require(
            total <= MAX_ARTIFACT_BYTES,
            "captured input exceeds aggregate limit",
        )?;
        require(
            files.insert(name, data.to_vec()).is_none(),
            "duplicate captured file",
        )?;
    }
    require(
        reader.position == bytes.len(),
        "trailing captured input data",
    )?;
    Ok((manifest, files))
}

fn read_file(root: &Path, relative: &str, limit: usize) -> Result<Vec<u8>> {
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(part) = component else {
            return Err(Error("invalid bundle path".into()));
        };
        path.push(part);
        require(
            !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "bundle symlinks are unsupported",
        )?;
    }
    require(
        std::fs::symlink_metadata(&path)?.is_file(),
        "build input is not a regular file",
    )?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    require(bytes.len() <= limit, "build input exceeds its byte limit")?;
    Ok(bytes)
}

/// Load the current checkpoint, validate a frozen artifact, and publish code,
/// provenance, and its receipt atomically. Lost replies can query installation ID.
pub fn install_once(
    store: &mut CheckpointStore,
    request: &str,
    artifact: &BuildArtifact,
) -> Result<InstallationRecord> {
    install_once_with_observer(store, request, artifact, |_| Ok(()))
}
/// Observer semantics are identical to CheckpointStore's commit observer.
pub fn install_once_with_observer(
    store: &mut CheckpointStore,
    request: &str,
    artifact: &BuildArtifact,
    observer: impl FnMut(CommitStage) -> Result<()>,
) -> Result<InstallationRecord> {
    validate_request_id(request)?;
    artifact.validate()?;
    let saved = store.load()?;
    let vm = saved.snapshot.restore()?;
    require(
        vm.invocation(request).is_none(),
        "request ID is already bound to an invocation",
    )?;
    if let Some(record) = vm.installations.get(request) {
        require(
            record.artifact.id() == artifact.id(),
            "installation request ID is bound to a different artifact",
        )?;
        return Ok(record.clone());
    }
    require(
        vm.installations.len() < MAX_INSTALLATIONS,
        "installation receipt limit reached",
    )?;
    let core = artifact.core()?;
    let runtime_core = vm
        .program
        .images
        .values()
        .find(|bytes| bytes.as_slice() == core);
    require(
        runtime_core.is_some(),
        "compiler core does not match this universe",
    )?;
    let linked = vm
        .program
        .link_modules(&[artifact.implementation()?], &artifact.views()?)?;
    let assembly = *linked
        .images
        .keys()
        .find(|id| !vm.program.images.contains_key(id))
        .ok_or_else(|| Error("installation has no new module".into()))?;
    let mut vm = vm.rebind_program(&linked)?;
    let record = InstallationRecord {
        assembly,
        artifact: artifact.clone(),
    };
    vm.installations.insert(request.into(), record.clone());
    store.save_with_observer(&vm, observer)?;
    Ok(record)
}

impl Vm<'_> {
    pub fn installation(&self, request: &str) -> Option<&InstallationRecord> {
        self.installations.get(request)
    }
    pub fn installations(&self) -> impl Iterator<Item = (&str, &InstallationRecord)> {
        self.installations.iter().map(|(k, v)| (k.as_str(), v))
    }
    pub(crate) fn validate_installations(&self) -> Result<()> {
        require(
            self.installations.len() <= MAX_INSTALLATIONS,
            "installation receipt limit reached",
        )?;
        let mut modules = BTreeSet::new();
        for (id, record) in &self.installations {
            validate_request_id(id)?;
            require(
                !self.invocations.contains_key(id) && modules.insert(record.assembly),
                "duplicate installation request/module",
            )?;
            record.artifact.validate()?;
            require(
                self.program.images.get(&record.assembly).map(Vec::as_slice)
                    == Some(record.artifact.implementation()?),
                "installation artifact does not match its code",
            )?;
            let core = record.artifact.core()?;
            require(
                self.program
                    .images
                    .values()
                    .any(|bytes| bytes.as_slice() == core),
                "installation core is absent",
            )?;
            for view in record.artifact.views()? {
                view.validate(self.program)?;
            }
        }
        Ok(())
    }
    /// Include exact captured source for the immutable implementation's module;
    /// source locations within a method need separate PDB/source-map support.
    pub fn read_code(&self, name_or_id: &str) -> Result<Value> {
        let mut code = self.program.read_code(name_or_id)?;
        let method = self
            .program
            .methods
            .values()
            .find(|m| Some(m.binding.code.to_string().as_str()) == code["codeVersion"].as_str())
            .unwrap();
        if let Some(record) = self
            .installations
            .values()
            .find(|r| r.assembly == method.assembly)
        {
            let manifest = Manifest::parse(&record.artifact.manifest)?;
            let sources = manifest
                .sources
                .iter()
                .map(|file| {
                    json!({"file":file.file,"sha256":file.sha256,
                "text":std::str::from_utf8(&record.artifact.files[&file.file]).unwrap()})
                })
                .collect::<Vec<_>>();
            code["sourceAvailable"] = json!(true);
            code["sourceScope"] = json!("assembly");
            code["sources"] = json!(sources);
            code["artifactId"] = json!(record.artifact.id());
            code["buildManifest"] = serde_json::from_slice(&record.artifact.manifest)
                .map_err(|_| Error("invalid saved build manifest".into()))?;
        }
        Ok(code)
    }
}
