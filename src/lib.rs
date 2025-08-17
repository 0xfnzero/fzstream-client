//! # FZ Stream Client SDK
//! 
//! A high-performance QUIC client SDK for real-time Solana event streaming.
//! 
//! ## Features
//! 
//! - **Ultra-low latency**: Optimized QUIC protocol implementation
//! - **Multi-protocol support**: JSON and Bincode serialization
//! - **Compression**: LZ4 and Zstd compression algorithms
//! - **Auto-reconnection**: Intelligent reconnection with exponential backoff
//! - **Event-driven**: Callback-based event handling
//! - **Performance monitoring**: Built-in statistics and metrics
//! 
//! ## Quick Start
//! 
//! ```rust,no_run
//! use fz_stream_client::{FastStreamClient, SerializationProtocol, CompressionLevel};
//! use std::time::Duration;
//! 
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut client = FastStreamClient::builder()
//!         .server_address("127.0.0.1:8080")
//!         .server_name("localhost")
//!         .auth_token("your_auth_token")
//!         .protocol(SerializationProtocol::Auto)
//!         .compression(CompressionLevel::LZ4Fast)
//!         .build()?;
//!     
//!     client.on_event(|event| {
//!         println!("Received event: {}", event.event_type);
//!     }).await;
//!     
//!     client.connect().await?;
//!     client.start_stream().await?;
//!     
//!     // Keep running...
//!     tokio::time::sleep(Duration::from_secs(60)).await;
//!     
//!     client.shutdown().await?;
//!     Ok(())
//! }
//! ```

mod config;
mod compression;
mod auth;
mod client;

// Re-export main types
pub use client::{
    FzStreamClient, 
    StreamClientBuilder, 
    StreamClientConfig,
    ConnectionStatus,
    ClientStats,
    EventHandler,
    StatusHandler,
};

pub use compression::{
    compress_data,
    decompress_data,
};

// Re-export common types
pub use fzstream_common::{
    EventMessage,
    EventType,
    EventMetadata,
    TransactionEvent,
    AuthMessage,
    AuthResponse,
    SerializationProtocol,
    CompressionLevel,
};