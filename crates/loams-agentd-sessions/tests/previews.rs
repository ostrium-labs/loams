use futures::StreamExt;
use loams_agentd_preview::discovery::Listener;
use loams_agentd_proto::HarnessId;
use loams_agentd_rpc::{RpcReply, RpcService, methods};
use loams_agentd_sessions::{EngineCore, HarnessRegistry};
use std::{sync::Arc, time::Duration};

fn server(cwd: &std::path::Path, port: u16, pid: u32) -> Listener {
    Listener {
        pid,
        parent: 1,
        cwd: cwd.to_path_buf(),
        args: vec![
            "node".into(),
            "vite".into(),
            "--port".into(),
            port.to_string(),
        ],
        started_at: pid as u64,
        address: ([127, 0, 0, 1], port).into(),
        loams_desktop_owned: true,
    }
}

#[tokio::test]
async fn preview_watch_follows_the_session_checkout() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("a")).unwrap();
    std::fs::create_dir(temp.path().join("b")).unwrap();
    let a = temp.path().join("a").canonicalize().unwrap();
    let b = temp.path().join("b").canonicalize().unwrap();
    let core = EngineCore::assemble(
        &temp.path().join("engine"),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
    )
    .unwrap();
    let device = core.device_id.clone();
    core.workspace
        .create_chat(
            "chat",
            None,
            Some(&device),
            None,
            Some(a.to_string_lossy().into_owned()),
        )
        .unwrap();
    let catalog = core.previews.catalog();
    catalog
        .replace_local(vec![
            (a.clone(), server(&a, 5173, 11)),
            (b.clone(), server(&b, 5174, 12)),
        ])
        .unwrap();
    let rpc = core.rpc_service();
    let RpcReply::Stream(mut stream) = rpc
        .handle(
            methods::WATCH_PREVIEWS,
            serde_json::json!({"chatId":"chat"}),
        )
        .await
        .unwrap()
    else {
        panic!("expected preview watch")
    };
    let first = stream.next().await.unwrap();
    assert_eq!(first["services"].as_array().unwrap().len(), 1);
    assert_eq!(first["services"][0]["port"], 5173);
    assert_eq!(first["remote"], false);
    core.workspace
        .set_chat_cwd("chat", &b.to_string_lossy())
        .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next["services"][0]["port"], 5174);
    catalog.replace_local(Vec::new()).unwrap();
    let next = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next["services"], serde_json::json!([]));
    drop(stream);
    core.shutdown().await;
}
