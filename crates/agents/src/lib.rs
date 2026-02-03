//! LLM agent runtime: model selection, prompt building, tool execution, streaming.

pub mod auth_profiles;
pub mod model;
pub mod prompt;
pub mod providers;
pub mod runner;
pub use runner::AgentRunError;
pub mod provider_chain;
pub mod silent_turn;
pub mod skills;
pub mod tool_registry;
