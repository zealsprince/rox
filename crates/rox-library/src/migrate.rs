//! An ordered schema-migration ladder over SQLite's PRAGMA user_version. Each
//! database owns a static slice of [`Migration`] steps; [`run`] applies every
//! step past the stamped version, each in its own transaction.
//!
//! Steps are forward-only and additive: an older binary pointed at a newer
//! file runs no steps and works with the columns it knows. A dropped or
//! renamed column is a version-gated decision, never a routine ALTER.
//!
//! The scanner skips files whose size and mtime match their row, so a step
//! adding a tag-filled column has to set [`Migration::rescan`] or old rows
//! never fill.
//!
//! Step 1 is the old idempotent init, converging every pre-ladder file
//! (user_version 0) to one shape.

use rusqlite::Connection;

/// `up` runs inside the step's transaction and must not open its own.
pub struct Migration {
    pub name: &'static str,
    pub up: fn(&Connection) -> rusqlite::Result<()>,

    /// Set on any step adding a column the scanner fills from tags. The ladder's
    /// rescan hook then resets every stored mtime. Leave it off otherwise: a
    /// needless rescan reopens every file in the library.
    pub rescan: bool,
}

/// For a ladder with no rescan hook. A step that sets
/// [`Migration::rescan`] here panics, whole ladder checked, so a fresh-db
/// test catches it.
pub fn run(conn: &Connection, ladder: &[Migration]) -> rusqlite::Result<()> {
    if let Some(step) = ladder.iter().find(|m| m.rescan) {
        panic!(
            "migration {:?} asks for a rescan, but its ladder runs without a rescan hook",
            step.name
        );
    }

    run_with_rescan(conn, ladder, |_| Ok(()))
}

/// A step's slice position is its version, so only ever append. Each step
/// commits with its user_version stamp.
///
/// The rescan runs once per call, inside the first flagged step's
/// transaction. Deferring it to the end would lose it if a later step
/// failed: the flagged step would be stamped done and never owe it again.
pub fn run_with_rescan(
    conn: &Connection,
    ladder: &[Migration],
    rescan: fn(&Connection) -> rusqlite::Result<()>,
) -> rusqlite::Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let mut rescanned = false;

    for (index, migration) in ladder.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }

        // Unchecked because init holds only a shared &Connection; nothing else
        // touches it during open.
        let tx = conn.unchecked_transaction()?;
        (migration.up)(&tx)?;

        // After `up`, so a fresh database's baseline has created the table.
        if migration.rescan && !rescanned {
            rescan(&tx)?;
            rescanned = true;
        }

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
                rescan: false,
            },
            Migration {
                name: "add-b",
                up: |c| c.execute_batch("ALTER TABLE t ADD COLUMN b INTEGER NOT NULL DEFAULT 0;"),
                rescan: false,
            },
        ];
        run(&conn, ladder).unwrap();
        assert_eq!(version(&conn), 2);
        conn.execute("INSERT INTO t (a, b) VALUES (1, 2)", [])
            .unwrap();
    }

    #[test]
    fn is_idempotent_and_only_runs_the_tail() {
        let conn = Connection::open_in_memory().unwrap();
        let step_one = &[Migration {
            name: "create",
            up: |c| c.execute_batch("CREATE TABLE t (a INTEGER);"),
            rescan: false,
        }];
        run(&conn, step_one).unwrap();
        assert_eq!(version(&conn), 1);

        run(&conn, step_one).unwrap();
        assert_eq!(version(&conn), 1);

        let step_two = &[
            step_one[0].clone_for_test(),
            Migration {
                name: "add-b",
                up: |c| c.execute_batch("ALTER TABLE t ADD COLUMN b INTEGER NOT NULL DEFAULT 0;"),
                rescan: false,
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
                rescan: false,
            },
            Migration {
                name: "broken",
                up: |c| c.execute_batch("ALTER TABLE nope ADD COLUMN b INTEGER;"),
                rescan: false,
            },
        ];
        assert!(run(&conn, ladder).is_err());
        assert_eq!(version(&conn), 1);
        conn.execute("INSERT INTO t (a) VALUES (1)", []).unwrap();
    }

    fn reset_mtimes(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            "UPDATE t SET mtime = 0;
             UPDATE calls SET n = n + 1;",
        )
    }

    fn mtime(conn: &Connection) -> i64 {
        conn.query_row("SELECT mtime FROM t", [], |r| r.get(0))
            .unwrap()
    }

    fn calls(conn: &Connection) -> i64 {
        conn.query_row("SELECT n FROM calls", [], |r| r.get(0))
            .unwrap()
    }

    const SCANNED: Migration = Migration {
        name: "scanned",
        up: |c| {
            c.execute_batch(
                "CREATE TABLE t (a INTEGER, mtime INTEGER NOT NULL);
                 INSERT INTO t (a, mtime) VALUES (1, 500);
                 CREATE TABLE calls (n INTEGER NOT NULL);
                 INSERT INTO calls (n) VALUES (0);",
            )
        },
        rescan: false,
    };

    #[test]
    fn only_a_batch_with_a_rescan_step_runs_the_hook() {
        let conn = Connection::open_in_memory().unwrap();

        let plain = &[
            SCANNED,
            Migration {
                name: "add-rating",
                up: |c| c.execute_batch("ALTER TABLE t ADD COLUMN rating INTEGER;"),
                rescan: false,
            },
        ];
        run_with_rescan(&conn, plain, reset_mtimes).unwrap();
        assert_eq!(mtime(&conn), 500, "no step asked for a rescan");
        assert_eq!(calls(&conn), 0);

        let tagged = &[
            SCANNED,
            plain[1].clone_for_test(),
            Migration {
                name: "add-bpm",
                up: |c| c.execute_batch("ALTER TABLE t ADD COLUMN bpm REAL;"),
                rescan: true,
            },
            Migration {
                name: "add-sort",
                up: |c| c.execute_batch("ALTER TABLE t ADD COLUMN sort TEXT;"),
                rescan: true,
            },
        ];
        run_with_rescan(&conn, tagged, reset_mtimes).unwrap();
        assert_eq!(version(&conn), 4);
        assert_eq!(mtime(&conn), 0, "the row is owed a re-read");
        assert_eq!(calls(&conn), 1, "one rescan covers the whole batch");
    }

    #[test]
    fn a_rescan_commits_with_its_step_when_a_later_step_fails() {
        let conn = Connection::open_in_memory().unwrap();
        let ladder = &[
            SCANNED,
            Migration {
                name: "add-bpm",
                up: |c| c.execute_batch("ALTER TABLE t ADD COLUMN bpm REAL;"),
                rescan: true,
            },
            Migration {
                name: "broken",
                up: |c| c.execute_batch("ALTER TABLE nope ADD COLUMN b INTEGER;"),
                rescan: false,
            },
        ];
        assert!(run_with_rescan(&conn, ladder, reset_mtimes).is_err());

        assert_eq!(version(&conn), 2);
        assert_eq!(mtime(&conn), 0);
    }

    #[test]
    #[should_panic(expected = "asks for a rescan")]
    fn a_rescan_step_without_a_hook_is_refused() {
        let conn = Connection::open_in_memory().unwrap();
        let ladder = &[Migration {
            rescan: true,
            ..SCANNED
        }];
        let _ = run(&conn, ladder);
    }

    impl Migration {
        fn clone_for_test(&self) -> Migration {
            Migration {
                name: self.name,
                up: self.up,
                rescan: self.rescan,
            }
        }
    }
}
