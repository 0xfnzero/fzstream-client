use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;
use quinn::{Endpoint, ClientConfig as QuinnClientConfig, Connection};
use rustls::client::danger;
use anyhow::Result;
use log::{info, warn, error, debug};
use serde_json;

use solana_streamer_sdk::streaming::event_parser::protocols::{
    bonk, pumpfun, pumpswap, raydium_amm_v4, raydium_clmm, raydium_cpmm, BlockMetaEvent
};
use solana_streamer_sdk::streaming::event_parser::core::UnifiedEvent;
use fzstream_common::{SerializationProtocol, CompressionLevel, EventMessage, EventType, EventMetadata, TransactionEvent, AuthMessage, AuthResponse};

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
        info!("📋 Configuration: server={}", config.server_address);
        
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
        info!("🔗 Starting connection to server: {}", self.config.server_address);
        self.set_status(ConnectionStatus::Connecting).await;
        
        // Configure QUIC client
        info!("⚙️ Configuring QUIC client...");
        let client_config = self.configure_quic_client()?;
        info!("✅ QUIC client configuration created");
        
        // Create endpoint
        info!("🔌 Creating QUIC endpoint...");
        let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
        endpoint.set_default_client_config(client_config);
        info!("✅ QUIC endpoint created");
        
        // Connect to server
        let server_addr = self.config.server_address.parse()?;
        info!("🚀 Attempting connection to {}...", server_addr);
        
        let connection = tokio::time::timeout(
            self.config.connection_timeout,
            endpoint.connect(server_addr, &self.config.server_name)?
        ).await??;

        info!("✅ Successfully connected to {}", self.config.server_address);
        info!("🔗 Connection details: {:?}", connection.stats());
        
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
        
        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx);
        
        // Start receiving events with comprehensive logging for UnifiedEvent callbacks
        tokio::spawn(async move {
            info!("🔄 Starting UnifiedEvent receive loop...");
            let mut stream_counter = 0;
            
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("📡 UnifiedEvent stream shutdown requested");
                        break;
                    }
                    stream_result = connection.accept_uni() => {
                        stream_counter += 1;
                        info!("📥 Incoming UnifiedEvent stream #{}: {:?}", stream_counter, stream_result);
                        
                        match stream_result {
                            Ok(mut recv_stream) => {
                                info!("✅ UnifiedEvent stream #{} accepted successfully", stream_counter);
                                let mut buffer = Vec::new();
                                let mut chunk = [0u8; 4096];
                                let mut total_bytes_read = 0;
                                let mut read_iterations = 0;
                                
                                // Read all data from stream
                                loop {
                                    read_iterations += 1;
                                    debug!("🔍 UnifiedEvent reading iteration #{} from stream #{}", read_iterations, stream_counter);
                                    
                                    match recv_stream.read(&mut chunk).await {
                                        Ok(Some(n)) => {
                                            total_bytes_read += n;
                                            buffer.extend_from_slice(&chunk[..n]);
                                            info!("📦 UnifiedEvent read {} bytes in iteration #{} (total: {} bytes)", n, read_iterations, total_bytes_read);
                                        }
                                        Ok(None) => {
                                            info!("🏁 UnifiedEvent stream #{} ended after {} iterations, total bytes: {}", stream_counter, read_iterations, total_bytes_read);
                                            break; // Stream ended
                            }
                            Err(e) => {
                                            error!("❌ Error reading from UnifiedEvent stream #{}: {}", stream_counter, e);
                                break;
                            }
                        }
                    }
                                
                                if !buffer.is_empty() {
                                    info!("🎯 Processing {} bytes from UnifiedEvent stream #{}", buffer.len(), stream_counter);
                                    
                                    // Log buffer content for debugging
                                    debug!("📝 UnifiedEvent buffer content (first 100 bytes): {:?}", &buffer[..std::cmp::min(100, buffer.len())]);
                                    
                                    // Try to process the event and call callback with UnifiedEvent
                                    match Self::parse_event_data_as_unified(&buffer) {
                                        Ok(unified_event) => {
                                            info!("✅ Successfully parsed UnifiedEvent from stream #{}: type={:?}, id={}", 
                                                   stream_counter, unified_event.event_type(), unified_event.id());
                                            
                                            // Update stats
                                            {
                                                let mut stats_guard = stats.write().await;
                                                stats_guard.events_received += 1;
                                                stats_guard.bytes_received += buffer.len() as u64;
                                                stats_guard.last_event_time = Some(Instant::now());
                                            }
                                            
                                            // Call the callback with UnifiedEvent
                                            info!("🎭 Calling UnifiedEvent callback for event: {}", unified_event.id());
                                            callback(unified_event);
                                        }
                                        Err(e) => {
                                            error!("❌ Failed to parse UnifiedEvent data from stream #{}: {} (buffer size: {})", 
                                                   stream_counter, e, buffer.len());
                                            debug!("🔍 Failed UnifiedEvent buffer hex dump: {}", hex::encode(&buffer[..std::cmp::min(200, buffer.len())]));
                                        }
                                    }
                                } else {
                                    warn!("⚠️ UnifiedEvent stream #{} ended with no data", stream_counter);
                                }
                            }
                            Err(e) => {
                                error!("❌ Error accepting UnifiedEvent stream #{}: {}", stream_counter, e);
                                break;
                            }
                        }
                    }
                }
            }
            
            info!("🔚 UnifiedEvent receive loop ended after processing {} streams", stream_counter);
        });
        
        Ok(())
    }

    /// Parse event data as UnifiedEvent - now EventMessage is self-describing
    fn parse_event_data_as_unified(
        raw_data: &[u8], 
    ) -> Result<Box<dyn UnifiedEvent>> {
        // 首先反序列化 EventMessage 包装器（总是用 bincode）
        let event_message: EventMessage = bincode::deserialize(raw_data)?;
        
        info!("📥 Received self-describing UnifiedEvent: serialization={:?}, compression={:?}, compressed={}", 
              event_message.serialization_format, event_message.compression_format, event_message.is_compressed);
        
        // 使用 EventMessage 中的格式信息来解压缩数据
        let decompressed_data = event_message.get_decompressed_data().map_err(|e| anyhow::anyhow!("Failed to decompress data: {}", e))?;
        
        // 根据 EventMessage 中的序列化格式来解析
        match event_message.serialization_format {
            SerializationProtocol::JSON => {
                // JSON 不支持 UnifiedEvent 回调
                Err(anyhow::anyhow!("JSON events not supported for UnifiedEvent callback"))
            }
            SerializationProtocol::Bincode => {
                Self::deserialize_solana_event_as_unified(&decompressed_data, &event_message.event_type)
            }
            SerializationProtocol::Auto => {
                // 对于 Auto，尝试 bincode
                Self::deserialize_solana_event_as_unified(&decompressed_data, &event_message.event_type)
            }
        }
    }

    /// Deserialize Solana event from bincode data as UnifiedEvent
    fn deserialize_solana_event_as_unified(data: &[u8], event_type: &EventType) -> Result<Box<dyn UnifiedEvent>> {
        info!("🔍 Attempting to deserialize {:?} event with {} bytes", event_type, data.len());
        
        // Try to deserialize as the specific Solana event type
        match event_type {
            EventType::BlockMeta => {
                if let Ok(event) = bincode::deserialize::<BlockMetaEvent>(data) {
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
                if let Ok(event) = bincode::deserialize::<pumpswap::PumpSwapBuyEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapSell => {
                if let Ok(event) = bincode::deserialize::<pumpswap::PumpSwapSellEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapCreate => {
                if let Ok(event) = bincode::deserialize::<pumpswap::PumpSwapCreatePoolEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapDeposit => {
                if let Ok(event) = bincode::deserialize::<pumpswap::PumpSwapDepositEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapWithdraw => {
                if let Ok(event) = bincode::deserialize::<pumpswap::PumpSwapWithdrawEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumCpmmSwap => {
                if let Ok(event) = bincode::deserialize::<raydium_cpmm::RaydiumCpmmSwapEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumCpmmDeposit => {
                if let Ok(event) = bincode::deserialize::<raydium_cpmm::RaydiumCpmmDepositEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumCpmmInitialize => {
                if let Ok(event) = bincode::deserialize::<raydium_cpmm::RaydiumCpmmInitializeEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumCpmmWithdraw => {
                if let Ok(event) = bincode::deserialize::<raydium_cpmm::RaydiumCpmmWithdrawEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmSwap => {
                if let Ok(event) = bincode::deserialize::<raydium_clmm::RaydiumClmmSwapEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmSwapV2 => {
                if let Ok(event) = bincode::deserialize::<raydium_clmm::RaydiumClmmSwapV2Event>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmClosePosition => {
                if let Ok(event) = bincode::deserialize::<raydium_clmm::RaydiumClmmClosePositionEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmDecreaseLiquidityV2 => {
                if let Ok(event) = bincode::deserialize::<raydium_clmm::RaydiumClmmDecreaseLiquidityV2Event>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmCreatePool => {
                if let Ok(event) = bincode::deserialize::<raydium_clmm::RaydiumClmmCreatePoolEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmIncreaseLiquidityV2 => {
                if let Ok(event) = bincode::deserialize::<raydium_clmm::RaydiumClmmIncreaseLiquidityV2Event>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmOpenPositionWithToken22Nft => {
                if let Ok(event) = bincode::deserialize::<raydium_clmm::RaydiumClmmOpenPositionWithToken22NftEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmOpenPositionV2 => {
                if let Ok(event) = bincode::deserialize::<raydium_clmm::RaydiumClmmOpenPositionV2Event>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumAmmV4Swap => {
                if let Ok(event) = bincode::deserialize::<raydium_amm_v4::RaydiumAmmV4SwapEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumAmmV4Deposit => {
                if let Ok(event) = bincode::deserialize::<raydium_amm_v4::RaydiumAmmV4DepositEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumAmmV4Initialize => {
                if let Ok(event) = bincode::deserialize::<raydium_amm_v4::RaydiumAmmV4Initialize2Event>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumAmmV4Withdraw => {
                if let Ok(event) = bincode::deserialize::<raydium_amm_v4::RaydiumAmmV4WithdrawEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumAmmV4WithdrawPnl => {
                if let Ok(event) = bincode::deserialize::<raydium_amm_v4::RaydiumAmmV4WithdrawPnlEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::BonkPoolStateAccount => {
                if let Ok(event) = bincode::deserialize::<bonk::BonkPoolStateAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::BonkGlobalConfigAccount => {
                if let Ok(event) = bincode::deserialize::<bonk::BonkGlobalConfigAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::BonkPlatformConfigAccount => {
                if let Ok(event) = bincode::deserialize::<bonk::BonkPlatformConfigAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapGlobalConfigAccount => {
                if let Ok(event) = bincode::deserialize::<pumpswap::PumpSwapGlobalConfigAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpSwapPoolAccount => {
                if let Ok(event) = bincode::deserialize::<pumpswap::PumpSwapPoolAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpFunBondingCurveAccount => {
                if let Ok(event) = bincode::deserialize::<pumpfun::PumpFunBondingCurveAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::PumpFunGlobalAccount => {
                if let Ok(event) = bincode::deserialize::<pumpfun::PumpFunGlobalAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumAmmV4InfoAccount => {
                if let Ok(event) = bincode::deserialize::<raydium_amm_v4::RaydiumAmmV4AmmInfoAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmConfigAccount => {
                if let Ok(event) = bincode::deserialize::<raydium_clmm::RaydiumClmmAmmConfigAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmPoolStateAccount => {
                if let Ok(event) = bincode::deserialize::<raydium_clmm::RaydiumClmmPoolStateAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumClmmTickArrayAccount => {
                if let Ok(event) = bincode::deserialize::<raydium_clmm::RaydiumClmmTickArrayStateAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumCpmmConfigAccount => {
                if let Ok(event) = bincode::deserialize::<raydium_cpmm::RaydiumCpmmAmmConfigAccountEvent>(data) {
                    return Ok(Box::new(event) as Box<dyn UnifiedEvent>);
                }
            },
            EventType::RaydiumCpmmPoolStateAccount => {
                if let Ok(event) = bincode::deserialize::<raydium_cpmm::RaydiumCpmmPoolStateAccountEvent>(data) {
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

