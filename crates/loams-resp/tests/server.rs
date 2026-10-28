//! The server, the command table and the connection commands over TCP
//! (plan KV2 Task 2).

use loams_resp::Value;
use loams_resp::testing::{Client, TestServer, config, start};

async fn server(extra: &str) -> TestServer {
    start(config(extra), loams_kv::testing::embedded().await).await
}

fn bulk(s: &str) -> Value {
    Value::bulk(s.to_owned())
}

fn err(s: &str) -> Value {
    Value::err(s.to_owned())
}

#[tokio::test]
async fn ping_echo_and_unknown_command() {
    let s = server("").await;
    let mut c = Client::connect(s.addr).await;
    assert_eq!(c.call(&["PING"]).await, Value::Simple("PONG".into()));
    assert_eq!(c.call(&["ping", "hi"]).await, bulk("hi"));
    assert_eq!(c.call(&["ECHO", "x y"]).await, bulk("x y"));
    assert_eq!(
        c.call(&["NOSUCH", "a", "b"]).await,
        err("ERR unknown command 'NOSUCH', with args beginning with: 'a' 'b' ")
    );
    assert_eq!(
        c.call(&["EVAL", "return 1", "0"]).await,
        err("ERR 'eval' is not supported by Loams")
    );
}

#[tokio::test]
async fn arity_errors_match_valkey() {
    let s = server("").await;
    let mut c = Client::connect(s.addr).await;
    assert_eq!(
        c.call(&["ECHO"]).await,
        err("ERR wrong number of arguments for 'echo' command")
    );
    assert_eq!(
        c.call(&["select", "1", "2"]).await,
        err("ERR wrong number of arguments for 'select' command")
    );
    assert_eq!(
        c.call(&["PING", "a", "b"]).await,
        err("ERR wrong number of arguments for 'ping' command")
    );
}

#[tokio::test]
async fn hello_switches_protocol() {
    let s = server("").await;
    let mut c = Client::connect(s.addr).await;
    let Value::Map(fields) = c.call(&["HELLO", "3"]).await else {
        panic!("HELLO 3 replies with a map");
    };
    assert!(fields.contains(&(bulk("proto"), Value::Integer(3))));
    let Value::Map(_) = c.call(&["HELLO"]).await else {
        panic!("still RESP3");
    };
    let Value::Array(flat) = c.call(&["HELLO", "2"]).await else {
        panic!("HELLO 2 replies with a flat array");
    };
    assert!(
        flat.windows(2)
            .any(|w| w == [bulk("proto"), Value::Integer(2)])
    );
    assert_eq!(
        c.call(&["HELLO", "4"]).await,
        err("NOPROTO unsupported protocol version")
    );
}

#[tokio::test]
async fn auth_required_when_configured() {
    let s = server("requirepass = \"sekrit\"").await;
    let mut c = Client::connect(s.addr).await;
    assert_eq!(
        c.call(&["PING"]).await,
        err("NOAUTH Authentication required.")
    );
    assert_eq!(
        c.call(&["AUTH", "wrong"]).await,
        err("WRONGPASS invalid username-password pair or user is disabled.")
    );
    assert_eq!(c.call(&["AUTH", "default", "sekrit"]).await, Value::ok());
    assert_eq!(c.call(&["PING"]).await, Value::Simple("PONG".into()));
    // RESET logs out.
    assert_eq!(c.call(&["RESET"]).await, Value::Simple("RESET".into()));
    assert_eq!(
        c.call(&["PING"]).await,
        err("NOAUTH Authentication required.")
    );
    // HELLO with AUTH logs in.
    let Value::Map(_) = c.call(&["HELLO", "3", "AUTH", "default", "sekrit"]).await else {
        panic!("HELLO AUTH");
    };
    // Before AUTH, a request may have at most 10 arguments.
    let mut d = Client::connect(s.addr).await;
    let many: Vec<&str> = std::iter::repeat_n("x", 11).collect();
    assert_eq!(
        d.call(&many).await,
        err("ERR Protocol error: invalid multibulk length")
    );
    assert_eq!(d.read().await, None, "closed after a protocol error");
    // With no password configured, AUTH <password> is an error.
    let open = server("").await;
    let mut e = Client::connect(open.addr).await;
    assert!(matches!(
        e.call(&["AUTH", "x"]).await,
        Value::Error(t) if t.contains("without any password")
    ));
}

#[tokio::test]
async fn select_bounds() {
    let s = server("databases = 4").await;
    let mut c = Client::connect(s.addr).await;
    assert_eq!(c.call(&["SELECT", "3"]).await, Value::ok());
    assert_eq!(
        c.call(&["SELECT", "4"]).await,
        err("ERR DB index is out of range")
    );
    assert_eq!(
        c.call(&["SELECT", "-1"]).await,
        err("ERR DB index is out of range")
    );
    assert_eq!(
        c.call(&["SELECT", "x"]).await,
        err("ERR value is not an integer or out of range")
    );
}

#[tokio::test]
async fn pipelined_replies_in_order() {
    let s = server("").await;
    let mut c = Client::connect(s.addr).await;
    let mut raw = Vec::new();
    for i in 0..200 {
        let msg = format!("m{i}");
        raw.extend_from_slice(
            format!("*2\r\n$4\r\nECHO\r\n${}\r\n{msg}\r\n", msg.len()).as_bytes(),
        );
    }
    raw.extend_from_slice(b"PING\r\n");
    c.send_raw(&raw).await;
    for i in 0..200 {
        assert_eq!(c.read().await, Some(bulk(&format!("m{i}"))));
    }
    assert_eq!(c.read().await, Some(Value::Simple("PONG".into())));
}

#[tokio::test]
async fn client_name_and_id() {
    let s = server("").await;
    let mut c = Client::connect(s.addr).await;
    let mut d = Client::connect(s.addr).await;
    assert_eq!(c.call(&["CLIENT", "GETNAME"]).await, Value::Null);
    assert_eq!(
        c.call(&["CLIENT", "SETNAME", "worker-1"]).await,
        Value::ok()
    );
    assert_eq!(c.call(&["CLIENT", "GETNAME"]).await, bulk("worker-1"));
    assert!(matches!(
        c.call(&["CLIENT", "SETNAME", "a b"]).await,
        Value::Error(t) if t.contains("cannot contain spaces")
    ));
    let Value::Integer(id) = d.call(&["CLIENT", "ID"]).await else {
        panic!("an id");
    };
    let Value::Bulk(list) = c.call(&["CLIENT", "LIST"]).await else {
        panic!("a list");
    };
    let list = String::from_utf8_lossy(&list).into_owned();
    assert!(list.contains("name=worker-1"), "{list}");
    assert!(list.contains(&format!("id={id} ")), "{list}");
    assert_eq!(
        c.call(&["CLIENT", "KILL", "ID", &id.to_string()]).await,
        Value::Integer(1)
    );
    assert_eq!(d.read().await, None, "the killed client is closed");
    assert_eq!(c.call(&["QUIT"]).await, Value::ok());
    assert_eq!(c.read().await, None);
}

#[tokio::test]
async fn config_get_set_echoes_unknown() {
    let s = server("").await;
    let mut c = Client::connect(s.addr).await;
    assert_eq!(
        c.call(&[
            "CONFIG",
            "SET",
            "list-max-ziplist-size",
            "4",
            "made-up",
            "x"
        ])
        .await,
        Value::ok()
    );
    assert_eq!(
        c.call(&["CONFIG", "GET", "list-max-ziplist-size"]).await,
        Value::Array(vec![bulk("list-max-ziplist-size"), bulk("4")])
    );
    assert_eq!(
        c.call(&["CONFIG", "GET", "made-up"]).await,
        Value::Array(vec![bulk("made-up"), bulk("x")])
    );
    assert_eq!(
        c.call(&["CONFIG", "GET", "save"]).await,
        Value::Array(vec![bulk("save"), bulk("")])
    );
    assert_eq!(
        c.call(&["DEBUG", "SET-ACTIVE-EXPIRE", "0"]).await,
        Value::ok()
    );
    assert_eq!(c.call(&["FUNCTION", "FLUSH"]).await, Value::ok());
    assert_eq!(c.call(&["FLUSHALL"]).await, Value::ok());
    assert_eq!(c.call(&["FLUSHDB", "ASYNC"]).await, Value::ok());
    let Value::Bulk(info) = c.call(&["INFO", "server"]).await else {
        panic!("INFO is a bulk string in RESP2");
    };
    let info = String::from_utf8_lossy(&info).into_owned();
    assert!(info.starts_with("# Server\r\nredis_version:"), "{info}");
    assert!(!info.contains("# Clients"));
    assert!(matches!(
        c.call(&["COMMAND", "COUNT"]).await,
        Value::Integer(n) if n > 10
    ));
    assert_eq!(
        c.call(&["COMMAND", "INFO", "nosuch"]).await,
        Value::Array(vec![Value::NullArray])
    );
}

/// FLUSHDB deletes only the selected database's keys; FLUSHALL deletes
/// every database's and keeps the keys sorting after them (D1653's GC queue
/// and leases).
#[tokio::test]
async fn flushdb_spares_other_dbs() {
    let store = loams_kv::testing::embedded().await;
    for key in [&b"\x00a"[..], b"\x01b", b"\x02c", b"gq", b"Llease"] {
        store.put(key, b"v".to_vec()).await.expect("a put");
    }
    // No workers: the GC would drop the planted `gq` row.
    let s = start(config("workers = false"), store.clone()).await;
    let mut c = Client::connect(s.addr).await;
    assert_eq!(c.call(&["SELECT", "1"]).await, Value::ok());
    assert_eq!(c.call(&["FLUSHDB"]).await, Value::ok());
    assert_eq!(store.get(b"\x01b").await.expect("a get"), None);
    assert!(store.get(b"\x00a").await.expect("a get").is_some());
    assert!(store.get(b"\x02c").await.expect("a get").is_some());
    assert_eq!(c.call(&["FLUSHALL"]).await, Value::ok());
    assert_eq!(store.get(b"\x00a").await.expect("a get"), None);
    assert_eq!(store.get(b"\x02c").await.expect("a get"), None);
    assert!(store.get(b"gq").await.expect("a get").is_some());
    assert!(store.get(b"Llease").await.expect("a get").is_some());
}

#[tokio::test]
async fn client_reply_off_and_skip() {
    let s = server("").await;
    let mut c = Client::connect(s.addr).await;
    c.send(&[b"CLIENT", b"REPLY", b"SKIP"]).await;
    c.send(&[b"ECHO", b"dropped"]).await;
    assert_eq!(c.call(&["ECHO", "kept"]).await, bulk("kept"));
    c.send(&[b"CLIENT", b"REPLY", b"OFF"]).await;
    c.send(&[b"ECHO", b"silent"]).await;
    assert_eq!(c.call(&["CLIENT", "REPLY", "ON"]).await, Value::ok());
    assert_eq!(c.call(&["PING"]).await, Value::Simple("PONG".into()));
}
