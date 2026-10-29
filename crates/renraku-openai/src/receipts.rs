//! A local completion receipt is the only reconciliation evidence for store:false.
use crate::{Result, fail, require, storage::Store};
use renraku_vm::{heap::MAX_STRING_UNITS, identity::EffectId};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{fs::OpenOptions, path::Path, time::Duration};

const APPLICATION: u32 = 0x52454f41;
pub(crate) struct Receipts(Connection);
impl Receipts {
    pub fn open(directory: &Path) -> Result<Self> {
        // Reuse private-directory/Git-exclusion checks; no auth data is put here.
        let _ = Store::open(directory)?;
        let path = directory.join("receipts.db");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(&path)
            .map_err(|_| fail("cannot open private receipt ledger"))?;
        let metadata = file
            .metadata()
            .map_err(|_| fail("cannot inspect receipt ledger"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            require(
                metadata.is_file() && metadata.nlink() == 1 && metadata.mode() & 0o077 == 0,
                "receipt ledger must be a private regular file",
            )?;
        }
        let mut db = Connection::open(path).map_err(db_error)?;
        db.busy_timeout(Duration::from_secs(2)).map_err(db_error)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let application: u32 = tx
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(db_error)?;
        let version: u32 = tx
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(db_error)?;
        if application == 0 && version == 0 {
            let tables: u32 = tx
                .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
                .map_err(db_error)?;
            require(
                tables == 0,
                "refusing to initialize a foreign receipt database",
            )?;
            tx.execute_batch(
                "CREATE TABLE receipts (
                effect BLOB PRIMARY KEY CHECK(length(effect)=16),
                binding BLOB NOT NULL, request BLOB NOT NULL,
                response_id TEXT, response BLOB,
                CHECK((response_id IS NULL) = (response IS NULL))
            ) STRICT;",
            )
            .map_err(db_error)?;
            tx.pragma_update(None, "application_id", APPLICATION)
                .map_err(db_error)?;
            tx.pragma_update(None, "user_version", 1)
                .map_err(db_error)?;
        } else {
            require(
                application == APPLICATION && version == 1,
                "unsupported or foreign receipt ledger",
            )?;
        }
        tx.commit().map_err(db_error)?;
        db.pragma_update(None, "journal_mode", "WAL")
            .map_err(db_error)?;
        db.pragma_update(None, "synchronous", "FULL")
            .map_err(db_error)?;
        Ok(Self(db))
    }
    /// Reserve once before sending. A prior attempt can never be sent again.
    pub fn reserve(
        &mut self,
        id: EffectId,
        binding: &[u8],
        request: &[u8],
    ) -> Result<Option<Vec<u16>>> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let existing = lookup(&tx, id, binding, request)?;
        match existing {
            Some(Some(response)) => Ok(Some(response)),
            Some(None) => Err(fail(
                "request was already attempted; outcome is unknown; automatic resubmission is forbidden",
            )),
            None => {
                tx.execute(
                    "INSERT INTO receipts(effect,binding,request) VALUES (?1,?2,?3)",
                    params![id.as_bytes().as_slice(), binding, request],
                )
                .map_err(db_error)?;
                tx.commit().map_err(db_error)?;
                Ok(None)
            }
        }
    }
    pub fn reconcile(
        &self,
        id: EffectId,
        binding: &[u8],
        request: &[u8],
    ) -> Result<Option<Vec<u16>>> {
        Ok(lookup(&self.0, id, binding, request)?.flatten())
    }
    pub fn complete(&mut self, id: EffectId, response_id: &str, units: &[u16]) -> Result<()> {
        require(
            units.len() <= MAX_STRING_UNITS,
            "response exceeds the managed string limit",
        )?;
        let bytes: Vec<_> = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
        let count = self.0.execute("UPDATE receipts SET response_id=?2,response=?3 WHERE effect=?1 AND response IS NULL",
            params![id.as_bytes().as_slice(), response_id, bytes]).map_err(db_error)?;
        require(
            count == 1,
            "completion receipt has no matching pending attempt",
        )
    }
}
fn lookup(
    db: &Connection,
    id: EffectId,
    binding: &[u8],
    request: &[u8],
) -> Result<Option<Option<Vec<u16>>>> {
    let row = db.query_row("SELECT binding=?2 AND request=?3, length(response),
        CASE WHEN length(response_id)<=256 THEN response_id ELSE NULL END,
        CASE WHEN length(response)<=2000000 THEN response ELSE NULL END FROM receipts WHERE effect=?1",
        params![id.as_bytes().as_slice(), binding, request], |row| {
            Ok((row.get::<_, bool>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<Vec<u8>>>(3)?))
        }).optional().map_err(db_error)?;
    let Some((matches, length, response_id, response)) = row else {
        return Ok(None);
    };
    require(
        matches,
        "receipt identity, profile, or request body does not match",
    )?;
    if length.is_none() {
        require(response_id.is_none(), "invalid pending receipt")?;
        return Ok(Some(None));
    }
    let response = response.ok_or_else(|| fail("receipt exceeds its response limit"))?;
    require(
        response.len() % 2 == 0
            && response.len() <= MAX_STRING_UNITS * 2
            && response_id.is_some_and(|id| !id.is_empty() && id.len() <= 256),
        "invalid completion receipt",
    )?;
    let units = response
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    Ok(Some(Some(units)))
}
fn db_error(_: rusqlite::Error) -> crate::Error {
    fail("completion receipt storage operation failed")
}
