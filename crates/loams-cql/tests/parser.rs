use loams_cql::parser::{
    Column, DataType, QualifiedName, Replication, SelectColumns, Statement, Value, parse,
};

fn name(keyspace: Option<&str>, table: &str) -> QualifiedName {
    QualifiedName {
        keyspace: keyspace.map(str::to_owned),
        table: table.to_owned(),
    }
}

#[test]
fn parses_keyspace_and_use_with_identifier_case() {
    assert_eq!(
        parse("CREATE KEYSPACE IF NOT EXISTS Foo WITH replication = {'class':'SimpleStrategy','replication_factor':1};").expect("valid CQL statement"),
        Statement::CreateKeyspace {
            name: "foo".into(),
            if_not_exists: true,
            replication: Replication { class: "SimpleStrategy".into(), replication_factor: 1 },
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
fn parses_bounded_replication_options_in_any_order() {
    assert_eq!(
        parse("CREATE KEYSPACE ks WITH replication = {'replication_factor': 2, 'class': 'org.apache.cassandra.locator.SimpleStrategy'}").expect("valid CQL statement"),
        Statement::CreateKeyspace { name:"ks".into(), if_not_exists:false, replication:Replication { class:"org.apache.cassandra.locator.SimpleStrategy".into(), replication_factor:2 } }
    );
    assert!(matches!(
        parse(
            "CREATE KEYSPACE ks WITH replication = {'class':'SimpleStrategy','replication_factor':'1'}"
        ),
        Ok(Statement::CreateKeyspace {
            replication: Replication {
                replication_factor: 1,
                ..
            },
            ..
        })
    ));
    for sql in [
        "CREATE KEYSPACE ks",
        "CREATE KEYSPACE ks WITH replication = {'class':'SimpleStrategy','replication_factor':0}",
        "CREATE KEYSPACE ks WITH replication = {'class':'NetworkTopologyStrategy','replication_factor':1}",
        "CREATE KEYSPACE ks WITH replication = {'class':'SimpleStrategy','replication_factor':1,'replication_factor':2}",
        "CREATE KEYSPACE ks WITH replication = {'class':'SimpleStrategy'}",
        "CREATE KEYSPACE ks WITH replication = {'class':'SimpleStrategy','replication_factor':1,'unexpected':1}",
        "CREATE KEYSPACE ks WITH replication = {'class':'SimpleStrategy','replication_factor':?}",
        "CREATE KEYSPACE ks WITH replication = {'class':'SimpleStrategy','replication_factor':4294967296}",
    ] {
        assert!(parse(sql).is_err(), "unexpectedly accepted {sql}");
    }
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

#[test]
fn parses_inet_scalar_and_rejects_quoted_builtin_type() {
    assert!(matches!(
        parse("CREATE TABLE addresses (id uuid PRIMARY KEY, address inet)"),
        Ok(Statement::CreateTable { columns, .. }) if columns[1].data_type == DataType::Inet
    ));
    assert!(parse("CREATE TABLE t (id \"text\" PRIMARY KEY)").is_err());
}
