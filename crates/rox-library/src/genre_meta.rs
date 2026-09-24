//! The genre metadata table: aliases ("DnB" counts as "Drum & Bass") laid
//! over the tags without rewriting a file. The display and art columns are
//! unused headroom.
//!
//! Aliases apply at the [`crate::genre`] choke point, so every consumer
//! agrees without knowing the table exists.

use std::collections::HashMap;

use rusqlite::Connection;

/// Keyed by the folded name, whatever the library's case setting.
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

/// Point `from` at `into` everywhere. A merge that would close a cycle is
/// refused, so the table never holds one.
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
    // Keep the table flat: rows pointing at the merged-away value follow it.
    conn.execute(
        "UPDATE genre_meta SET fold_into = ?2 WHERE LOWER(fold_into) = ?1",
        rusqlite::params![key, resolved],
    )?;
    Ok(())
}

pub fn clear_aliases_into(conn: &Connection, target: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE genre_meta SET fold_into = '' WHERE LOWER(fold_into) = LOWER(?1)",
        [target],
    )
}

pub fn aliases_into(conn: &Connection, target: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT name FROM genre_meta WHERE LOWER(fold_into) = LOWER(?1) ORDER BY name",
    )?;
    let rows = stmt.query_map([target], |row| row.get(0))?;
    rows.collect()
}

/// The flattened alias map, folded name -> canonical display, with a depth
/// cap against a cycle an older write may have left.
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
        set_alias(&conn, "D&B", "Drum & Bass").unwrap();
        let map = aliases(&conn).unwrap();
        assert_eq!(map["dnb"], "Drum & Bass");
        assert_eq!(map["d&b"], "Drum & Bass");
        set_alias(&conn, "Drum & Bass", "dnb").unwrap();
        assert!(!aliases(&conn).unwrap().contains_key("drum & bass"));
        assert_eq!(
            aliases_into(&conn, "Drum & Bass").unwrap(),
            ["d&b", "dnb"],
            "both names read as folding into the target"
        );
        assert_eq!(clear_aliases_into(&conn, "drum & bass").unwrap(), 2);
        assert!(aliases(&conn).unwrap().is_empty());
    }
}
