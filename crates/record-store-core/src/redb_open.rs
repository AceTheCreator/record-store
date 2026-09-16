//! Opening a redb database, migrating an older file format on the way.

use std::path::Path;

use redb::{Database, DatabaseError, StorageError, UpgradeError};

/// Opens a redb database at `path`, creating it if absent, and migrates a
/// file-format v2 file to v3.
///
/// Every Record Store release up to and including 0.1.2 wrote v2, because redb
/// 2.6 creates that format unless asked otherwise. redb 3.0 removed the ability
/// to read it, so a deployment that reached a redb 4 build with a v2 file would
/// fail to open its data. Migrating whenever a database is opened means each
/// deployment converts itself on first start, while it is still running a redb
/// version that can read both.
///
/// The migration is a no-op on a file that is already v3, so this costs one
/// format check per open once a deployment has converted, and redb 2.6 keeps
/// reading a converted file — the release that migrates is still one you can go
/// back to.
pub fn open_database(path: impl AsRef<Path>) -> Result<Database, DatabaseError> {
    let mut database = Database::create(path)?;
    database.upgrade().map_err(|error| match error {
        UpgradeError::Storage(error) => DatabaseError::Storage(error),
        // Record Store creates no savepoints and holds the only handle to a
        // database it has just opened, so these cannot arise. Reporting them as
        // corruption keeps the signature usable by every caller rather than
        // widening the error type for cases that do not occur.
        other => DatabaseError::Storage(StorageError::Corrupted(format!(
            "redb file format migration failed: {other}"
        ))),
    })?;
    Ok(database)
}

#[cfg(test)]
mod tests {
    use redb::TableDefinition;
    use tempfile::tempdir;

    use super::*;

    const TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("test.v1");

    // Guards the reason this function exists. A plain `Database::create` would
    // pass every other test in the workspace, because they all start from an
    // empty directory and so never meet a file written by an older release.
    #[test]
    fn opening_reports_the_file_format_the_next_major_requires() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("store.redb");

        let database = open_database(&path).expect("create");
        let write = database.begin_write().expect("begin write");
        {
            let mut table = write.open_table(TABLE).expect("open table");
            table.insert("key", b"value".as_slice()).expect("insert");
        }
        write.commit().expect("commit");
        drop(database);

        // Reopening migrates if needed and must leave the data readable. redb 4
        // refuses a v2 file outright, so a database this function has opened is
        // one that release can still read.
        let database = open_database(&path).expect("reopen");
        let read = database.begin_read().expect("begin read");
        let table = read.open_table(TABLE).expect("open table");
        assert_eq!(
            table
                .get("key")
                .expect("get")
                .map(|value| value.value().to_vec()),
            Some(b"value".to_vec())
        );
    }

    #[test]
    fn opening_a_second_time_is_a_no_op() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("store.redb");
        drop(open_database(&path).expect("create"));
        drop(open_database(&path).expect("reopen"));
        drop(open_database(&path).expect("reopen again"));
    }
}
