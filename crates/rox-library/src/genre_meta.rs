//! The genre metadata table: the library's own opinions about its genre
//! values, laid over the tags without ever rewriting a file. One row per
//! folded genre name with an alias target ("DnB" counts as
//! "Drum & Bass"), a display override, and a custom art path. The alias
//! is the shipped feature; the other two columns are schema headroom for
//! the panel's later knobs.
//!
//! Aliases apply at the [`crate::genre`] choke point, so every consumer
//! of genre values (the projection's matching, the filter panel, the
//! stats rollups, the genre grid) agrees without knowing the table
//! exists. The app loads the flattened map after opening the library and
//! after every edit here, then reloads the projection.

use std::collections::HashMap;

use rusqlite::Connection;

/// The table beside the tracks it describes. Keyed by the folded name so
/// "DnB" and "dnb" share a row no matter the library's case setting.
pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS genre_meta (
            name      TEXT PRIMARY KEY,
            fold_into TEXT NOT NULL DEFAULT '',
            display   TEXT NOT NULL DEFAULT '',
            art       TEXT NOT NULL DEFAULT ''
        );",
    )
}

/// Point one genre at another: rows tagged `from` count under `into`
/// everywhere. `from` folds to the row key; `into` keeps its casing, since
/// it's the display the merged value shows. A merge into itself (or into
/// something that already resolves back to `from`) is refused rather
/// than written, so the table never holds a cycle.
pub fn set_alias(conn: &Connection, from: &str, into: &str) -> rusqlite::Result<()> {
    let key = from.trim().to_lowercase();
    let into = into.trim();
    if key.is_empty() || into.is_empty() {
        return Ok(());
    }
    let map = aliases(conn)?;
    let resolved = map
        .get(&into.to_lowercase())
        .map(String::as_str)
        .unwrap_or(into);
    if resolved.to_lowercase() == key {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO genre_meta (name, fold_into) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET fold_into = excluded.fold_into",
        rusqlite::params![key, resolved],
    )?;
    // Rows that pointed at the value being merged away follow it to the
    // new end, so the table stays flat and the unmerge menu sees every
    // name under its real target.
    conn.execute(
        "UPDATE genre_meta SET fold_into = ?2 WHERE LOWER(fold_into) = ?1",
        rusqlite::params![key, resolved],
    )?;
    Ok(())
}

/// Drop every alias pointing at `target`, the unmerge: the folded-away
/// values come back as their own genres on the next reload.
pub fn clear_aliases_into(conn: &Connection, target: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE genre_meta SET fold_into = '' WHERE LOWER(fold_into) = LOWER(?1)",
        [target],
    )
}

/// The alias names folding into `target`, for the unmerge menu's tally.
pub fn aliases_into(conn: &Connection, target: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT name FROM genre_meta WHERE LOWER(fold_into) = LOWER(?1) ORDER BY name",
    )?;
    let rows = stmt.query_map([target], |row| row.get(0))?;
    rows.collect()
}

/// The flattened alias map, folded name -> canonical display: chains
/// resolve to their end ("DnB" -> "D&B" -> "Drum & Bass" reads straight
/// through), with a depth cap standing guard against a cycle an older
/// write may have left.
pub fn aliases(conn: &Connection) -> rusqlite::Result<HashMap<String, String>> {
    let mut stmt =
        conn.prepare_cached("SELECT name, fold_into FROM genre_meta WHERE fold_into <> ''")?;
    let raw: HashMap<String, String> = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut flat = HashMap::new();
    for (name, mut target) in raw.iter().map(|(n, t)| (n.clone(), t.clone())) {
        for _ in 0..raw.len() {
            match raw.get(&target.to_lowercase()) {
                Some(next) if next.to_lowercase() != name => target = next.clone(),
                _ => break,
            }
        }
        flat.insert(name, target);
    }
    Ok(flat)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn aliases_flatten_chains_and_refuse_cycles() {
        let conn = conn();
        set_alias(&conn, "DnB", "D&B").unwrap();
        // Merging the middle onto a new end rewrites the chain flat.
        set_alias(&conn, "D&B", "Drum & Bass").unwrap();
        let map = aliases(&conn).unwrap();
        assert_eq!(map["dnb"], "Drum & Bass");
        assert_eq!(map["d&b"], "Drum & Bass");
        // A merge that would close the loop writes nothing.
        set_alias(&conn, "Drum & Bass", "dnb").unwrap();
        assert!(!aliases(&conn).unwrap().contains_key("drum & bass"));
        // The unmerge frees everything pointing at the target.
        assert_eq!(
            aliases_into(&conn, "Drum & Bass").unwrap(),
            ["d&b", "dnb"],
            "both names read as folding into the target"
        );
        assert_eq!(clear_aliases_into(&conn, "drum & bass").unwrap(), 2);
        assert!(aliases(&conn).unwrap().is_empty());
    }
}
