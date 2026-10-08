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
    ] {
        let err = cmd.refusal().expect("refused");
        assert_eq!((err.code, &err.sql_state), (1235, b"42000"), "{cmd:?}");
    }
    for cmd in [Command::Quit, Command::Ping, Command::Other(0x03)] {
        assert!(cmd.refusal().is_none(), "{cmd:?}");
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
