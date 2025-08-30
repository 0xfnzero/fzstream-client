# FZStream Client SDK

[![Crates.io](https://img.shields.io/crates/v/fzstream-client)](https://crates.io/crates/fzstream-client)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://github.com/0xfnzero/fz-stream-client/workflows/Rust/badge.svg)](https://github.com/0xfnzero/fz-stream-client/actions)

一个高性能的 QUIC 客户端 SDK，专为实时 Solana 事件流设计。

## ✨ 特性

- **超低延迟**: 优化的 QUIC 协议实现
- **多协议支持**: JSON 和 Bincode 序列化
- **压缩算法**: LZ4 和 Zstd 压缩支持
- **自动重连**: 智能重连机制，支持指数退避
- **事件驱动**: 基于回调的事件处理
- **性能监控**: 内置统计和指标
- **多协议事件**: 支持 PumpFun、PumpSwap、Bonk、Raydium 等协议

## 🚀 快速开始

### 安装

在 `Cargo.toml` 中添加依赖：

```toml
[dependencies]
fzstream-client = {"0.1.0"}
```

### 基本使用

```rust
use anyhow::Result;
use fzstream_client::{FzStreamClient, StreamClientConfig};

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    env_logger::init();
    
    // 配置客户端
    let mut config = StreamClientConfig::default();
    config.endpoint = "127.0.0.1:2222".to_string();
    config.auth_token = Some("demo_token_12345".to_string());
    
    let mut client = FzStreamClient::with_config(config);
    
    // 连接到服务器
    client.connect().await?;
    
    // 订阅事件
    client.subscribe_events(|unified_event| {
        let event_type = unified_event.event_type();
        let event_id = unified_event.id();
        println!("收到事件 [{}]: {:?}", event_id, event_type);
    }).await?;
    
    // 保持运行直到 Ctrl+C
    tokio::signal::ctrl_c().await?;
    
    // 关闭连接
    client.shutdown().await?;
    Ok(())
}
```

## 📖 高级用法

### 详细事件处理

```rust
use solana_streamer_sdk::streaming::event_parser::protocols::{bonk, pumpfun, pumpswap};

// 使用 match_event 宏处理特定事件类型
macro_rules! match_event {
    ($event:expr, {
        $($event_type:ident => |$e:ident: $event_struct:ty| $body:block,)*
        _ => $default:block
    }) => {
        $(
            if let Some($e) = $event.as_any().downcast_ref::<$event_struct>() {
                $body
                return;
            }
        )*
        $default
    };
}

client.subscribe_events(|unified_event| {
    match_event!(unified_event, {
        PumpFunTradeEvent => |e: pumpfun::PumpFunTradeEvent| {
            println!("PumpFun 交易: {} at {}", e.metadata.signature, e.metadata.block_time);
        },
        BonkTradeEvent => |e: bonk::BonkTradeEvent| {
            println!("Bonk 交易: {}", e.metadata.signature);
        },
        _ => {
            println!("其他事件类型: {:?}", unified_event.event_type());
        }
    });
}).await?;
```

### 连接状态监控

```rust
use fzstream_client::ConnectionStatus;

// 监控连接状态变化
client.on_status_change(|status| {
    match status {
        ConnectionStatus::Connected => println!("已连接"),
        ConnectionStatus::Streaming => println!("开始流式传输"),
        ConnectionStatus::Error(msg) => println!("连接错误: {}", msg),
        _ => {}
    }
});
```

## ⚙️ 配置选项

### StreamClientConfig

```rust
use std::time::Duration;
use fzstream_client::StreamClientConfig;

let config = StreamClientConfig {
    endpoint: "127.0.0.1:2222".to_string(),
    server_name: "localhost".to_string(),
    auth_token: Some("your_auth_token".to_string()),
    auto_reconnect: true,
    reconnect_interval: Duration::from_secs(5),
    max_reconnect_attempts: 10,
    connection_timeout: Duration::from_secs(10),
    keep_alive_interval: Duration::from_secs(30),
    reconnect_backoff_multiplier: 1.5,
    max_reconnect_backoff: Duration::from_secs(60),
};
```

| 配置项 | 类型 | 默认值 | 描述 |
|--------|------|--------|------|
| `endpoint` | `String` | `"127.0.0.1:2222"` | 服务器地址 |
| `server_name` | `String` | `"localhost"` | 服务器名称 |
| `auth_token` | `Option<String>` | `None` | 认证令牌 |
| `auto_reconnect` | `bool` | `true` | 是否自动重连 |
| `reconnect_interval` | `Duration` | `5s` | 重连间隔 |
| `max_reconnect_attempts` | `u32` | `10` | 最大重连次数 |
| `connection_timeout` | `Duration` | `10s` | 连接超时 |
| `keep_alive_interval` | `Duration` | `30s` | 保活间隔 |
| `reconnect_backoff_multiplier` | `f64` | `1.5` | 重连退避倍数 |
| `max_reconnect_backoff` | `Duration` | `60s` | 最大重连退避时间 |

## 🔧 API 参考

### FzStreamClient

#### 主要方法

- `connect()` - 连接到服务器
- `subscribe_events(handler)` - 订阅事件流
- `on_status_change(handler)` - 监控连接状态
- `shutdown()` - 关闭连接

### 支持的事件类型

- **PumpFun 协议**:
  - `PumpFunTradeEvent` - 交易事件
  - `PumpFunCreateTokenEvent` - 创建代币事件
  - `PumpFunMigrateEvent` - 迁移事件

- **PumpSwap 协议**:
  - `PumpSwapBuyEvent` - 买入事件
  - `PumpSwapSellEvent` - 卖出事件
  - `PumpSwapCreatePoolEvent` - 创建池事件

- **Bonk 协议**:
  - `BonkTradeEvent` - 交易事件
  - `BonkPoolCreateEvent` - 池创建事件

- **Raydium 协议**:
  - `RaydiumCpmmSwapEvent` - CPMM 交换事件
  - `RaydiumAmmV4SwapEvent` - AMM V4 交换事件
  - `RaydiumClmmSwapEvent` - CLMM 交换事件

- **区块元数据**:
  - `BlockMetaEvent` - 区块元数据事件

## 🚀 性能特性

- **超低延迟**: 基于 QUIC 协议，延迟通常在毫秒级别
- **高吞吐量**: 支持大量并发连接和事件处理
- **自动重连**: 网络中断时自动重连，支持指数退避
- **压缩支持**: 内置 LZ4 和 Zstd 压缩，减少网络传输
- **内存优化**: 高效的内存管理和垃圾回收

## 🔍 故障排除

### 常见问题

1. **连接失败**
   ```bash
   # 检查服务器是否运行
   cargo run --release
   
   # 检查端口是否正确
   netstat -an | grep 2222
   ```

2. **认证失败**
   ```rust
   // 确保使用正确的认证令牌
   config.auth_token = Some("your_correct_token".to_string());
   ```

3. **事件接收问题**
   ```rust
   // 启用详细日志
   std::env::set_var("RUST_LOG", "debug");
   env_logger::init();
   ```

### 日志级别

- `error`: 错误信息
- `warn`: 警告信息  
- `info`: 一般信息
- `debug`: 调试信息
- `trace`: 详细跟踪信息

## 📄 许可证

本项目采用 MIT 许可证 - 查看 [LICENSE](LICENSE) 文件了解详情。

## 🤝 贡献

欢迎提交 Issue 和 Pull Request！

## 📞 支持

- GitHub Issues: [https://github.com/0xfnzero/fz-stream-client/issues](https://github.com/0xfnzero/fz-stream-client/issues)
- 邮箱: byteblock6@gmail.com
