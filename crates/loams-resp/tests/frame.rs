//! RESP frames and the request decoder (plan KV2 Task 1).

use bytes::{BufMut, Bytes, BytesMut};
use loams_resp::frame::{Decoder, Limits, Proto, ProtoError, Value, split_args};
use proptest::prelude::*;

fn multibulk(args: &[Vec<u8>]) -> Vec<u8> {
    let mut out = format!("*{}\r\n", args.len()).into_bytes();
    for a in args {
        out.extend_from_slice(format!("${}\r\n", a.len()).as_bytes());
        out.extend_from_slice(a);
        out.extend_from_slice(b"\r\n");
    }
    out
}

fn authed() -> Decoder {
    Decoder::new(Limits::default(), true)
}

/// Feeds `input` to a decoder in the given chunk sizes and returns every
/// request it yields.
fn feed(
    decoder: &mut Decoder,
    input: &[u8],
    cuts: &[usize],
) -> Result<Vec<Vec<Bytes>>, ProtoError> {
    let mut buf = BytesMut::new();
    let mut out = Vec::new();
    let mut at = 0;
    let mut cuts = cuts.iter().copied().chain(std::iter::repeat(usize::MAX));
    while at < input.len() {
        let n = cuts
            .next()
            .unwrap_or(usize::MAX)
            .max(1)
            .min(input.len() - at);
        buf.put_slice(&input[at..at + n]);
        at += n;
        while let Some(req) = decoder.decode(&mut buf)? {
            out.push(req);
        }
    }
    assert!(buf.is_empty(), "{} bytes left over", buf.len());
    Ok(out)
}

fn to_vecs(reqs: Vec<Vec<Bytes>>) -> Vec<Vec<Vec<u8>>> {
    reqs.into_iter()
        .map(|r| r.into_iter().map(|b| b.to_vec()).collect())
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    /// Arbitrary binary arguments, pipelined and split at arbitrary points,
    /// decode to the same requests in order.
    #[test]
    fn request_roundtrip(
        reqs in prop::collection::vec(
            prop::collection::vec(prop::collection::vec(any::<u8>(), 0..40), 1..6),
            1..5,
        ),
        cuts in prop::collection::vec(1usize..17, 0..40),
    ) {
        let mut input = Vec::new();
        for r in &reqs {
            input.extend(multibulk(r));
        }
        let got = to_vecs(feed(&mut authed(), &input, &cuts).expect("decodes"));
        prop_assert_eq!(got, reqs);
    }

    /// Every reply encodes and decodes back in RESP3.
    #[test]
    fn reply_roundtrip(v in value()) {
        let bytes = v.to_bytes(Proto::Resp3);
        let (back, n) = Value::decode(&bytes).expect("valid").expect("whole");
        prop_assert_eq!(n, bytes.len());
        prop_assert_eq!(&back, &v);
        // RESP2 output is always decodable too, and every proper prefix is
        // incomplete rather than wrong.
        let two = v.to_bytes(Proto::Resp2);
        prop_assert!(Value::decode(&two).expect("valid").is_some());
        for cut in 0..two.len() {
            prop_assert!(Value::decode(&two[..cut]).expect("valid prefix").is_none());
        }
    }
}

fn value() -> impl Strategy<Value = Value> {
    let text = "[a-zA-Z0-9 _.:-]{0,12}";
    let leaf = prop_oneof![
        text.prop_map(Value::Simple),
        text.prop_map(|t| Value::Error(format!("ERR {t}"))),
        any::<i64>().prop_map(Value::Integer),
        prop::collection::vec(any::<u8>(), 0..24).prop_map(|b| Value::Bulk(b.into())),
        Just(Value::Null),
        any::<bool>().prop_map(Value::Boolean),
        (-1e300f64..1e300).prop_map(Value::Double),
        "-?[1-9][0-9]{0,40}".prop_map(Value::BigNumber),
        text.prop_map(|t| Value::Verbatim("txt".into(), Bytes::from(t))),
    ];
    leaf.prop_recursive(3, 32, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Set),
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Push),
            prop::collection::vec((inner.clone(), inner), 0..3).prop_map(Value::Map),
        ]
    })
}

#[test]
fn encode_resp2_and_resp3() {
    let cases: Vec<(Value, &str, &str)> = vec![
        (Value::ok(), "+OK\r\n", "+OK\r\n"),
        (Value::err("ERR x"), "-ERR x\r\n", "-ERR x\r\n"),
        (Value::Integer(-3), ":-3\r\n", ":-3\r\n"),
        (Value::bulk("ab"), "$2\r\nab\r\n", "$2\r\nab\r\n"),
        (Value::Null, "$-1\r\n", "_\r\n"),
        (Value::NullArray, "*-1\r\n", "_\r\n"),
        (
            Value::Array(vec![Value::Integer(1), Value::Null]),
            "*2\r\n:1\r\n$-1\r\n",
            "*2\r\n:1\r\n_\r\n",
        ),
        (
            Value::Map(vec![(Value::bulk("k"), Value::Integer(1))]),
            "*2\r\n$1\r\nk\r\n:1\r\n",
            "%1\r\n$1\r\nk\r\n:1\r\n",
        ),
        (
            Value::Set(vec![Value::bulk("a")]),
            "*1\r\n$1\r\na\r\n",
            "~1\r\n$1\r\na\r\n",
        ),
        (Value::Double(1.5), "$3\r\n1.5\r\n", ",1.5\r\n"),
        (Value::Double(f64::INFINITY), "$3\r\ninf\r\n", ",inf\r\n"),
        (Value::Boolean(true), ":1\r\n", "#t\r\n"),
        (Value::BigNumber("123".into()), "$3\r\n123\r\n", "(123\r\n"),
        (
            Value::Verbatim("txt".into(), Bytes::from_static(b"hi")),
            "$2\r\nhi\r\n",
            "=6\r\ntxt:hi\r\n",
        ),
        (
            Value::Push(vec![Value::bulk("m")]),
            "*1\r\n$1\r\nm\r\n",
            ">1\r\n$1\r\nm\r\n",
        ),
    ];
    for (v, two, three) in cases {
        assert_eq!(v.to_bytes(Proto::Resp2), two.as_bytes(), "{v:?} in RESP2");
        assert_eq!(v.to_bytes(Proto::Resp3), three.as_bytes(), "{v:?} in RESP3");
    }
}

#[test]
fn decoder_partial_input_is_incomplete() {
    let input = [
        multibulk(&[b"SET".to_vec(), b"k".to_vec(), b"v\r\n\x00".to_vec()]),
        b"PING\r\n".to_vec(),
        multibulk(&[b"GET".to_vec(), b"k".to_vec()]),
    ]
    .concat();
    let want = vec![
        vec![b"SET".to_vec(), b"k".to_vec(), b"v\r\n\x00".to_vec()],
        vec![b"PING".to_vec()],
        vec![b"GET".to_vec(), b"k".to_vec()],
    ];
    // Every split point, and byte by byte.
    for cut in 1..input.len() {
        let got = to_vecs(feed(&mut authed(), &input, &[cut]).expect("decodes"));
        assert_eq!(got, want, "split at {cut}");
    }
    let got = to_vecs(feed(&mut authed(), &input, &[1; 200]).expect("decodes"));
    assert_eq!(got, want);
}

#[test]
fn decoder_limits_before_allocation() {
    let limits = Limits {
        max_bulk: 1024,
        max_multibulk: 100,
        ..Limits::default()
    };
    let refuse = |input: &[u8], authenticated: bool, text: &str| {
        let mut buf = BytesMut::from(input);
        let capacity = buf.capacity();
        let err = Decoder::new(limits, authenticated)
            .decode(&mut buf)
            .expect_err("refused");
        assert_eq!(err.0, format!("Protocol error: {text}"), "{input:?}");
        assert!(
            buf.capacity() <= capacity.max(64),
            "the buffer grew before the check"
        );
    };
    refuse(b"*101\r\n", true, "invalid multibulk length");
    refuse(
        b"*99999999999999999999\r\n",
        true,
        "invalid multibulk length",
    );
    refuse(b"*x\r\n", true, "invalid multibulk length");
    refuse(b"*1\r\n$1025\r\n", true, "invalid bulk length");
    refuse(b"*1\r\n$-5\r\n", true, "invalid bulk length");
    refuse(b"*1\r\n:5\r\n", true, "expected '$', got ':'");
    // Before AUTH: at most 10 arguments of at most 16 KiB (capped by max_bulk).
    refuse(b"*11\r\n", false, "invalid multibulk length");
    refuse(b"*1\r\n$1025\r\n", false, "invalid bulk length");
    // A count line with no end in sight.
    let long = [b"*".as_slice(), &[b'1'; 70_000]].concat();
    refuse(&long, true, "too big mbulk count string");
    let inline = vec![b'a'; 70_000];
    refuse(&inline, true, "too big inline request");
}

#[test]
fn inline_commands() {
    let cases: Vec<(&[u8], Vec<&[u8]>)> = vec![
        (b"PING", vec![b"PING"]),
        (b"  set  k   v ", vec![b"set", b"k", b"v"]),
        (b"set \"a b\" 'c d'", vec![b"set", b"a b", b"c d"]),
        (b"set \"\\x41\\n\\t\\\\\" x", vec![b"set", b"A\n\t\\", b"x"]),
        (b"set 'it\\'s' \"\"", vec![b"set", b"it's", b""]),
        (b"", vec![]),
    ];
    for (line, want) in cases {
        let got = split_args(line).expect("balanced");
        let want: Vec<Vec<u8>> = want.into_iter().map(<[u8]>::to_vec).collect();
        assert_eq!(got, want, "{:?}", String::from_utf8_lossy(line));
    }
    for bad in [&b"set \"a"[..], b"set 'a", b"set \"a\"b", b"set 'a'b"] {
        assert_eq!(split_args(bad), None, "{:?}", String::from_utf8_lossy(bad));
    }
    // Through the decoder: \n or \r\n, blank lines skipped, then multibulk.
    let input = b"\r\n\nPING\nECHO \"hi there\"\r\n*1\r\n$4\r\nPING\r\n";
    let got = to_vecs(feed(&mut authed(), input, &[]).expect("decodes"));
    assert_eq!(
        got,
        vec![
            vec![b"PING".to_vec()],
            vec![b"ECHO".to_vec(), b"hi there".to_vec()],
            vec![b"PING".to_vec()],
        ]
    );
    let mut buf = BytesMut::from(&b"set \"a\r\n"[..]);
    assert_eq!(
        authed().decode(&mut buf).expect_err("unbalanced").0,
        "Protocol error: unbalanced quotes in request"
    );
}

#[test]
fn pipelined_requests_decode_in_order() {
    let mut input = Vec::new();
    let mut want = Vec::new();
    for i in 0..500 {
        let req = vec![b"INCR".to_vec(), format!("k{i}").into_bytes()];
        input.extend(multibulk(&req));
        want.push(req);
    }
    // Empty multibulk requests in between are skipped.
    input.extend_from_slice(b"*0\r\n*-1\r\n");
    input.extend(multibulk(&[b"PING".to_vec()]));
    want.push(vec![b"PING".to_vec()]);
    let got = to_vecs(feed(&mut authed(), &input, &[4096]).expect("decodes"));
    assert_eq!(got, want);
}

#[test]
fn arguments_share_the_read_buffer() {
    let mut buf = BytesMut::from(&multibulk(&[b"SET".to_vec(), vec![7u8; 4096]])[..]);
    let req = authed().decode(&mut buf).expect("ok").expect("whole");
    assert_eq!(req[1].len(), 4096);
    // Zero-copy: the two arguments are slices of one allocation.
    let gap = req[1].as_ptr() as usize - req[0].as_ptr() as usize;
    assert_eq!(gap, b"SET\r\n$4096\r\n".len());
}
