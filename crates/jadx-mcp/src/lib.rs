//! Model Context Protocol server exposing the decompiler engine to AI clients.
//!
//! The server speaks newline delimited JSON-RPC over stdio or a Unix domain socket. All tools
//! work against the [`project::Project`] trait; [`engine::EngineProject`] implements it over the
//! engine crates, so a different backend only needs another implementation and a
//! [`project::ProjectLoader`].
//!
//! Parts of the transport, tool registry and tool conventions are adapted from the RDX MCP
//! server (Apache License 2.0, Copyright 2026 Ch0pin and RDX contributors); see the NOTICE file
//! of the repository and the header comment of each adapted module.

pub mod args;
pub mod cli;
pub mod engine;
pub mod error;
pub mod index;
pub mod manifest;
pub mod project;
pub mod protocol;
pub mod registry;
pub mod server;
pub mod skeleton;
pub mod socket;
pub mod tools;

pub use engine::{EngineLoader, EngineProject};
pub use project::{Project, ProjectLoader};
pub use protocol::serve;
pub use server::{Server, SharedState};
