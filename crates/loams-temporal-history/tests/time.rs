use loams_temporal_history::time::{dur, dur_ms, ms, ts};

#[test]
fn millisecond_round_trips() {
    for v in [0, 1, 999, 1000, 1_761_300_000_123, -1, -1001] {
        assert_eq!(ms(&ts(v)), v);
        assert_eq!(dur_ms(Some(&dur(v))), v);
    }
    assert_eq!(dur_ms(None), 0);
}
