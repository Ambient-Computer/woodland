//! Reading a table a few rows at a time.

use super::indexes::{sign_up_at, signed_up};
use super::unique::{account, tier};
use super::*;

/// The keys of the rows in each part, and whether every one of them
/// came with the table's columns and its own cells.
fn rows_in(parts: &[Vec<Found>], table: ModelId) -> Vec<Vec<String>> {
    let rows = |part: &Vec<Found>| {
        let read = Table::read(part).expect("the columns come with every part");
        assert_eq!(read.columns().count(), 3);

        let key = |row: &Row| {
            assert_ne!(row.get(tier(table)), &Value::Null);

            String::from_utf8_lossy(row.key()).into_owned()
        };

        read.rows().iter().map(key).collect()
    };

    parts.iter().map(rows).collect()
}

#[test]
fn rows_are_read_a_few_at_a_time() {
    let (cluster, group, table) = signed_up();
    let everyone = Query::rows::<&str>(group, table, ..);

    let parts = cluster.read_in_parts(NAS, &everyone, 3);
    assert_eq!(
        rows_in(&parts, table),
        [vec!["ada", "al", "alan"], vec!["grace"]]
    );

    let parts = cluster.read_in_parts(NAS, &everyone, 1);
    assert_eq!(
        rows_in(&parts, table).concat(),
        ["ada", "al", "alan", "grace"]
    );
    assert!(
        parts
            .iter()
            .all(|part| Table::read(part).expect("created").rows().len() <= 1)
    );

    let some = Query::rows(group, table, "al".."b");
    let parts = cluster.read_in_parts(NAS, &some, 1);
    assert_eq!(rows_in(&parts, table).concat(), ["al", "alan"]);
}

/// A whole table is read the same way, and what isn't a row doesn't
/// count toward the limit.
#[test]
fn a_whole_table_is_read_a_few_rows_at_a_time() {
    let (cluster, group, table) = signed_up();
    let parts = cluster.read_in_parts(NAS, &Query::table(group, table), 2);

    let whole = cluster.read(NAS, &Query::table(group, table));
    assert_eq!(Table::read(&whole).expect("created").rows().len(), 4);
    assert_eq!(
        rows_in(&parts[..2], table),
        [vec!["ada", "al"], vec!["alan", "grace"]]
    );

    // Whatever else the table keeps is somewhere among the parts.
    let mut found = parts.concat();
    found.sort();
    found.dedup();
    assert_eq!(found, whole);
}

/// Rows nothing has committed are read in parts like any others, on
/// the device that wrote them.
#[test]
fn rows_not_yet_committed_are_read_a_few_at_a_time() {
    let (mut cluster, group, table) = signed_up();
    for (key, paid) in [("bo", 4), ("alice", 5), ("zed", 6)] {
        sign_up_at(&mut cluster, LAPTOP, key, paid);
    }

    let al = account(&cluster, LAPTOP, "al").expect("signed up");
    cluster.write(LAPTOP, Transaction::new().delete_row(table, &al));

    let everyone = Query::rows::<&str>(group, table, ..);
    let parts = cluster.read_in_parts(LAPTOP, &everyone, 2);
    assert_eq!(
        rows_in(&parts, table),
        [
            vec!["ada", "alan"],
            vec!["alice", "bo"],
            vec!["grace", "zed"],
            vec![]
        ]
    );

    let committed = cluster.read_in_parts(LAPTOP, &everyone.committed(), 2);
    assert_eq!(
        rows_in(&committed, table).concat(),
        ["ada", "al", "alan", "grace"]
    );
}

#[test]
fn an_index_is_read_a_few_rows_at_a_time() {
    let (cluster, group, table) = signed_up();
    let by_tier = Query::index(group, table, tier(table), ..);
    let keys = |part: &Vec<Found>| {
        let keys = Table::row_keys(part);

        keys.iter()
            .map(|key| String::from_utf8_lossy(key).into_owned())
            .collect::<Vec<_>>()
    };

    let parts = cluster.read_in_parts(NAS, &by_tier, 3);
    let found: Vec<_> = parts.iter().map(keys).collect();
    assert_eq!(found, [vec!["ada", "grace", "alan"], vec!["al"]]);
}
