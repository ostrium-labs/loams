//! Budgets: refuse before the call, settle the reported tokens, reset at
//! 00:00 UTC.

mod common;

use std::sync::Arc;

use common::{MockServer, Reply, fast_http, fixture, fixture_path, person_request, person_schema};
use loams_llm::{
    Budget, ChatModel, DailyBudget, Embedder, FakeChat, FakeEmbedder, LlmError, LocalPolicy,
    ManualClock, ModelRef, OpenAiCompatible, OpenAiOptions, RunBudget, StaticSecrets,
};

async fn local_model(server: &MockServer) -> OpenAiCompatible {
    let mut id = ModelRef::ollama("llama-fixture");
    id.base_url = server.url("/v1");
    OpenAiCompatible::connect(
        id,
        &LocalPolicy::open(),
        &StaticSecrets::default(),
        OpenAiOptions {
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await
    .expect("connect")
}

#[tokio::test]
async fn budget_exhausted_refuses_before_call() {
    let server = MockServer::start(vec![Reply::ok(fixture(
        "json/openai-chat-json-schema.json",
    ))])
    .await;
    let model = local_model(&server).await;
    let req = person_request("Ada");
    let estimate = req.estimate(&person_schema());

    // Each cap on its own refuses a call that would pass it.
    let budgets = [
        Budget::unlimited().with_daily(Arc::new(DailyBudget::new(estimate - 1))),
        Budget::unlimited().with_run(Arc::new(RunBudget::new(estimate - 1))),
        Budget::unlimited().with_per_call(estimate - 1),
    ];
    for budget in &budgets {
        let err = model
            .complete_json(req.clone(), &person_schema(), budget)
            .await
            .expect_err("refused");
        assert_eq!(err, LlmError::BudgetExhausted);
    }
    assert!(server.seen().is_empty(), "nothing reached the provider");

    let fake = FakeChat::from_fixture_file(fixture_path("json/fake-chat.json")).expect("fixture");
    let err = fake
        .complete_json(req.clone(), &person_schema(), &budgets[0])
        .await
        .expect_err("refused");
    assert_eq!(err, LlmError::BudgetExhausted);
    assert_eq!(fake.calls(), 0);

    let embedder = FakeEmbedder::new("fake-embed", 8, 1).expect("embedder");
    let err = embedder
        .embed(
            &["a long enough text".to_string()],
            &Budget::unlimited().with_per_call(1),
        )
        .await
        .expect_err("refused");
    assert_eq!(err, LlmError::BudgetExhausted);
}

#[tokio::test]
async fn budget_settles_actual_tokens() {
    let server = MockServer::start(vec![
        Reply::ok(fixture("json/openai-chat-json-schema.json")),
        Reply::status(400, "{}"),
    ])
    .await;
    let model = local_model(&server).await;
    let daily = Arc::new(DailyBudget::new(100_000));
    let run = Arc::new(RunBudget::new(100_000));
    let budget = Budget::unlimited()
        .with_daily(daily.clone())
        .with_run(run.clone());

    model
        .complete_json(person_request("Ada"), &person_schema(), &budget)
        .await
        .expect("answer");
    // The fixture reports 61 + 19 tokens; the reservation was larger.
    assert_eq!(daily.used(), 80);
    assert_eq!(run.used(), 80);

    // A refused call releases its reservation and charges nothing.
    model
        .complete_json(person_request("Ada"), &person_schema(), &budget)
        .await
        .expect_err("400");
    assert_eq!(daily.used(), 80);

    // Only the reserved remainder is free: a held reservation blocks a
    // call that would pass the cap together with it.
    let small = Arc::new(RunBudget::new(1_000));
    let b = Budget::unlimited().with_run(small.clone());
    let held = b.reserve(600).expect("reserve");
    assert_eq!(b.reserve(600).expect_err("over"), LlmError::BudgetExhausted);
    held.settle(100);
    assert_eq!(small.used(), 100);
    b.reserve(600).expect("free again").settle(600);
    assert_eq!(small.used(), 700);

    // The fake settles its scripted counts.
    let fake = FakeChat::from_fixture_file(fixture_path("json/fake-chat.json")).expect("fixture");
    let run = Arc::new(RunBudget::new(100_000));
    fake.complete_json(
        person_request("Ada"),
        &person_schema(),
        &Budget::unlimited().with_run(run.clone()),
    )
    .await
    .expect("answer");
    assert_eq!(run.used(), 110);
}

#[tokio::test]
async fn budget_resets_at_utc_midnight() {
    // 2026-10-19 23:59:30 UTC.
    let clock = Arc::new(ManualClock::new(1_792_454_370));
    let daily = Arc::new(DailyBudget::with_clock(1_000, clock.clone()));
    let budget = Budget::unlimited().with_daily(daily.clone());
    budget.reserve(900).expect("reserve").settle(900);
    assert_eq!(
        budget.reserve(200).expect_err("over"),
        LlmError::BudgetExhausted
    );

    // A reservation that straddles midnight is charged to the new day.
    let straddling = budget.reserve(100).expect("reserve");
    clock.advance(29);
    assert_eq!(daily.used(), 900, "still the same day");
    clock.advance(1);
    assert_eq!(daily.used(), 0, "00:00:00 UTC is a new day");
    budget
        .reserve(950)
        .expect("the new day's budget")
        .settle(950);
    straddling.settle(40);
    assert_eq!(daily.used(), 990);
    assert_eq!(
        budget.reserve(20).expect_err("over"),
        LlmError::BudgetExhausted
    );
}
