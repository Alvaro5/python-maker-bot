//! Real-time web dashboard module.
//!
//! Provides a local web interface for Python Maker Bot with script history,
//! code generation, real-time execution logs, and session statistics.

pub mod routes;
pub mod server;
pub mod state;
pub mod templates;
pub mod websocket;

pub use server::start_dashboard;
pub use state::{DashboardState, ExecutionEvent};
