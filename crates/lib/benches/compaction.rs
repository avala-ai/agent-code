use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

use agent_code_lib::llm::message::{AssistantMessage, ContentBlock, Message, UserMessage};
use agent_code_lib::services::compact::microcompact;
use uuid::Uuid;

fn make_conversation(turns: usize) -> Vec<Message> {
    let mut messages = Vec::with_capacity(turns * 2);
    for i in 0..turns {
        messages.push(Message::User(UserMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![ContentBlock::Text {
                text: format!("User message {i} with some content to estimate tokens from"),
            }],
            is_meta: false,
            is_compact_summary: false,
        }));
        messages.push(Message::Assistant(AssistantMessage {
            uuid: Uuid::new_v4(),
            timestamp: String::new(),
            content: vec![
                ContentBlock::Text {
                    text: format!("Assistant response {i} with a longer body of text that simulates a real response from the model"),
                },
                ContentBlock::ToolUse {
                    id: format!("tool_{i}"),
                    name: "FileRead".to_string(),
                    input: serde_json::json!({"file_path": "src/main.rs"}),
                },
                ContentBlock::ToolResult {
                    tool_use_id: format!("tool_{i}"),
                    content: format!("File content line 1\nFile content line 2\nFile content line 3\n{}", "x".repeat(500)),
                    is_error: false,
                    extra_content: vec![],
                },
            ],
            model: Some("test-model".to_string()),
            usage: None,
            stop_reason: None,
            request_id: None,
        }));
    }
    messages
}

fn bench_microcompact(c: &mut Criterion) {
    let mut group = c.benchmark_group("microcompact");

    for turns in [10, 50, 100, 500] {
        group.bench_function(format!("{turns}_turns"), |b| {
            b.iter_batched(
                || make_conversation(turns),
                |mut msgs| {
                    black_box(microcompact(&mut msgs, 2));
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, bench_microcompact);
criterion_main!(benches);
