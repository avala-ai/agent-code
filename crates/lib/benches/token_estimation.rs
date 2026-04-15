use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

use agent_code_lib::llm::message::{AssistantMessage, ContentBlock, Message, UserMessage};
use agent_code_lib::services::tokens::{estimate_context_tokens, estimate_tokens};
use uuid::Uuid;

fn bench_estimate_tokens(c: &mut Criterion) {
    let mut group = c.benchmark_group("estimate_tokens");

    for size in [100, 1_000, 10_000, 100_000] {
        let text = "word ".repeat(size / 5);
        group.bench_function(format!("{size}_chars"), |b| {
            b.iter(|| {
                black_box(estimate_tokens(&text));
            });
        });
    }

    group.finish();
}

fn make_messages(count: usize) -> Vec<Message> {
    let mut msgs = Vec::with_capacity(count);
    for i in 0..count {
        if i % 2 == 0 {
            msgs.push(Message::User(UserMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::Text {
                    text: format!("Message {i}: {}", "content ".repeat(20)),
                }],
                is_meta: false,
                is_compact_summary: false,
            }));
        } else {
            msgs.push(Message::Assistant(AssistantMessage {
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                content: vec![ContentBlock::Text {
                    text: format!("Response {i}: {}", "response text ".repeat(30)),
                }],
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            }));
        }
    }
    msgs
}

fn bench_estimate_context_tokens(c: &mut Criterion) {
    let mut group = c.benchmark_group("estimate_context_tokens");

    for count in [10, 50, 200, 1000] {
        let messages = make_messages(count);
        group.bench_function(format!("{count}_messages"), |b| {
            b.iter(|| {
                black_box(estimate_context_tokens(&messages));
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_estimate_tokens,
    bench_estimate_context_tokens
);
criterion_main!(benches);
