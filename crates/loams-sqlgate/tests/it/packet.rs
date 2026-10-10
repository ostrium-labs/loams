use loams_sqlgate::codec::packet::{Assembler, FrameError, MAX_FRAME, encode};

fn frames(bytes: &[u8]) -> Vec<(usize, u8)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let len = usize::from(bytes[i])
            | usize::from(bytes[i + 1]) << 8
            | usize::from(bytes[i + 2]) << 16;
        out.push((len, bytes[i + 3]));
        i += 4 + len;
    }
    out
}

fn reassemble(bytes: &[u8], max: usize) -> Vec<u8> {
    let mut a = Assembler::new(max);
    let (used, msg) = a.push(bytes).expect("valid frames");
    assert_eq!(used, bytes.len());
    msg.expect("complete message").payload
}

#[test]
fn packet_split_at_16mib() {
    assert_eq!(MAX_FRAME, 0xFF_FFFF);
    // Below, at and above the 16 MiB - 1 frame limit, and at twice it.
    for (len, want) in [
        (0, vec![(0, 0)]),
        (MAX_FRAME - 1, vec![(MAX_FRAME - 1, 0)]),
        (MAX_FRAME, vec![(MAX_FRAME, 0), (0, 1)]),
        (MAX_FRAME + 1, vec![(MAX_FRAME, 0), (1, 1)]),
        (2 * MAX_FRAME, vec![(MAX_FRAME, 0), (MAX_FRAME, 1), (0, 2)]),
    ] {
        let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let mut seq = 0u8;
        let mut out = Vec::new();
        encode(&payload, &mut seq, &mut out);
        assert_eq!(frames(&out), want, "len {len}");
        assert_eq!(usize::from(seq), want.len(), "next sequence id");
        assert_eq!(reassemble(&out, 3 * MAX_FRAME), payload, "len {len}");
    }
}

#[test]
fn sequence_ids_wrap_and_are_checked() {
    let mut seq = 255u8;
    let mut out = Vec::new();
    encode(b"a", &mut seq, &mut out);
    assert_eq!((out[3], seq), (255, 0));

    // A frame out of sequence is refused.
    let mut a = Assembler::new(1024);
    a.expect_seq(3);
    let mut s = 4u8;
    let mut bad = Vec::new();
    encode(b"x", &mut s, &mut bad);
    assert_eq!(
        a.push(&bad),
        Err(FrameError::Sequence {
            expected: 3,
            got: 4
        })
    );

    // The assembler tracks the next id across messages.
    let mut a = Assembler::new(1024);
    let mut s = 0u8;
    let mut two = Vec::new();
    encode(b"first", &mut s, &mut two);
    encode(b"second", &mut s, &mut two);
    let (used, m) = a.push(&two).expect("first");
    assert_eq!(
        (m.expect("msg").payload, a.next_seq()),
        (b"first".to_vec(), 1)
    );
    let (_, m) = a.push(&two[used..]).expect("second");
    assert_eq!((m.expect("msg").seq, a.next_seq()), (1, 2));
}

#[test]
fn oversized_messages_are_refused_from_the_header() {
    let mut a = Assembler::new(100);
    // Only the 4-byte header has arrived: the length alone refuses it.
    assert_eq!(
        a.push(&[101, 0, 0, 0]),
        Err(FrameError::TooLarge { limit: 100 })
    );
    // A split message over the limit is refused at the frame that crosses it.
    let mut a = Assembler::new(MAX_FRAME + 10);
    let mut header = vec![0xff, 0xff, 0xff, 0];
    header.extend(std::iter::repeat_n(0u8, MAX_FRAME));
    let (used, m) = a.push(&header).expect("first frame fits");
    assert_eq!((used, m), (header.len(), None));
    assert_eq!(
        a.push(&[11, 0, 0, 1]),
        Err(FrameError::TooLarge {
            limit: MAX_FRAME + 10
        })
    );
}

#[test]
fn partial_input_consumes_nothing_until_a_frame_is_whole() {
    let mut seq = 0;
    let mut out = Vec::new();
    encode(b"hello", &mut seq, &mut out);
    let mut a = Assembler::new(64);
    for cut in 0..out.len() {
        assert_eq!(
            a.push(&out[..cut]).expect("partial"),
            (0, None),
            "cut {cut}"
        );
    }
    assert_eq!(
        a.push(&out).expect("whole").1.expect("msg").payload,
        b"hello"
    );
}
