//! Task management tools and the per-kind executor framework.
//!
//! The `tools` submodule houses the LLM-facing [`Tool`] implementations
//! (`TaskCreate`, `TaskList`, etc.). The `executor` and `executors`
//! submodules define the kind-tagged execution pipeline used to run
//! whatever a [`TaskKind`](crate::services::background::TaskKind)
//! describes.
//!
//! # Architecture
//!
//! - `TaskKind` (in `services::background`) — what kind of work the
//!   task represents.
//! - `TaskPayload` (in `services::background`) — kind-specific data
//!   carried alongside the task record.
//! - [`executor::TaskExecutor`] — trait every kind implements.
//! - [`executor::TaskExecutorRegistry`] — analogue of the tool registry,
//!   keyed by `TaskKind`.

pub mod executor;
pub mod executors;
pub mod tools;

pub use executor::{
    TaskContext, TaskError, TaskExecutor, TaskExecutorRegistry, TaskResult, default_registry,
};
pub use executors::resolve_workflow_prompt;
pub use tools::{
    TaskCreateTool, TaskGetTool, TaskListTool, TaskOutputTool, TaskStopTool, TaskUpdateTool,
};
