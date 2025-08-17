// Fast Stream Client SDK - Simple and easy-to-use QUIC client SDK with bincode support
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;
use quinn::{Endpoint, ClientConfig as QuinnClientConfig, Connection};
use rustls::{ClientConfig as RustlsClientConfig, client::danger};
use anyhow::Result;
use log::{info, warn, error, debug};
use serde::{Serialize, Deserialize};
use serde_json;
use base64::prelude::*;

use solana_streamer_sdk::streaming::event_parser::protocols::{
    bonk,
    pumpfun,
    pumpswap,
    raydium_amm_v4,
    raydium_clmm,
};
use solana_streamer_sdk::streaming::event_parser::core::UnifiedEvent;
use fzstream_common::{SerializationProtocol, CompressionLevel, CompressionType, EventMessage, EventType, EventMetadata, SolanaEventWrapper, TransactionEvent, AuthMessage, AuthResponse};
use crate::compression::decompress_data;

/// SDK version information
pub const SDK_VERSION: &str = "1.0.0";
pub const SDK_NAME: &str = "fz-stream-client";

/// Client connection status
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Authenticated,
    Streaming,
    Reconnecting,
    Error(String),
}

// 使用公共库中的类型定义

/// Configuration for the stream client
#[derive(Debug, Clone)]
pub struct StreamClientConfig {
    pub server_address: String,
    pub server_name: String,
    pub auth_token: Option<String>,
    pub protocol: SerializationProtocol,
    pub compression: CompressionLevel,
    pub auto_reconnect: bool,
    pub reconnect_interval: Duration,
    pub max_reconnect_attempts: u32,
    pub connection_timeout: Duration,
    pub keep_alive_interval: Duration,
}

impl Default for StreamClientConfig {
    fn default() -> Self {
        Self {
            server_address: "127.0.0.1:8080".to_string(),
            server_name: "localhost".to_string(),
            auth_token: None,
            protocol: SerializationProtocol::Auto,
            compression: CompressionLevel::LZ4Fast,
            auto_reconnect: true,
            reconnect_interval: Duration::from_secs(5),
            max_reconnect_attempts: 10,
            connection_timeout: Duration::from_secs(10),
            keep_alive_interval: Duration::from_secs(30),
        }
    }
}

/// Client statistics
#[derive(Debug, Clone, Default)]
pub struct ClientStats {
    pub events_received: u64,
    pub bytes_received: u64,
    pub connection_attempts: u32,
    pub successful_connections: u32,
    pub reconnection_count: u32,
    pub last_event_time: Option<Instant>,
    pub average_latency_us: Option<u64>,
    pub connection_uptime: Duration,
}

/// Event handler type
pub type EventHandler = Box<dyn Fn(TransactionEvent) + Send + Sync>;

/// Status change handler type  
pub type StatusHandler = Box<dyn Fn(ConnectionStatus) + Send + Sync>;

pub struct FzStreamClient {
    config: StreamClientConfig,
    endpoint: Option<Endpoint>,
    connection: Option<Connection>,
    status: Arc<RwLock<ConnectionStatus>>,
    event_handler: Arc<RwLock<Option<EventHandler>>>,
    status_handler: Arc<RwLock<Option<StatusHandler>>>,
    stats: Arc<RwLock<ClientStats>>,
    shutdown_tx: Option<broadcast::Sender<()>>,
}

impl FzStreamClient {
    /// Create a new client with default configuration
    pub fn new() -> Self {
        Self::with_config(StreamClientConfig::default())
    }

    /// Create a new client with custom configuration
    pub fn with_config(config: StreamClientConfig) -> Self {
        info!("📋 Configuration: server={}, protocol={:?}, compression={:?}", 
              config.server_address, config.protocol, config.compression);
        
        Self {
            config,
            endpoint: None,
            connection: None,
            status: Arc::new(RwLock::new(ConnectionStatus::Disconnected)),
            event_handler: Arc::new(RwLock::new(None)),
            status_handler: Arc::new(RwLock::new(None)),
            stats: Arc::new(RwLock::new(ClientStats::default())),
            shutdown_tx: None,
        }
    }

    /// Builder pattern for easy configuration
    pub fn builder() -> StreamClientBuilder {
        StreamClientBuilder::new()
    }

    /// Set event handler for processing received events
    pub async fn on_event<F>(&self, handler: F) 
    where
        F: Fn(TransactionEvent) + Send + Sync + 'static,
    {
        let mut event_handler = self.event_handler.write().await;
        *event_handler = Some(Box::new(handler));
        debug!("✅ Event handler set");
    }

    /// Set status change handler
    pub async fn on_status_change<F>(&self, handler: F)
    where 
        F: Fn(ConnectionStatus) + Send + Sync + 'static,
    {
        let mut status_handler = self.status_handler.write().await;
        *status_handler = Some(Box::new(handler));
        debug!("✅ Status change handler set");
    }


    /// Connect to the server
    pub async fn connect(&mut self) -> Result<()> {
        self.set_status(ConnectionStatus::Connecting).await;
        
        // Configure QUIC client
        let client_config = self.configure_quic_client()?;
        
        // Create endpoint
        let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
        endpoint.set_default_client_config(client_config);
        
        // Connect to server
        let server_addr = self.config.server_address.parse()?;
        let connection = tokio::time::timeout(
            self.config.connection_timeout,
            endpoint.connect(server_addr, &self.config.server_name)?
        ).await??;

        info!("✅ Connected to {}", self.config.server_address);
        
        self.endpoint = Some(endpoint);
        self.connection = Some(connection);
        self.set_status(ConnectionStatus::Connected).await;
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.connection_attempts += 1;
            stats.successful_connections += 1;
        }

        Ok(())
    }

    /// Authenticate with the server if auth token is provided
    pub async fn authenticate(&self) -> Result<()> {
        if let Some(auth_token) = &self.config.auth_token {
            if let Some(connection) = &self.connection {
                info!("🔐 Authenticating with server...");
                
                let auth_msg = AuthMessage {
                    auth_token: auth_token.clone(),
                    client_id: format!("client_{}", Uuid::new_v4()),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                };
                
                let (mut send_stream, mut recv_stream) = connection.open_bi().await?;
                
                // Send auth message
                let auth_data = serde_json::to_vec(&auth_msg)?;
                send_stream.write_all(&auth_data).await?;
                send_stream.finish()?;
                
                // Read response
                let mut response_data = Vec::new();
                let mut buffer = [0u8; 1024];
                
                loop {
                    match recv_stream.read(&mut buffer).await? {
                        Some(n) => response_data.extend_from_slice(&buffer[..n]),
                        None => break,
                    }
                }
                
                let auth_response: AuthResponse = serde_json::from_slice(&response_data)?;
                
                match auth_response {
                    AuthResponse::Success { message, client_id, permissions } => {
                        info!("✅ Authentication successful: {} (client_id: {}, permissions: {:?})", message, client_id, permissions);
                        self.set_status(ConnectionStatus::Authenticated).await;
                    },
                    AuthResponse::Failure { error, code } => {
                        let error_msg = format!("Authentication failed: {} (code: {})", error, code);
                        error!("{}", error_msg);
                        self.set_status(ConnectionStatus::Error(error_msg.clone())).await;
                        return Err(anyhow::anyhow!(error_msg));
                    }
                }
            }
        } else {
            // No auth token provided, proceed without authentication
            self.set_status(ConnectionStatus::Authenticated).await;
        }
        
        Ok(())
    }

    /// Start receiving events
    pub async fn start_stream(&mut self) -> Result<()> {
        if self.connection.is_none() {
            return Err(anyhow::anyhow!("Not connected. Call connect() first."));
        }

        // Authenticate if needed
        self.authenticate().await?;
        
        self.set_status(ConnectionStatus::Streaming).await;
        info!("🚀 Starting event stream...");
        
        let connection = self.connection.as_ref().unwrap().clone();
        let event_handler = Arc::clone(&self.event_handler);
        let stats = Arc::clone(&self.stats);
        let protocol = self.config.protocol.clone();
        let compression = self.config.compression.clone();
        
        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx);
        
        // Start receiving events
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("📡 Event stream shutdown requested");
                        break;
                    }
                    stream_result = connection.accept_uni() => {
                        match stream_result {
                            Ok(mut recv_stream) => {
                                let mut buffer = Vec::new();
                                let mut chunk = [0u8; 4096];
                                
                                // Read all data from stream
                                loop {
                                    match recv_stream.read(&mut chunk).await {
                                        Ok(Some(n)) => {
                                            buffer.extend_from_slice(&chunk[..n]);
                                        }
                                        Ok(None) => break, // Stream ended
                                        Err(e) => {
                                            error!("❌ Error reading from stream: {}", e);
                                            break;
                                        }
                                    }
                                }
                                
                                if !buffer.is_empty() {
                                    // Try to process the event
                                    if let Ok(event) = Self::parse_event_data(&buffer, &protocol, &compression) {
                                        // Update stats
                                        {
                                            let mut stats_guard = stats.write().await;
                                            stats_guard.events_received += 1;
                                            stats_guard.bytes_received += buffer.len() as u64;
                                            stats_guard.last_event_time = Some(Instant::now());
                                        }
                                        
                                        
                                        // Call event handler
                                        if let Some(handler) = event_handler.read().await.as_ref() {
                                            handler(event);
                                        }
                                    } else {
                                        debug!("⚠️ Failed to parse event data, {} bytes received", buffer.len());
                                    }
                                }
                            }
                            Err(e) => {
                                error!("❌ Error accepting stream: {}", e);
                                break;
                            }
                        }
                    }
                }
            }
        });
        
        Ok(())
    }

    /// Subscribe to events with a callback function that receives UnifiedEvent
    pub async fn subscribe_events<F>(
        &mut self,
        callback: F,
    ) -> Result<()>
    where
        F: Fn(Box<dyn UnifiedEvent>) + Send + Sync + 'static,
    {
        if self.connection.is_none() {
            return Err(anyhow::anyhow!("Not connected. Call connect() first."));
        }

        // Authenticate if needed
        self.authenticate().await?;
        
        self.set_status(ConnectionStatus::Streaming).await;
        info!("🚀 Starting event stream with UnifiedEvent callback...");
        
        let connection = self.connection.as_ref().unwrap().clone();
        let stats = Arc::clone(&self.stats);
        let protocol = self.config.protocol.clone();
        let compression = self.config.compression.clone();
        
        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx);
        
        // Start receiving events
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("📡 Event stream shutdown requested");
                        break;
                    }
                    stream_result = connection.accept_uni() => {
                        match stream_result {
                            Ok(mut recv_stream) => {
                                let mut buffer = Vec::new();
                                let mut chunk = [0u8; 4096];
                                
                                // Read all data from stream
                                loop {
                                    match recv_stream.read(&mut chunk).await {
                                        Ok(Some(n)) => {
                                            buffer.extend_from_slice(&chunk[..n]);
                                        }
                                        Ok(None) => break, // Stream ended
                                        Err(e) => {
                                            error!("❌ Error reading from stream: {}", e);
                                            break;
                                        }
                                    }
                                }
                                
                                if !buffer.is_empty() {
                                    info!("📥 Received {} bytes from QUIC server", buffer.len());
                                    // Try to process the event and call callback with UnifiedEvent
                                    if let Ok(unified_event) = Self::parse_event_data_as_unified(&buffer, &protocol, &compression) {
                                        // Update stats
                                        {
                                            let mut stats_guard = stats.write().await;
                                            stats_guard.events_received += 1;
                                            stats_guard.bytes_received += buffer.len() as u64;
                                            stats_guard.last_event_time = Some(Instant::now());
                                        }
                                        
                                        // Call the callback with UnifiedEvent
                                        callback(unified_event);
                                    } else {
                                        error!("⚠️ Failed to parse event data as UnifiedEvent, {} bytes received", buffer.len());
                                    }
                                }
                            }
                            Err(e) => {
                                error!("❌ Error accepting stream: {}", e);
                                break;
                            }
                        }
                    }
                }
            }
        });
        
        Ok(())
    }

    /// Parse event data based on protocol and compression
    fn parse_event_data(
        raw_data: &[u8], 
        protocol: &SerializationProtocol,
        compression: &CompressionLevel
    ) -> Result<TransactionEvent> {
        // Handle compression if enabled
        let decompressed_data = if matches!(compression, CompressionLevel::None) {
            raw_data.to_vec()
        } else {
            let compression_type = match compression {
                CompressionLevel::LZ4Fast | CompressionLevel::LZ4High => CompressionType::LZ4,
                CompressionLevel::ZstdFast | CompressionLevel::ZstdMedium 
                | CompressionLevel::ZstdHigh | CompressionLevel::ZstdMax => CompressionType::Zstd,
                CompressionLevel::None => CompressionType::None,
            };
            decompress_data(raw_data, &compression_type)?
        };

        // Parse according to protocol
        match protocol {
            SerializationProtocol::JSON => {
                Self::parse_json_event(&decompressed_data)
            }
            SerializationProtocol::Bincode => {
                Self::parse_bincode_event(&decompressed_data)
            }
            SerializationProtocol::Auto => {
                // Try bincode first, fallback to JSON
                Self::parse_bincode_event(&decompressed_data)
                    .or_else(|_| Self::parse_json_event(&decompressed_data))
            }
        }
    }

    /// Parse event data as UnifiedEvent based on protocol and compression
    fn parse_event_data_as_unified(
        raw_data: &[u8], 
        protocol: &SerializationProtocol,
        compression: &CompressionLevel
    ) -> Result<Box<dyn UnifiedEvent>> {
        // Handle compression if enabled
        let decompressed_data = if matches!(compression, CompressionLevel::None) {
            raw_data.to_vec()
        } else {
            let compression_type = match compression {
                CompressionLevel::LZ4Fast | CompressionLevel::LZ4High => CompressionType::LZ4,
                CompressionLevel::ZstdFast | CompressionLevel::ZstdMedium 
                | CompressionLevel::ZstdHigh | CompressionLevel::ZstdMax => CompressionType::Zstd,
                CompressionLevel::None => CompressionType::None,
            };
            decompress_data(raw_data, &compression_type)?
        };

        // Parse according to protocol
        match protocol {
            SerializationProtocol::JSON => {
                // For JSON, we need to convert to UnifiedEvent
                let event: TransactionEvent = serde_json::from_slice(&decompressed_data)?;
                // Convert TransactionEvent to UnifiedEvent (this is a simplified approach)
                Err(anyhow::anyhow!("JSON events not supported for UnifiedEvent callback"))
            }
            SerializationProtocol::Bincode => {
                Self::parse_bincode_event_as_unified(&decompressed_data)
            }
            SerializationProtocol::Auto => {
                // Try bincode first, fallback to JSON
                Self::parse_bincode_event_as_unified(&decompressed_data)
                    .or_else(|_| Err(anyhow::anyhow!("Auto fallback to JSON not supported for UnifiedEvent")))
            }
        }
    }

    /// Parse JSON event
    fn parse_json_event(data: &[u8]) -> Result<TransactionEvent> {
        let event: TransactionEvent = serde_json::from_slice(data)?;
        Ok(event)
    }

    /// Parse bincode event as UnifiedEvent - deserialize EventMessage and return UnifiedEvent
    fn parse_bincode_event_as_unified(data: &[u8]) -> Result<Box<dyn UnifiedEvent>> {
        // Deserialize the EventMessage directly
        let event_message: EventMessage = bincode::deserialize(data)?;
        
        info!("📥 Received EventMessage: type={:?}, id={}, data_size={} bytes", 
              event_message.event_type, event_message.event_id, event_message.data.len());
        
        // Deserialize the actual Solana event from the data field and return as UnifiedEvent
        Self::deserialize_solana_event_as_unified(&event_message.data, &event_message.event_type)
    }

    /// Parse bincode event - deserialize EventMessage and parse the event data
    fn parse_bincode_event(data: &[u8]) -> Result<TransactionEvent> {
        // Deserialize the EventMessage directly
        let event_message: EventMessage = bincode::deserialize(data)?;
        
        // Deserialize the actual Solana event from the data field
        let solana_event = Self::deserialize_solana_event(&event_message.data, &event_message.event_type)?;
        
        Ok(TransactionEvent {
            event_id: event_message.event_id,
            event_type: event_message.event_type,
            timestamp: event_message.timestamp,
            data: serde_json::to_value(solana_event)?,
            metadata: Some(EventMetadata {
                block_time: None,
                slot: None,
                signature: None,
                source: None,
                priority: None,
                additional_fields: std::collections::HashMap::new(),
            }),
        })
    }

    /// Deserialize Solana event from bincode data as UnifiedEvent
    fn deserialize_solana_event_as_unified(data: &[u8], event_type: &EventType) -> Result<Box<dyn UnifiedEvent>> {
        info!("🔍 Attempting to deserialize {:?} event with {} bytes", event_type, data.len());
        
        // Try to deserialize as the specific Solana event type
        match event_type {
            EventType::BlockMeta => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::BlockMetaEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::BonkPoolCreate => {
                if let Ok(event) = bincode::deserialize::<bonk::BonkPoolCreateEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::BonkTrade => {
                if let Ok(event) = bincode::deserialize::<bonk::BonkTradeEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::BonkMigrateToAmm => {
                if let Ok(event) = bincode::deserialize::<bonk::BonkMigrateToAmmEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::BonkMigrateToCpswap => {
                if let Ok(event) = bincode::deserialize::<bonk::BonkMigrateToCpswapEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpFunTrade => {
                if let Ok(event) = bincode::deserialize::<pumpfun::PumpFunTradeEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpFunMigrate => {
                if let Ok(event) = bincode::deserialize::<pumpfun::PumpFunMigrateEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpFunCreate => {
                if let Ok(event) = bincode::deserialize::<pumpfun::PumpFunCreateTokenEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapBuy => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapBuyEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapSell => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapSellEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapCreate => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapCreatePoolEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapDeposit => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapDepositEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapWithdraw => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapWithdrawEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumCpmmSwap => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_cpmm::RaydiumCpmmSwapEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumCpmmDeposit => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_cpmm::RaydiumCpmmDepositEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumCpmmInitialize => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_cpmm::RaydiumCpmmInitializeEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumCpmmWithdraw => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_cpmm::RaydiumCpmmWithdrawEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmSwap => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmSwapEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmSwapV2 => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmSwapV2Event>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmClosePosition => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmClosePositionEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmDecreaseLiquidityV2 => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmDecreaseLiquidityV2Event>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmCreatePool => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmCreatePoolEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmIncreaseLiquidityV2 => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmIncreaseLiquidityV2Event>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmOpenPositionWithToken22Nft => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmOpenPositionWithToken22NftEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmOpenPositionV2 => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmOpenPositionV2Event>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumAmmV4Swap => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_amm_v4::RaydiumAmmV4SwapEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumAmmV4Deposit => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_amm_v4::RaydiumAmmV4DepositEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumAmmV4Initialize => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_amm_v4::RaydiumAmmV4Initialize2Event>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumAmmV4Withdraw => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_amm_v4::RaydiumAmmV4WithdrawEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumAmmV4WithdrawPnl => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_amm_v4::RaydiumAmmV4WithdrawPnlEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::BonkPoolStateAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::bonk::BonkPoolStateAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::BonkGlobalConfigAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::bonk::BonkGlobalConfigAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::BonkPlatformConfigAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::bonk::BonkPlatformConfigAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapGlobalConfigAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapGlobalConfigAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapPoolAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapPoolAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpFunBondingCurveAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpfun::PumpFunBondingCurveAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpFunGlobalAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpfun::PumpFunGlobalAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumAmmV4InfoAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_amm_v4::RaydiumAmmV4AmmInfoAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmConfigAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmAmmConfigAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmPoolStateAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmPoolStateAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmTickArrayAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmTickArrayStateAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumCpmmConfigAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_cpmm::RaydiumCpmmAmmConfigAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumCpmmPoolStateAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_cpmm::RaydiumCpmmPoolStateAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::Custom(custom_type) => {
                // For custom events, we can't deserialize to a specific type
                // Return an error or handle as needed
                return Err(anyhow::anyhow!("Custom event type '{}' not supported for UnifiedEvent deserialization", custom_type));
            },
        }
        
        Err(anyhow::anyhow!("Failed to deserialize event for type: {:?}", event_type))
    }

    /// Deserialize Solana event from bincode data
    fn deserialize_solana_event(data: &[u8], event_type: &EventType) -> Result<serde_json::Value> {
        // Try to deserialize as the specific Solana event type
        match event_type {
            EventType::BlockMeta => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::BlockMetaEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::BonkPoolCreate => {
                if let Ok(event) = bincode::deserialize::<bonk::BonkPoolCreateEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::BonkTrade => {
                if let Ok(event) = bincode::deserialize::<bonk::BonkTradeEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::BonkMigrateToAmm => {
                if let Ok(event) = bincode::deserialize::<bonk::BonkMigrateToAmmEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::BonkMigrateToCpswap => {
                if let Ok(event) = bincode::deserialize::<bonk::BonkMigrateToCpswapEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::PumpFunTrade => {
                if let Ok(event) = bincode::deserialize::<pumpfun::PumpFunTradeEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::PumpFunMigrate => {
                if let Ok(event) = bincode::deserialize::<pumpfun::PumpFunMigrateEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::PumpFunCreate => {
                if let Ok(event) = bincode::deserialize::<pumpfun::PumpFunCreateTokenEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::PumpSwapBuy => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapBuyEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::PumpSwapSell => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapSellEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::PumpSwapCreate => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapCreatePoolEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::PumpSwapDeposit => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapDepositEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::PumpSwapWithdraw => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapWithdrawEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumCpmmSwap => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_cpmm::RaydiumCpmmSwapEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumCpmmDeposit => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_cpmm::RaydiumCpmmDepositEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumCpmmInitialize => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_cpmm::RaydiumCpmmInitializeEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumCpmmWithdraw => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_cpmm::RaydiumCpmmWithdrawEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumClmmSwap => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmSwapEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumClmmSwapV2 => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmSwapV2Event>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumClmmClosePosition => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmClosePositionEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumClmmDecreaseLiquidityV2 => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmDecreaseLiquidityV2Event>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumClmmCreatePool => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmCreatePoolEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumClmmIncreaseLiquidityV2 => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmIncreaseLiquidityV2Event>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumClmmOpenPositionWithToken22Nft => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmOpenPositionWithToken22NftEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumClmmOpenPositionV2 => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmOpenPositionV2Event>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumAmmV4Swap => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_amm_v4::RaydiumAmmV4SwapEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumAmmV4Deposit => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_amm_v4::RaydiumAmmV4DepositEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumAmmV4Initialize => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_amm_v4::RaydiumAmmV4Initialize2Event>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumAmmV4Withdraw => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_amm_v4::RaydiumAmmV4WithdrawEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumAmmV4WithdrawPnl => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_amm_v4::RaydiumAmmV4WithdrawPnlEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            // Account events
            EventType::BonkPoolStateAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::bonk::BonkPoolStateAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::BonkGlobalConfigAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::bonk::BonkGlobalConfigAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::BonkPlatformConfigAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::bonk::BonkPlatformConfigAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::PumpSwapGlobalConfigAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapGlobalConfigAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::PumpSwapPoolAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::PumpSwapPoolAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::PumpFunBondingCurveAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpfun::PumpFunBondingCurveAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::PumpFunGlobalAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::pumpfun::PumpFunGlobalAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumAmmV4InfoAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_amm_v4::RaydiumAmmV4AmmInfoAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumClmmConfigAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmAmmConfigAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumClmmPoolStateAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmPoolStateAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumClmmTickArrayAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_clmm::RaydiumClmmTickArrayStateAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumCpmmConfigAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_cpmm::RaydiumCpmmAmmConfigAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            EventType::RaydiumCpmmPoolStateAccount => {
                if let Ok(event) = bincode::deserialize::<solana_streamer_sdk::streaming::event_parser::protocols::raydium_cpmm::RaydiumCpmmPoolStateAccountEvent>(data) {
                    return Ok(serde_json::to_value(event)?);
                }
            },
            _ => {
                // If we can't match the specific type, return the raw data
                let event_data_str = String::from_utf8_lossy(data);
                return Ok(serde_json::json!({
                    "raw_data": event_data_str,
                    "event_type": event_type,
                    "serialization": "bincode"
                }));
            }
        }
        
        // If deserialization failed, return raw data
        let event_data_str = String::from_utf8_lossy(data);
        Ok(serde_json::json!({
            "raw_data": event_data_str,
            "event_type": event_type,
            "serialization": "bincode"
        }))
    }

    /// Get current connection status
    pub async fn get_status(&self) -> ConnectionStatus {
        self.status.read().await.clone()
    }

    /// Get client statistics
    pub async fn get_stats(&self) -> ClientStats {
        self.stats.read().await.clone()
    }

    /// Shutdown the client
    pub async fn shutdown(&mut self) -> Result<()> {
        info!("🔌 Shutting down Fast Stream Client...");
        
        if let Some(shutdown_tx) = &self.shutdown_tx {
            let _ = shutdown_tx.send(());
        }
        
        if let Some(connection) = &self.connection {
            connection.close(quinn::VarInt::from_u32(0), b"Client shutdown");
        }
        
        self.connection = None;
        self.endpoint = None;
        self.shutdown_tx = None;
        
        self.set_status(ConnectionStatus::Disconnected).await;
        info!("✅ Client shutdown complete");
        
        Ok(())
    }

    /// Configure QUIC client
    fn configure_quic_client(&self) -> Result<QuinnClientConfig> {
        // For now, use a simple configuration - in production you should use proper certificates
        let mut crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertificateVerification {}))
            .with_no_client_auth();

        // Set ALPN protocols to match server
        crypto.alpn_protocols = vec![b"h3".to_vec()];

        let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(crypto))?;
        let client_config = QuinnClientConfig::new(Arc::new(quic_config));
        
        Ok(client_config)
    }
}

/// Simple certificate verifier that accepts all certificates (for testing only)
#[derive(Debug)]
struct NoCertificateVerification;

impl danger::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<danger::ServerCertVerified, rustls::Error> {
        Ok(danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<danger::HandshakeSignatureValid, rustls::Error> {
        Ok(danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<danger::HandshakeSignatureValid, rustls::Error> {
        Ok(danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA1,
            rustls::SignatureScheme::ECDSA_SHA1_Legacy,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}

impl FzStreamClient {
    /// Set connection status
    async fn set_status(&self, status: ConnectionStatus) {
        {
            let mut current_status = self.status.write().await;
            *current_status = status.clone();
        }
        
        // Call status handler if set
        if let Some(handler) = self.status_handler.read().await.as_ref() {
            handler(status);
        }
    }
}

/// Builder for StreamClient configuration
pub struct StreamClientBuilder {
    config: StreamClientConfig,
}

impl StreamClientBuilder {
    pub fn new() -> Self {
        Self {
            config: StreamClientConfig::default(),
        }
    }

    pub fn server_address(mut self, address: &str) -> Self {
        self.config.server_address = address.to_string();
        self
    }

    pub fn server_name(mut self, name: &str) -> Self {
        self.config.server_name = name.to_string();
        self
    }

    pub fn auth_token(mut self, token: &str) -> Self {
        self.config.auth_token = Some(token.to_string());
        self
    }

    pub fn protocol(mut self, protocol: SerializationProtocol) -> Self {
        self.config.protocol = protocol;
        self
    }

    pub fn compression(mut self, compression: CompressionLevel) -> Self {
        self.config.compression = compression;
        self
    }

    pub fn auto_reconnect(mut self, enabled: bool) -> Self {
        self.config.auto_reconnect = enabled;
        self
    }

    pub fn connection_timeout(mut self, timeout: Duration) -> Self {
        self.config.connection_timeout = timeout;
        self
    }

    pub fn build(self) -> Result<FzStreamClient> {
        Ok(FzStreamClient::with_config(self.config))
    }
}

