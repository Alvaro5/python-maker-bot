//! Real-time web dashboard module.
//!
//! Provides a local web interface for Python Maker Bot with script history,
//! code generation, multi-turn chat, code execution, model switching,
//! lint/security tools, and session statistics.

pub mod routes;
pub mod server;
pub mod state;
pub mod templates;
pub mod websocket;

pub use server::start_dashboard;
pub use state::{ChatSession, DashboardState, ExecutionEvent, RuntimeSettings};
