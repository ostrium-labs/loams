use loams_cql::parser::{Column, DataType, QualifiedName, SelectColumns, Statement, Value, parse};

fn name(keyspace: Option<&str>, table: &str) -> QualifiedName {
    QualifiedName {
        keyspace: keyspace.map(str::to_owned),
        table: table.to_owned(),
    }
}

#[test]
fn parses_keyspace_and_use_with_identifier_case() {
    assert_eq!(
        parse("CREATE KEYSPACE IF NOT EXISTS Foo;").expect("valid CQL statement"),
        Statement::CreateKeyspace {
            name: "foo".into(),
            if_not_exists: true
        }
    );
    assert_eq!(
        parse("USE \"Foo\";").expect("valid CQL statement"),
        Statement::Use {
            keyspace: "Foo".into()
        }
    );
}

#[test]
fn parses_single_primary_key_table() {
    assert_eq!(parse("CREATE TABLE IF NOT EXISTS Ks.\"MyTable\" (id bigint, \"Body\" text, active boolean, PRIMARY KEY (id));").expect("valid CQL statement"), Statement::CreateTable {
        name: name(Some("ks"), "MyTable"),
        columns: vec![Column { name:"id".into(), data_type:DataType::BigInt },Column { name:"Body".into(), data_type:DataType::Text },Column { name:"active".into(), data_type:DataType::Boolean }],
        primary_key:"id".into(), if_not_exists:true,
    });
    assert!(
        matches!(parse("create table t (id int primary key, value text)"), Ok(Statement::CreateTable { primary_key, .. }) if primary_key == "id")
    );
}

#[test]
fn parses_insert_with_escaped_strings_and_ordered_bind_markers() {
    assert_eq!(
        parse("INSERT INTO ks.t (id, body, enabled, score) VALUES (?, 'it''s ready', true, ?);")
            .expect("valid CQL statement"),
        Statement::Insert {
            table: name(Some("ks"), "t"),
            columns: vec!["id".into(), "body".into(), "enabled".into(), "score".into()],
            values: vec![
                Value::Bind(0),
                Value::String("it's ready".into()),
                Value::Boolean(true),
                Value::Bind(1)
            ],
        }
    );
}

#[test]
fn parses_select_and_drop() {
    assert_eq!(
        parse("SELECT \"Body\", ID FROM ks.t WHERE id = ?;").expect("valid CQL statement"),
        Statement::Select {
            table: name(Some("ks"), "t"),
            columns: SelectColumns::Named(vec!["Body".into(), "id".into()]),
            key_column: "id".into(),
            key_value: Value::Bind(0)
        }
    );
    assert_eq!(
        parse("SELECT * FROM t WHERE id = -42").expect("valid CQL statement"),
        Statement::Select {
            table: name(None, "t"),
            columns: SelectColumns::All,
            key_column: "id".into(),
            key_value: Value::Integer(-42)
        }
    );
    assert_eq!(
        parse("DROP TABLE IF EXISTS ks.t;").expect("valid CQL statement"),
        Statement::DropTable {
            name: name(Some("ks"), "t"),
            if_exists: true
        }
    );
}

#[test]
fn rejects_unsupported_or_malformed_statements() {
    for sql in [
        "SELECT * FROM t WHERE id = 1 AND x = 2",
        "SELECT * FROM t WHERE id > 1",
        "SELECT * FROM t WHERE id = 1 LIMIT 10",
        "INSERT INTO t (id) VALUES (1) IF NOT EXISTS",
        "CREATE TABLE t (id int, x int, PRIMARY KEY (id,x))",
        "CREATE TABLE t (id int, PRIMARY KEY ((id)))",
        "CREATE TABLE t (x list<text>, id int primary key)",
        "CREATE TABLE t (id int)",
        "CREATE TABLE t (id int primary key, id text)",
        "INSERT INTO t (id, v) VALUES (1)",
        "SELECT * FROM t",
        "DELETE FROM t WHERE id = 1",
        "USE \"unterminated",
        "SELECT * FROM t WHERE id = 'unterminated",
        "SELECT * FROM t WHERE id = 1; SELECT * FROM t WHERE id = 2",
    ] {
        assert!(parse(sql).is_err(), "unexpectedly accepted {sql}");
    }
}

#[test]
fn quoted_identifiers_escape_quotes_and_keep_case() {
    assert_eq!(
        parse("USE \"Foo\"\"Bar\"").expect("valid CQL statement"),
        Statement::Use {
            keyspace: "Foo\"Bar".into()
        }
    );
}
