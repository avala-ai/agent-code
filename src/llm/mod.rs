//! LLM client layer.
//!
//! Handles all communication with LLM APIs. Supports OpenAI-compatible
//! and Anthropic-native APIs with streaming via Server-Sent Events (SSE).
//!
//! # Architecture
//!
//! - `client` — HTTP client with retry logic and streaming
//! - `message` — Message types for the conversation protocol
//! - `stream` — SSE parser that yields `StreamEvent` values

pub mod client;
pub mod message;
pub mod stream;
