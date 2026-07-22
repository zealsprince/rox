//! An ordered schema-migration ladder over SQLite's PRAGMA user_version, the
//! embedded-database answer to keeping a store in sync across version bumps.
//! Each database (the library store, the thumbnail cache) owns a static slice
//! of [`Migration`] steps; [`run`] applies every step past the version stamped
//! in the file, in order, each in its own transaction, then records the new
//! version. A file already at or past the head of the ladder is left untouched,
//! so this is cheap to call on every open.
//!
//! Timestamped, free-standing SQL files (the Supabase model) buy nothing here:
//! the SQL ships inside the binary either way, and a single-author desktop app
//! never merges two migrations authored at once, so a plain ordered slice keeps
//! the sequence unambiguous without a directory to list.
//!
//! Steps are forward-only and, by policy, additive: a user who downgrades and
//! points an older binary at a newer file must still read it. An older binary
//! that finds a user_version past its own ladder simply runs no steps and works
//! against the columns it knows, since every step only ever adds. Anything that
//! would break that (a dropped or renamed column) is a version-gated decision,
//! not a routine ALTER.
//!
//! The one-time transition: every file written before this module existed sits
//! at user_version 0, but in a shape that depends on which release last touched
//! it. Step 1 (the "baseline") is therefore the old idempotent init, guarded
//! probes and all, so it converges any such file to the current shape and
//! stamps it 1. History before this point is not re-expressed as steps; step 2
//! onward is where clean forward migrations begin.

use rusqlite::Connection;

/// One step in a database's ladder. `name` is for the failure message and any
/// future logging; `up` applies the step and is handed a connection already
/// inside the step's transaction, so it must not open its own.
pub struct Migration {
    pub name: &'static str,
    pub up: fn(&Connection) -> rusqlite::Result<()>,
}

/// Bring `conn` up to the head of `ladder`. A step's position in the slice is
/// its version (1-based), so appending to the slice is the only way to add one
/// and reordering is never safe. Each pending step runs in its own transaction
/// that also stamps the new user_version, so a step that fails rolls back whole
/// and leaves the recorded version on the last one that landed.
pub fn run(conn: &Connection, ladder: &[Migration]) -> rusqlite::Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (index, migration) in ladder.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }
        // unchecked_transaction because init runs against a shared &Connection
        // (the caller holds no &mut); nothing else touches this connection
        // during open, so the borrow checker's guarantee is not needed here.
        let tx = conn.unchecked_transaction()?;
        (migration.up)(&tx)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(conn: &Connection) -> i64 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn runs_pending_steps_in_order_and_stamps_the_version() {
        let conn = Connection::open_in_memory().unwrap();
        let ladder = &[
            Migration {
                name: "create",
                up: |c| c.execute_batch("CREATE TABLE t (a INTEGER);"),
            },
            Migration {
                name: "add-b",
                up: |c| c.execute_batch("ALTER TABLE t ADD COLUMN b INTEGER NOT NULL DEFAULT 0;"),
            },
        ];
        run(&conn, ladder).unwrap();
        assert_eq!(version(&conn), 2);
        // Both columns exist, so the ladder ran end to end.
        conn.execute("INSERT INTO t (a, b) VALUES (1, 2)", []).unwrap();
    }

    #[test]
    fn is_idempotent_and_only_runs_the_tail() {
        let conn = Connection::open_in_memory().unwrap();
        let step_one = &[Migration {
            name: "create",
            up: |c| c.execute_batch("CREATE TABLE t (a INTEGER);"),
        }];
        run(&conn, step_one).unwrap();
        assert_eq!(version(&conn), 1);

        // A second call with the same ladder is a no-op: rerunning the CREATE
        // without IF NOT EXISTS would error, so reaching version 1 unchanged
        // proves the step was skipped.
        run(&conn, step_one).unwrap();
        assert_eq!(version(&conn), 1);

        // Appending a step and running again applies only the new tail against
        // the already-baselined file.
        let step_two = &[
            step_one[0].clone_for_test(),
            Migration {
                name: "add-b",
                up: |c| c.execute_batch("ALTER TABLE t ADD COLUMN b INTEGER NOT NULL DEFAULT 0;"),
            },
        ];
        run(&conn, step_two).unwrap();
        assert_eq!(version(&conn), 2);
    }

    #[test]
    fn a_failed_step_rolls_back_and_holds_the_prior_version() {
        let conn = Connection::open_in_memory().unwrap();
        let ladder = &[
            Migration {
                name: "create",
                up: |c| c.execute_batch("CREATE TABLE t (a INTEGER);"),
            },
            Migration {
                name: "broken",
                up: |c| c.execute_batch("ALTER TABLE nope ADD COLUMN b INTEGER;"),
            },
        ];
        assert!(run(&conn, ladder).is_err());
        // Step 1 committed and stamped; the failed step 2 left no trace and did
        // not advance the version.
        assert_eq!(version(&conn), 1);
        conn.execute("INSERT INTO t (a) VALUES (1)", []).unwrap();
    }

    impl Migration {
        // The tests build a longer ladder that reuses an earlier step; a plain
        // Copy/Clone on a public type carrying a fn pointer is more surface than
        // this needs, so the helper stays test-only.
        fn clone_for_test(&self) -> Migration {
            Migration {
                name: self.name,
                up: self.up,
            }
        }
    }
}
