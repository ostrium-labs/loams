use loams_sqlgate::codec::command::{Command, ErrPacket, OkPacket, classify};
use loams_sqlgate::codec::handshake::Capabilities as C;

#[test]
fn commands_are_classified_by_first_byte_only() {
    assert_eq!(classify(&[]), None);
    for (byte, cmd) in [
        (0x01, Command::Quit),
        (0x0e, Command::Ping),
        (0x11, Command::ChangeUser),
        (0x12, Command::BinlogDump),
        (0x1e, Command::BinlogDumpGtid),
        (0x15, Command::RegisterSlave),
        (0x08, Command::Shutdown),
        (0x0d, Command::Debug),
        (0x03, Command::Other(0x03)),
        (0xff, Command::Other(0xff)),
    ] {
        // The rest of the packet is never read.
        assert_eq!(classify(&[byte, 0xde, 0xad]), Some(cmd));
        assert_eq!(classify(&[byte]), Some(cmd));
    }
}

#[test]
fn refused_commands_get_1235_and_ping_is_not_activity() {
    for cmd in [
        Command::ChangeUser,
        Command::BinlogDump,
        Command::BinlogDumpGtid,
        Command::RegisterSlave,
        Command::Shutdown,
        Command::Debug,
    ] {
        let err = cmd.refusal().expect("refused");
        assert_eq!((err.code, &err.sql_state), (1235, b"42000"), "{cmd:?}");
    }
    for cmd in [Command::Quit, Command::Ping, Command::Other(0x03)] {
        assert!(cmd.refusal().is_none(), "{cmd:?}");
    }
    // Fix round 1, M2: only what TiDB dispatches is relayed.
    for b in loams_sqlgate::codec::command::RELAYED {
        assert!(classify(&[b]).unwrap().refusal().is_none(), "{b:#x}");
    }
    for b in [0x00, 0x05, 0x06, 0x0a, 0x0c, 0x10, 0x14, 0x20, 0xff] {
        let err = classify(&[b]).unwrap().refusal().expect("not relayed");
        assert_eq!((err.code, &err.sql_state), (1047, b"08S01"), "{b:#x}");
    }
    assert!(!Command::Ping.is_activity());
    assert!(Command::Other(0x03).is_activity());
    assert!(Command::Quit.is_activity());
}

#[test]
fn ok_and_err_packets_roundtrip() {
    let caps = C::PROTOCOL_41 | C::SESSION_TRACK;
    let ok = OkPacket {
        affected_rows: 300,
        last_insert_id: 1 << 40,
        status: 0x0002,
        warnings: 1,
        info: b"hi".to_vec(),
    };
    assert_eq!(OkPacket::decode(&ok.encode(caps), caps), Ok(ok.clone()));
    assert_eq!(
        OkPacket::decode(&ok.encode(C::PROTOCOL_41), C::PROTOCOL_41),
        Ok(ok)
    );
    let err = ErrPacket::new(1045, *b"28000", "Access denied for user 'u'");
    let bytes = err.encode();
    assert_eq!(bytes[0], 0xff);
    assert_eq!(ErrPacket::decode(&bytes), Ok(err));
    assert!(ErrPacket::decode(&[0xff, 0x15]).is_err());
    assert!(
        OkPacket::decode(&[0x00, 0xfb], C::PROTOCOL_41).is_err(),
        "NULL is not a length"
    );
}

/// Fix round 1, M3: `KILL` statements are recognised past comments and
/// executable-comment openers, and an undecidable prefix counts as one.
#[test]
fn kill_statements_are_recognised() {
    use loams_sqlgate::codec::command::starts_with_kill;
    for sql in [
        "KILL 5",
        "kill query 5",
        "  KILL TIDB 5",
        "/* x */ KILL 5",
        "/*!50000 KILL 5 */",
        "/*+ hint */kill 5",
        "/*T! KILL 5 */",
        "/*T![clustered_index] KILL 5 */",
        "-- note\nKILL 5",
        "# note\nKill connection 5",
        "\t\nKILL\t5",
        "KILL",
    ] {
        assert!(starts_with_kill(sql.as_bytes(), true), "{sql:?}");
    }
    for sql in [
        "SELECT 1",
        "killer()",
        "SELECT 'KILL 5'",
        "/* KILL */ SELECT 1",
        "/*+ KILL */ SELECT 1",
        "--x",
        "SKILL",
        "kill_me()",
        "",
    ] {
        assert!(!starts_with_kill(sql.as_bytes(), true), "{sql:?}");
    }
    // A prefix that cannot be decided is refused.
    assert!(starts_with_kill(b"/* a long comment", false));
    assert!(starts_with_kill(b"KI", false));
    assert!(!starts_with_kill(b"SE", false));
}

/// R4.3: multi-statements stay offered and `COM_SET_OPTION` stays relayed
/// (drivers use them); KILL is enforced in TiDB, not by the gate.
#[test]
fn multi_statements_stay_offered() {
    use loams_sqlgate::codec::command::RELAYED;
    use loams_sqlgate::codec::handshake::{TIDB_V8_5_8, advertise};
    assert!(advertise(TIDB_V8_5_8).contains(C::MULTI_STATEMENTS));
    assert!(RELAYED.contains(&0x1b), "COM_SET_OPTION");
    assert!(classify(&[0x1b]).unwrap().refusal().is_none());
}
