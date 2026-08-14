//! Harness Control Interface (HCI).
//!
//! Exposes a JSON-RPC 2.0 surface over TCP on localhost so external agents
//! (and the in-process Health Agent) can observe and drive a running
//! `harness.exe` without going through the WebView. See
//! `docs/superpowers/specs/2026-08-12--hci-and-binary-optimization.md` for
//! the design.

pub mod auth;
pub mod conn_info;
pub mod integration;
pub mod methods_canvas;
pub mod methods_sessions_agents;
pub mod methods_system;
pub mod methods_workspace;
pub mod router;
pub mod server;

pub use methods_canvas::CanvasEventBus;
