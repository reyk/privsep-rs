//! Privilege Separation for Rust.
//!
//! This crate is **experimental** and **WIP**.
//!
//! Privilege separation[1] is a technique to split a program into
//! multiple isolated processes that only communicate via a strict and
//! well-defined internal messaging IPC with each other.  Unlike
//! containers or micro services, they still belong to one closely
//! coupled program.
//!
//! In the implementation of the `privsep` crate, a privileged parent
//! process forks and executes the unprivileged child processes.
//! Those processes drop privileges and run in a sandboxed
//! environment; communication is done via an async socket pair using
//! `imsg` channels.
//!
//! The most popular implementation of a privilege-separated network
//! service is OpenSSH.  Another example is OpenBSD's relayd, an async
//! and privilege-separated load balancer that is written in C.
//!
//! # Examples
//!
//! relayd uses four types of processes: the health check engine
//! (hce), the packet filter engine (pfe), the relay processes, and
//! the privileged parent process.  When implemented using the
//! [`privsep-derive`] crate, the model could be expressed like the
//! following example:
//!
//! ```ignore
//! mod health;
//! mod parent;
//! mod redirect;
//! mod relay;
//!
//! use privsep_derive::Privsep;
//!
//! /// Privsep processes.
//! #[derive(Debug, Privsep)]
//! #[username = "_relayd"]
//! pub enum Privsep {
//!     /// Parent process
//!     Parent,
//!     /// Health Check Engine
//!     #[connect(Relay, Redirect)]
//!     Health,
//!     /// Packet Filter Engine
//!     Redirect,
//!     /// L7 Relays
//!     Relay,
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     if let Err(err) = Privsep::main().await {
//!         eprintln!("Error: {}", err);
//!     }
//! }
//! ```
//!
//! See [`simple.rs`] for a more complete example.
//!
//! [1]: https://en.wikipedia.org/wiki/Privilege_separation
//! [`privsep-derive`]: https://docs.rs/privsep-derive/
//! [`simple.rs`]: https://github.com/reyk/privsep-rs/blob/main/privsep/examples/simple.rs

mod error;
pub mod imsg;
pub mod net;
pub mod process;

pub use {error::Error, process::Config};
