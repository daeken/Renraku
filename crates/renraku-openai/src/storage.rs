use crate::{Error, Identity, Result, fail, require, valid_client};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

// Credentials deliberately have no Debug implementation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Session {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub scopes: BTreeSet<String>,
    pub expires_at: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    pub id: Uuid,
    pub client_id: Option<String>,
    pub identity: Option<Identity>,
    pub session: Option<Session>,
    pub refresh_pending: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Vault {
    version: u32,
    pub host_id: String,
    pub profiles: Vec<Profile>,
    pub active: Option<Uuid>,
}
pub(crate) struct Store {
    directory: PathBuf,
}
pub(crate) struct Guard<'a> {
    store: &'a Store,
    _lock: File,
    pub vault: Vault,
}

fn io(_: std::io::Error) -> Error {
    fail("authentication storage operation failed")
}
#[cfg(unix)]
fn private(metadata: &fs::Metadata, directory: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    require(
        if directory {
            metadata.is_dir()
        } else {
            metadata.is_file() && metadata.nlink() == 1
        },
        "authentication storage must use regular private files and directories",
    )?;
    require(
        metadata.mode() & 0o077 == 0,
        "authentication storage permissions allow other users; use a private directory and owner-only files",
    )
}
#[cfg(not(unix))]
fn private(_: &fs::Metadata, _: bool) -> Result<()> {
    Err(fail(
        "protected authentication storage is currently supported only on Unix hosts",
    ))
}
fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
}
impl Store {
    pub fn open(directory: &Path) -> Result<Self> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(directory).map_err(io)?;
        private(&fs::symlink_metadata(directory).map_err(io)?, true)?;
        let canonical = fs::canonicalize(directory).map_err(io)?;
        require(
            !canonical.ancestors().any(|path| path.join(".git").exists()),
            "authentication storage must be outside Git checkouts",
        )?;
        Ok(Self {
            directory: canonical,
        })
    }
    pub fn lock(&self) -> Result<Guard<'_>> {
        private(&fs::symlink_metadata(&self.directory).map_err(io)?, true)?;
        let lock = options()
            .create(true)
            .truncate(false)
            .open(self.directory.join("auth.lock"))
            .map_err(io)?;
        private(&lock.metadata().map_err(io)?, false)?;
        lock.try_lock()
            .map_err(|_| fail("authentication store is in use by another operation"))?;
        for entry in fs::read_dir(&self.directory).map_err(io)? {
            let entry = entry.map_err(io)?;
            if entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix(".pending-"))
                .is_some_and(|id| Uuid::parse_str(id).is_ok())
            {
                fs::remove_file(entry.path()).map_err(io)?;
            }
        }
        let path = self.directory.join("profiles.json");
        let vault = if path.try_exists().map_err(io)? {
            let file = options().open(path).map_err(io)?;
            private(&file.metadata().map_err(io)?, false)?;
            let mut bytes = Vec::new();
            file.take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(io)?;
            require(
                bytes.len() <= 1024 * 1024,
                "authentication store exceeds its size limit",
            )?;
            let vault: Vault =
                serde_json::from_slice(&bytes).map_err(|_| fail("invalid authentication store"))?;
            vault.validate()?;
            vault
        } else {
            Vault {
                version: 1,
                host_id: Uuid::new_v4().urn().to_string(),
                profiles: Vec::new(),
                active: None,
            }
        };
        Ok(Guard {
            store: self,
            _lock: lock,
            vault,
        })
    }
}
impl Vault {
    fn validate(&self) -> Result<()> {
        require(
            self.version == 1,
            "unsupported authentication store version",
        )?;
        require(
            Uuid::parse_str(&self.host_id).is_ok() && self.profiles.len() <= 64,
            "invalid authentication host or profile count",
        )?;
        let mut ids = BTreeSet::new();
        for profile in &self.profiles {
            require(ids.insert(profile.id), "duplicate authentication profile")?;
            if let Some(client) = &profile.client_id {
                require(valid_client(client), "invalid issued authentication client")?;
            }
            if let Some(identity) = &profile.identity {
                require(
                    profile.client_id.is_some()
                        && !identity.subject.is_empty()
                        && identity.subject.len() <= 1024
                        && identity.issuer == crate::ISSUER,
                    "invalid stored authentication identity",
                )?;
            }
            if let Some(session) = &profile.session {
                require(
                    profile.identity.is_some()
                        && session.id_token.is_some()
                        && session.expires_at > 0,
                    "stored session has no verified account",
                )?;
                require(
                    session.scopes.len() <= 64 && session.scopes.iter().all(|s| s.len() <= 256),
                    "invalid stored token scopes",
                )?;
                for token in [
                    &session.access_token,
                    &session.refresh_token,
                    &session.id_token,
                ]
                .into_iter()
                .flatten()
                {
                    require(
                        !token.is_empty() && token.len() <= 65536,
                        "invalid stored token length",
                    )?;
                }
            }
        }
        require(
            self.active.is_none_or(|id| ids.contains(&id)),
            "active authentication profile does not exist",
        )
    }
}
impl Guard<'_> {
    pub fn save(&self) -> Result<()> {
        self.vault.validate()?;
        let bytes = serde_json::to_vec(&self.vault)
            .map_err(|_| fail("could not encode authentication store"))?;
        require(
            bytes.len() <= 1024 * 1024,
            "authentication store exceeds its size limit",
        )?;
        let temporary = self
            .store
            .directory
            .join(format!(".pending-{}", Uuid::new_v4()));
        let result = (|| {
            let mut file = options().create_new(true).open(&temporary).map_err(io)?;
            file.write_all(&bytes).map_err(io)?;
            file.sync_all().map_err(io)?;
            fs::rename(&temporary, self.store.directory.join("profiles.json")).map_err(io)?;
            File::open(&self.store.directory)
                .map_err(io)?
                .sync_all()
                .map_err(io)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}
