//! The queue is written from several threads at once: the composer path
//! appends while a drain takes the head and a snapshot exports the doc.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use loams_agentd_doc::{QueuedMessage, SessionDoc};

fn row(id: String) -> QueuedMessage {
    QueuedMessage {
        id,
        text: "queued".into(),
        attachments: Vec::new(),
        hold_for_turn_end: false,
        issued_by: "device".into(),
        issued_at: 1,
        edited_at: None,
        delivery_gate: None,
    }
}

#[test]
fn concurrent_push_take_and_export_never_fail() {
    for round in 0..10 {
        let doc = Arc::new(SessionDoc::init(&format!("chat-{round}")).unwrap());
        let done = Arc::new(AtomicBool::new(false));
        let readers: Vec<_> = (0..2)
            .map(|reader| {
                let doc = doc.clone();
                let done = done.clone();
                std::thread::spawn(move || {
                    while !done.load(Ordering::Acquire) {
                        if reader == 0 {
                            doc.take_queue_head().unwrap();
                        } else {
                            doc.export_snapshot().unwrap();
                            doc.read_queue().unwrap();
                        }
                    }
                })
            })
            .collect();
        for i in 0..50 {
            doc.push_queued(&row(format!("r{round}-{i}"))).unwrap();
        }
        done.store(true, Ordering::Release);
        for reader in readers {
            reader.join().unwrap();
        }
    }
}
