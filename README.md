# FZ Stream Client SDK

A high-performance QUIC client SDK for real-time Solana event streaming with protobuf support.

## Features

- **Ultra-low latency**: Optimized QUIC protocol implementation
- **Multi-protocol support**: JSON, Protobuf, and automatic protocol selection
- **Compression**: LZ4 and Zstd compression algorithms for bandwidth optimization
- **Auto-reconnection**: Intelligent reconnection with exponential backoff
- **Event-driven architecture**: Callback-based event handling
- **Performance monitoring**: Built-in statistics and metrics collection
- **Flexible configuration**: Multiple performance profiles and custom settings

## Quick Start

Add this to your `Cargo.toml`:

```toml
[dependencies]
fz-stream-client = "0.1.0"
```

### Basic Usage

```rust
use fz_stream_client::{FastStreamClient, SerializationProtocol, CompressionLevel};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    
    let mut client = FastStreamClient::builder()
        .server_address("127.0.0.1:8080")
        .server_name("localhost")
        .auth_token("your_auth_token")
        .protocol(SerializationProtocol::Auto)
        .compression(CompressionLevel::LZ4Fast)
        .build()?;
    
    client.on_event(|event| {
        println!("Received event: {} - {}", event.event_type, event.event_id);
    }).await;
    
    client.on_status_change(|status| {
        println!("Status changed: {:?}", status);
    }).await;
    
    client.connect().await?;
    client.start_stream().await?;
    
    // Keep running...
    tokio::time::sleep(Duration::from_secs(60)).await;
    
    client.shutdown().await?;
    Ok(())
}
```

## Performance Profiles

The SDK includes several predefined performance profiles:

### Ultra Low Latency
- **Protocol**: JSON
- **Compression**: None
- **Use case**: Real-time trading signals, high-frequency updates

### Bandwidth Optimized
- **Protocol**: Protobuf
- **Compression**: Zstd High
- **Use case**: Mobile networks, large data transfers

### Balanced Mode
- **Protocol**: Auto (adaptive)
- **Compression**: LZ4 Fast
- **Use case**: General applications, development

### High Throughput
- **Protocol**: Protobuf
- **Compression**: LZ4 Fast
- **Use case**: Batch processing, log aggregation

## Advanced Configuration

```rust
use fz_stream_client::{FastStreamClient, StreamClientConfig, SerializationProtocol, CompressionLevel};
use std::time::Duration;

let config = StreamClientConfig {
    server_address: "127.0.0.1:8080".to_string(),
    server_name: "localhost".to_string(),
    auth_token: Some("your_token".to_string()),
    client_id: "my_client".to_string(),
    protocol: SerializationProtocol::Auto,
    compression: CompressionLevel::LZ4Fast,
    auto_reconnect: true,
    reconnect_interval: Duration::from_secs(5),
    max_reconnect_attempts: 10,
    connect_timeout: Duration::from_secs(10),
    heartbeat_interval: Duration::from_secs(30),
    buffer_size: 2048,
};

let mut client = FastStreamClient::new(config)?;
```

## Event Types

The SDK handles various Solana event types:

- **PumpFun Events**: Trade and token creation events
- **Raydium Events**: CLMM and CPMM swap events
- **Bonk Events**: Trade and pool creation events
- **PumpSwap Events**: Buy, sell, deposit, withdraw events

Each event includes:
- Event ID and type
- Timestamp
- Event data (JSON)
- Optional metadata (block time, slot, signature, etc.)

## Error Handling and Reconnection

The SDK automatically handles:
- Connection failures
- Network interruptions
- Authentication errors
- Protocol errors

Features:
- Exponential backoff for reconnection attempts
- Configurable retry limits
- Status change notifications
- Connection health monitoring

## Performance Monitoring

Built-in statistics tracking:

```rust
let stats = client.get_stats().await;
println!("Events received: {}", stats.total_events_received);
println!("Uptime: {:?}", stats.connection_uptime);
println!("Reconnections: {}", stats.reconnections_count);
```

## Examples

See the `examples/` directory for more detailed usage:

- `basic_usage.rs` - Simple client setup and event handling
- `advanced_usage.rs` - Multiple clients with statistics and monitoring
- `multi_client.rs` - Multiple clients with different configurations

Run examples:

```bash
cargo run --example basic_usage
cargo run --example advanced_usage
cargo run --example multi_client
```

## Requirements

- Rust 1.70+
- Tokio async runtime
- Network access to the QUIC server

## License

MIT License - see LICENSE file for details.

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.