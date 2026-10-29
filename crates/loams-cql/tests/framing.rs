use loams_cql::frame::{EnvelopeCodec, FrameError, ProtocolCodec};

const OPTIONS_V4: &[u8] = &[4, 0, 0, 7, 5, 0, 0, 0, 0];

#[test]
fn fragmented_request_is_not_consumed_until_complete() {
    let codec = EnvelopeCodec::new(1024);
    for prefix in 0..OPTIONS_V4.len() {
        assert!(codec.decode(&OPTIONS_V4[..prefix]).unwrap().is_none());
    }
    let (frame, consumed) = codec.decode(OPTIONS_V4).unwrap().unwrap();
    assert_eq!(consumed, OPTIONS_V4.len());
    assert_eq!(frame.stream_id, 7);
    assert_eq!(frame.opcode, cassandra_protocol::frame::Opcode::Options);
}

#[test]
fn rejects_negative_and_over_limit_lengths_before_allocation() {
    let codec = EnvelopeCodec::new(1024);
    let mut bytes = OPTIONS_V4.to_vec();
    bytes[5..9].copy_from_slice(&(-1_i32).to_be_bytes());
    assert!(matches!(
        codec.decode(&bytes),
        Err(FrameError::InvalidLength)
    ));
    bytes[5..9].copy_from_slice(&(1025_i32).to_be_bytes());
    assert!(matches!(codec.decode(&bytes), Err(FrameError::TooLarge)));
}

#[test]
fn rejects_response_direction_and_unsafe_flags() {
    let codec = EnvelopeCodec::new(1024);
    let mut bytes = OPTIONS_V4.to_vec();
    bytes[0] = 0x84;
    assert!(matches!(codec.decode(&bytes), Err(FrameError::Direction)));
    bytes[0] = 4;
    bytes[1] = 0x08; // warnings flag is response-only
    assert!(matches!(
        codec.decode(&bytes),
        Err(FrameError::UnsupportedFlags)
    ));
}

#[test]
fn encode_response_preserves_version_and_stream() {
    use cassandra_protocol::frame::{Direction, Envelope, Flags, Opcode, Version};
    let codec = EnvelopeCodec::new(1024);
    let response = Envelope::new(
        Version::V4,
        Direction::Response,
        Flags::empty(),
        Opcode::Ready,
        7,
        vec![],
        None,
        vec![],
    );
    assert_eq!(
        codec.encode(&response).unwrap(),
        [0x84, 0, 0, 7, 2, 0, 0, 0, 0]
    );
}
