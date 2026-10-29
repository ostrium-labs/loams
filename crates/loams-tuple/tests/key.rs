//! Property tests of `loams_tuple::key` (KV1 plan KV0 Task 1; design §77
//! §7): order, composites, round trips, prefix-freedom and agreement with
//! the tuple codec.

use std::borrow::Cow;
use std::cmp::Ordering;

use loams_tuple::key;
use loams_tuple::tuple::{self, Elem};
use proptest::prelude::*;

/// One primitive component.
#[derive(Debug, Clone)]
enum Val {
    U16(u16),
    U32(u32),
    U64(u64),
    I64(i64),
    F64(f64),
    Bytes(Vec<u8>),
}

const KINDS: u8 = 6;

/// Floats as the encoding orders them: `-0.0 == 0.0`, every NaN equal and
/// greater than everything else.
fn cmp_f64(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.partial_cmp(&b).expect("neither is NaN"),
    }
}

/// The value order of two components of the same kind.
fn cmp_val(a: &Val, b: &Val) -> Ordering {
    match (a, b) {
        (Val::U16(a), Val::U16(b)) => a.cmp(b),
        (Val::U32(a), Val::U32(b)) => a.cmp(b),
        (Val::U64(a), Val::U64(b)) => a.cmp(b),
        (Val::I64(a), Val::I64(b)) => a.cmp(b),
        (Val::F64(a), Val::F64(b)) => cmp_f64(*a, *b),
        (Val::Bytes(a), Val::Bytes(b)) => a.cmp(b),
        _ => panic!("components of different kinds: {a:?} {b:?}"),
    }
}

fn same(a: &Val, b: &Val) -> bool {
    cmp_val(a, b) == Ordering::Equal
}

fn put(out: &mut Vec<u8>, v: &Val, descending: bool) {
    let start = out.len();
    match v {
        Val::U16(v) => key::put_u16(out, *v),
        Val::U32(v) => key::put_u32(out, *v),
        Val::U64(v) => key::put_u64(out, *v),
        Val::I64(v) => key::put_i64(out, *v),
        Val::F64(v) => key::put_f64(out, *v),
        Val::Bytes(v) => key::put_bytes(out, v),
    }
    if descending {
        key::desc(&mut out[start..]);
    }
}

fn enc(v: &Val, descending: bool) -> Vec<u8> {
    let mut out = Vec::new();
    put(&mut out, v, descending);
    out
}

fn get(buf: &[u8], like: &Val, descending: bool) -> (Val, usize) {
    let r = match (like, descending) {
        (Val::U16(_), false) => key::get_u16(buf).map(|(v, n)| (Val::U16(v), n)),
        (Val::U16(_), true) => key::get_u16_desc(buf).map(|(v, n)| (Val::U16(v), n)),
        (Val::U32(_), false) => key::get_u32(buf).map(|(v, n)| (Val::U32(v), n)),
        (Val::U32(_), true) => key::get_u32_desc(buf).map(|(v, n)| (Val::U32(v), n)),
        (Val::U64(_), false) => key::get_u64(buf).map(|(v, n)| (Val::U64(v), n)),
        (Val::U64(_), true) => key::get_u64_desc(buf).map(|(v, n)| (Val::U64(v), n)),
        (Val::I64(_), false) => key::get_i64(buf).map(|(v, n)| (Val::I64(v), n)),
        (Val::I64(_), true) => key::get_i64_desc(buf).map(|(v, n)| (Val::I64(v), n)),
        (Val::F64(_), false) => key::get_f64(buf).map(|(v, n)| (Val::F64(v), n)),
        (Val::F64(_), true) => key::get_f64_desc(buf).map(|(v, n)| (Val::F64(v), n)),
        (Val::Bytes(_), false) => key::get_bytes(buf).map(|(v, n)| (Val::Bytes(v), n)),
        (Val::Bytes(_), true) => key::get_bytes_desc(buf).map(|(v, n)| (Val::Bytes(v), n)),
    };
    r.expect("a valid encoding decodes")
}

/// Bytes biased towards the escape's interesting values.
fn bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(
        prop_oneof![
            Just(0x00u8),
            Just(0xFF),
            Just(0x01),
            Just(0xFE),
            any::<u8>()
        ],
        0..12,
    )
}

/// Floats including the specials (both zeros, NaNs, infinities, subnormals).
fn float() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(f64::NAN),
        Just(-f64::NAN),
        Just(f64::INFINITY),
        Just(f64::NEG_INFINITY),
        Just(f64::MIN_POSITIVE),
        Just(-f64::MIN_POSITIVE),
        prop::num::f64::ANY,
        any::<u64>().prop_map(f64::from_bits),
        (-4i32..4).prop_map(f64::from),
    ]
}

fn small_i64() -> impl Strategy<Value = i64> {
    prop_oneof![any::<i64>(), -3i64..3, Just(i64::MIN), Just(i64::MAX)]
}

fn val(kind: u8) -> BoxedStrategy<Val> {
    match kind {
        0 => prop_oneof![any::<u16>(), 0u16..3]
            .prop_map(Val::U16)
            .boxed(),
        1 => prop_oneof![any::<u32>(), 0u32..3]
            .prop_map(Val::U32)
            .boxed(),
        2 => prop_oneof![any::<u64>(), 0u64..3, Just(u64::MAX)]
            .prop_map(Val::U64)
            .boxed(),
        3 => small_i64().prop_map(Val::I64).boxed(),
        4 => float().prop_map(Val::F64).boxed(),
        _ => bytes().prop_map(Val::Bytes).boxed(),
    }
}

/// Two components of one kind, and a direction.
fn pair() -> impl Strategy<Value = (Val, Val, bool)> {
    (0..KINDS, any::<bool>()).prop_flat_map(|(k, d)| (val(k), val(k), Just(d)))
}

/// A composite shape of two or three (kind, descending) components, and two
/// keys of that shape. The second key shares a leading run of the first's
/// components often, so later components decide the order too.
fn composites() -> impl Strategy<Value = (Vec<bool>, Vec<Val>, Vec<Val>)> {
    prop::collection::vec((0..KINDS, any::<bool>()), 2..=3).prop_flat_map(|shape| {
        let dirs: Vec<bool> = shape.iter().map(|&(_, d)| d).collect();
        let a: Vec<BoxedStrategy<Val>> = shape.iter().map(|&(k, _)| val(k)).collect();
        let b: Vec<BoxedStrategy<Val>> = shape.iter().map(|&(k, _)| val(k)).collect();
        let n = shape.len();
        (Just(dirs), a, b, 0..=n).prop_map(|(dirs, a, mut b, shared)| {
            b[..shared].clone_from_slice(&a[..shared]);
            (dirs, a, b)
        })
    })
}

fn encode_all(vals: &[Val], dirs: &[bool]) -> Vec<u8> {
    let mut out = Vec::new();
    for (v, &d) in vals.iter().zip(dirs) {
        put(&mut out, v, d);
    }
    out
}

fn cmp_all(a: &[Val], b: &[Val], dirs: &[bool]) -> Ordering {
    for ((x, y), &d) in a.iter().zip(b).zip(dirs) {
        let o = cmp_val(x, y);
        let o = if d { o.reverse() } else { o };
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

fn tuple_enc(e: &Elem) -> Vec<u8> {
    let mut out = Vec::new();
    tuple::encode(&mut out, e);
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    fn key_order_matches_value_order((a, b, d) in pair()) {
        let want = cmp_val(&a, &b);
        prop_assert_eq!(enc(&a, false).cmp(&enc(&b, false)), want);
        prop_assert_eq!(enc(&a, true).cmp(&enc(&b, true)), want.reverse());
        let want = if d { want.reverse() } else { want };
        prop_assert_eq!(enc(&a, d).cmp(&enc(&b, d)), want);
    }

    #[test]
    fn key_composites_order_lexicographically((dirs, a, b) in composites()) {
        let (ea, eb) = (encode_all(&a, &dirs), encode_all(&b, &dirs));
        prop_assert_eq!(ea.cmp(&eb), cmp_all(&a, &b, &dirs));
        // And each composite decodes back component by component.
        let mut pos = 0;
        for (v, &d) in a.iter().zip(&dirs) {
            let (got, n) = get(&ea[pos..], v, d);
            prop_assert!(same(&got, v), "{:?} decoded as {:?}", v, got);
            pos += n;
        }
        prop_assert_eq!(pos, ea.len());
    }

    #[test]
    fn key_roundtrip((v, _, d) in pair()) {
        for descending in [false, true, d] {
            let buf = enc(&v, descending);
            let (got, n) = get(&buf, &v, descending);
            prop_assert!(same(&got, &v), "{:?} decoded as {:?}", v, got);
            prop_assert_eq!(n, buf.len());
            // The decoded value encodes to the same bytes (NaN and -0.0
            // canonical).
            prop_assert_eq!(enc(&got, descending), buf);
        }
    }

    #[test]
    fn key_no_prefix_aliasing(a in bytes(), b in bytes(), d in any::<bool>()) {
        prop_assume!(a != b);
        let (ea, eb) = (enc(&Val::Bytes(a), d), enc(&Val::Bytes(b), d));
        prop_assert!(!eb.starts_with(&ea), "{:?} is a prefix of {:?}", ea, eb);
        prop_assert!(!ea.starts_with(&eb), "{:?} is a prefix of {:?}", eb, ea);
    }

    #[test]
    fn key_agrees_with_tuple(
        (i, j) in (small_i64(), small_i64()),
        (f, g) in (float(), float()),
        (x, y) in (bytes(), bytes()),
    ) {
        let key_i = enc(&Val::I64(i), false).cmp(&enc(&Val::I64(j), false));
        prop_assert_eq!(key_i, tuple_enc(&Elem::I64(i)).cmp(&tuple_enc(&Elem::I64(j))));
        let key_f = enc(&Val::F64(f), false).cmp(&enc(&Val::F64(g), false));
        prop_assert_eq!(key_f, tuple_enc(&Elem::F64(f)).cmp(&tuple_enc(&Elem::F64(g))));
        let key_b = enc(&Val::Bytes(x.clone()), false).cmp(&enc(&Val::Bytes(y.clone()), false));
        let tuple_b = tuple_enc(&Elem::Bytes(Cow::Borrowed(&x)))
            .cmp(&tuple_enc(&Elem::Bytes(Cow::Borrowed(&y))));
        prop_assert_eq!(key_b, tuple_b);
    }
}
