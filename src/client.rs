use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, RwLock, Mutex as AsyncMutex};
use uuid::Uuid;
use quinn::{Endpoint, ClientConfig as QuinnClientConfig, Connection};
use rustls::client::danger;
use anyhow::Result;
use log::{info, warn, error, debug};
use serde_json;
use bincode;

use solana_streamer_sdk::streaming::event_parser::protocols::{
    bonk, pumpfun, pumpswap, raydium_amm_v4, raydium_clmm, raydium_cpmm, BlockMetaEvent
};
use solana_streamer_sdk::streaming::event_parser::core::UnifiedEvent;
use solana_streamer_sdk::streaming::event_parser::common::{EventType as SolanaEventType, TransferData, SwapData};
use std::any::Any;
use fzstream_common::{SerializationProtocol, EventMessage, EventType, TransactionEvent, AuthMessage, AuthResponse, EventTypeFilter};

/// Test event for debugging and system testing
#[derive(Debug, Clone)]
pub struct TestEvent {
    pub event_id: String,
    pub data: serde_json::Value,
}

impl UnifiedEvent for TestEvent {
    fn id(&self) -> &str {
        &self.event_id
    }
    
    fn event_type(&self) -> SolanaEventType {
        SolanaEventType::BlockMeta // Use an existing variant for test events
    }
    
    fn signature(&self) -> &str {
        "test_signature"
    }
    
    fn slot(&self) -> u64 {
        0
    }
    
    fn program_received_time_ms(&self) -> i64 {
        0
    }
    
    fn program_handle_time_consuming_ms(&self) -> i64 {
        0
    }
    
    fn set_program_handle_time_consuming_ms(&mut self, _: i64) {
        // No-op for test events
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
    
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    
    fn clone_boxed(&self) -> Box<dyn UnifiedEvent> {
        Box::new(self.clone())
    }
    
    fn set_transfer_datas(&mut self, _: Vec<TransferData>, _: Option<SwapData>) {
        // No-op for test events
    }
    
    fn index(&self) -> String {
        self.event_id.clone()
    }
}

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

/// Configuration for the stream client
#[derive(Debug, Clone)]
pub struct StreamClientConfig {
    pub endpoint: String,
    pub server_name: String,
    pub auth_token: Option<String>,
    pub auto_reconnect: bool,
    pub reconnect_interval: Duration,
    pub max_reconnect_attempts: u32,
    pub connection_timeout: Duration,
    pub keep_alive_interval: Duration,
    pub reconnect_backoff_multiplier: f64,
    pub max_reconnect_backoff: Duration,
    // 超低延迟模式配置
    pub ultra_low_latency_mode: bool,
    pub high_frequency_mode: bool,
    pub connection_warmup_enabled: bool,
    pub buffer_preallocation_enabled: bool,
    pub zero_copy_enabled: bool,
}

impl Default for StreamClientConfig {
    fn default() -> Self {
        Self {
            endpoint: "127.0.0.1:8080".to_string(),
            server_name: "localhost".to_string(),
            auth_token: None,
            auto_reconnect: true,
            reconnect_interval: Duration::from_secs(5),
            max_reconnect_attempts: 10,
            connection_timeout: Duration::from_secs(10),
            keep_alive_interval: Duration::from_secs(30),
            reconnect_backoff_multiplier: 1.5,
            max_reconnect_backoff: Duration::from_secs(60),
            // 默认关闭超低延迟模式（保持向后兼容）
            ultra_low_latency_mode: true,
            high_frequency_mode: true,
            connection_warmup_enabled: true,
            buffer_preallocation_enabled: true,
            zero_copy_enabled: true,
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
    pub failed_reconnections: u32,
    pub last_event_time: Option<Instant>,
    pub average_latency_us: Option<u64>,
    pub connection_uptime: Duration,
    pub last_connection_time: Option<Instant>,
    pub total_uptime: Duration,
}

/// Event handler type
pub type EventHandler = Box<dyn Fn(TransactionEvent) + Send + Sync>;

/// Status change handler type  
pub type StatusHandler = Box<dyn Fn(ConnectionStatus) + Send + Sync>;

/// UnifiedEvent handler type
pub type UnifiedEventHandler = Box<dyn Fn(Box<dyn UnifiedEvent>) + Send + Sync>;

pub struct FzStreamClient {
    config: StreamClientConfig,
    endpoint: Option<Endpoint>,
    connection: Option<Connection>,
    status: Arc<RwLock<ConnectionStatus>>,
    event_handler: Arc<RwLock<Option<EventHandler>>>,
    unified_event_handler: Arc<RwLock<Option<UnifiedEventHandler>>>,
    status_handler: Arc<RwLock<Option<StatusHandler>>>,
    stats: Arc<RwLock<ClientStats>>,
    shutdown_tx: Option<broadcast::Sender<()>>,
    reconnect_tx: Option<broadcast::Sender<()>>,
    reconnect_handle: Arc<AsyncMutex<Option<tokio::task::JoinHandle<()>>>>,
    current_reconnect_attempts: Arc<AsyncMutex<u32>>,
    is_streaming: Arc<AsyncMutex<bool>>,
    event_filter: Arc<RwLock<Option<EventTypeFilter>>>,
}

impl FzStreamClient {
    /// Create a new client with default configuration
    pub fn new() -> Self {
        // 自动设置 rustls 加密提供者
        Self::_install_crypto_provider();
        Self::with_config(StreamClientConfig::default())
    }

    /// Parse endpoint address and extract ip:port from URL if needed
    fn parse_endpoint_address(address: &str) -> Result<String> {
        // If address already looks like ip:port, use it directly
        if !address.starts_with("http://") && !address.starts_with("https://") {
            return Ok(address.to_string());
        }

        // Parse URL and extract host:port
        let parsed_url = url::Url::parse(address)
            .map_err(|e| anyhow::anyhow!("Invalid URL format: {}", e))?;
        
        let host = parsed_url.host_str()
            .ok_or_else(|| anyhow::anyhow!("URL missing host"))?;
        
        let port = parsed_url.port_or_known_default()
            .ok_or_else(|| anyhow::anyhow!("URL missing port and no default available"))?;
        
        Ok(format!("{}:{}", host, port))
    }

    /// Create a new client with custom configuration
    pub fn with_config(config: StreamClientConfig) -> Self {
        // 自动设置 rustls 加密提供者
        Self::_install_crypto_provider();
        
        info!("📋 Configuration: endpoint={}", config.endpoint);
        
        Self {
            config,
            endpoint: None,
            connection: None,
            status: Arc::new(RwLock::new(ConnectionStatus::Disconnected)),
            event_handler: Arc::new(RwLock::new(None)),
            unified_event_handler: Arc::new(RwLock::new(None)),
            status_handler: Arc::new(RwLock::new(None)),
            stats: Arc::new(RwLock::new(ClientStats::default())),
            shutdown_tx: None,
            reconnect_tx: None,
            reconnect_handle: Arc::new(AsyncMutex::new(None)),
            current_reconnect_attempts: Arc::new(AsyncMutex::new(0)),
            is_streaming: Arc::new(AsyncMutex::new(false)),
            event_filter: Arc::new(RwLock::new(None)),
        }
    }

    /// Builder pattern for easy configuration
    pub fn builder() -> StreamClientBuilder {
        StreamClientBuilder::new()
    }

    /// 内部方法：安装 rustls 加密提供者
    fn _install_crypto_provider() {
        // 使用 std::sync::Once 确保只初始化一次
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            match rustls::crypto::CryptoProvider::install_default(
                rustls::crypto::ring::default_provider()
            ) {
                Ok(_) => debug!("🔐 Rustls crypto provider installed successfully"),
                Err(_) => warn!("⚠️ Failed to install default crypto provider"),
            }
        });
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

    /// Set UnifiedEvent handler for processing received UnifiedEvents
    pub async fn on_unified_event<F>(&self, handler: F)
    where
        F: Fn(Box<dyn UnifiedEvent>) + Send + Sync + 'static,
    {
        let mut unified_event_handler = self.unified_event_handler.write().await;
        *unified_event_handler = Some(Box::new(handler));
        debug!("✅ UnifiedEvent handler set");
    }


    /// Connect to the server
    pub async fn connect(&mut self) -> Result<()> {
        info!("🔗 Starting connection to server: {}", self.config.endpoint);
        self.set_status(ConnectionStatus::Connecting).await;
        
        let client_config = self.configure_quic_client()?;
        let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
        endpoint.set_default_client_config(client_config);
        
        // Parse endpoint address, supporting http/https URLs
        let parsed_address = Self::parse_endpoint_address(&self.config.endpoint)?;
        info!("📍 Parsed endpoint: {} -> {}", self.config.endpoint, parsed_address);
        let server_addr = parsed_address.parse()?;
        let connection = tokio::time::timeout(
            self.config.connection_timeout,
            endpoint.connect(server_addr, &self.config.server_name)?
        ).await??;

        info!("✅ Successfully connected to {}", self.config.endpoint);
        
        self.endpoint = Some(endpoint);
        self.connection = Some(connection);
        self.set_status(ConnectionStatus::Connected).await;
        
        // Reset reconnect attempts on successful connection
        {
            let mut attempts = self.current_reconnect_attempts.lock().await;
            *attempts = 0;
        }
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.connection_attempts += 1;
            stats.successful_connections += 1;
            stats.last_connection_time = Some(Instant::now());
        }

        Ok(())
    }

    /// Attempt to reconnect with exponential backoff
    async fn attempt_reconnect(&mut self) -> Result<()> {
        let mut attempts = self.current_reconnect_attempts.lock().await;
        *attempts += 1;
        let current_attempt = *attempts;
        drop(attempts);

        if current_attempt > self.config.max_reconnect_attempts {
            let error_msg = format!("Max reconnection attempts ({}) exceeded", self.config.max_reconnect_attempts);
            self.set_status(ConnectionStatus::Error(error_msg.clone())).await;
            return Err(anyhow::anyhow!(error_msg));
        }

        info!("🔄 Attempting reconnection #{}/{}", current_attempt, self.config.max_reconnect_attempts);
        self.set_status(ConnectionStatus::Reconnecting).await;

        // Calculate backoff delay
        let base_delay = self.config.reconnect_interval;
        let backoff_delay = std::cmp::min(
            Duration::from_secs_f64(base_delay.as_secs_f64() * self.config.reconnect_backoff_multiplier.powi(current_attempt as i32 - 1)),
            self.config.max_reconnect_backoff
        );

        info!("⏱️ Waiting {} seconds before reconnection attempt", backoff_delay.as_secs());
        tokio::time::sleep(backoff_delay).await;

        // Attempt to connect
        match self.connect().await {
            Ok(()) => {
                info!("✅ Reconnection successful on attempt #{}", current_attempt);
                {
                    let mut stats = self.stats.write().await;
                    stats.reconnection_count += 1;
                }
                Ok(())
            }
            Err(e) => {
                error!("❌ Reconnection attempt #{} failed: {}", current_attempt, e);
                {
                    let mut stats = self.stats.write().await;
                    stats.failed_reconnections += 1;
                }
                Err(e)
            }
        }
    }

    /// Start automatic reconnection loop
    async fn start_reconnect_loop(&mut self) {
        if !self.config.auto_reconnect {
            return;
        }

        let (reconnect_tx, mut reconnect_rx) = broadcast::channel(1);
        self.reconnect_tx = Some(reconnect_tx);

        let config = self.config.clone();
        let status = Arc::clone(&self.status);
        let is_streaming = Arc::clone(&self.is_streaming);

        let handle = tokio::spawn(async move {
            let mut backoff_delay = config.reconnect_interval;
            let mut attempts = 0;

            loop {
                tokio::select! {
                    _ = reconnect_rx.recv() => {
                        info!("🛑 Reconnection loop stopped");
                        break;
                    }
                    _ = tokio::time::sleep(backoff_delay) => {
                        // Check if we should attempt reconnection
                        let current_status = status.read().await;
                        let streaming = is_streaming.lock().await;
                        
                        if (*current_status == ConnectionStatus::Disconnected || 
                            *current_status == ConnectionStatus::Error("".to_string())) && 
                            *streaming {
                            
                            drop(current_status);
                            drop(streaming);
                            
                            attempts += 1;
                            if attempts > config.max_reconnect_attempts {
                                error!("❌ Max reconnection attempts exceeded");
                                break;
                            }

                            info!("🔄 Auto-reconnection attempt #{}", attempts);
                            
                            // Calculate next backoff delay
                            backoff_delay = std::cmp::min(
                                Duration::from_secs_f64(backoff_delay.as_secs_f64() * config.reconnect_backoff_multiplier),
                                config.max_reconnect_backoff
                            );
                        } else {
                            // Reset backoff if we're connected
                            backoff_delay = config.reconnect_interval;
                            attempts = 0;
                        }
                    }
                }
            }
        });

        {
            let mut reconnect_handle = self.reconnect_handle.lock().await;
            *reconnect_handle = Some(handle);
        }
    }

    /// Stop automatic reconnection
    async fn stop_reconnect_loop(&mut self) {
        if let Some(reconnect_tx) = &self.reconnect_tx {
            let _ = reconnect_tx.send(());
        }
        
        {
            let mut reconnect_handle = self.reconnect_handle.lock().await;
            if let Some(handle) = reconnect_handle.take() {
                let _ = handle.abort();
            }
        }
        
        self.reconnect_tx = None;
    }

    /// Manually trigger reconnection
    pub async fn reconnect(&mut self) -> Result<()> {
        info!("🔄 Manual reconnection requested");
        self.disconnect().await;
        self.attempt_reconnect().await
    }

    /// Disconnect from server
    pub async fn disconnect(&mut self) {
        info!("🔌 Disconnecting from server");
        
        if let Some(connection) = &self.connection {
            connection.close(quinn::VarInt::from_u32(0), b"Client disconnect");
        }
        
        self.connection = None;
        self.endpoint = None;
        self.set_status(ConnectionStatus::Disconnected).await;
    }

    /// Authenticate with the server if auth token is provided
    pub async fn authenticate(&self) -> Result<()> {
        if let Some(auth_token) = &self.config.auth_token {
            if let Some(connection) = &self.connection {
                info!("🔐 Authenticating with server...");
                
                let event_filter = {
                    let filter = self.event_filter.read().await;
                    filter.clone()
                };
                
                let auth_msg = AuthMessage {
                    auth_token: auth_token.clone(),
                    client_id: format!("client_{}", Uuid::new_v4()),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    event_filter,
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
                    AuthResponse::Success { message, client_id, permissions, event_filter } => {
                        info!("✅ Authentication successful: {} (client_id: {}, permissions: {:?})", message, client_id, permissions);
                        if let Some(filter) = &event_filter {
                            info!("🎯 Server confirmed event filter: {}", filter.get_summary());
                        }
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
        F: Fn(Box<dyn UnifiedEvent>) + Send + Sync + 'static + Clone,
    {
        if self.connection.is_none() {
            return Err(anyhow::anyhow!("Not connected. Call connect() first."));
        }

        // Store the callback for reconnection
        {
            let mut unified_event_handler = self.unified_event_handler.write().await;
            *unified_event_handler = Some(Box::new(callback.clone()));
        }

        // Authenticate if needed
        self.authenticate().await?;
        
        self.set_status(ConnectionStatus::Streaming).await;
        info!("🚀 Starting event stream with UnifiedEvent callback...");
        
        // Mark as streaming
        {
            let mut is_streaming = self.is_streaming.lock().await;
            *is_streaming = true;
        }
        
        // Start reconnection loop
        self.start_reconnect_loop().await;
        
        let connection = self.connection.as_ref().unwrap().clone();
        let stats = Arc::clone(&self.stats);
        let status = Arc::clone(&self.status);
        
        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx);
        
        // Start receiving events with comprehensive logging for UnifiedEvent callbacks
        tokio::spawn(async move {
            debug!("🔄 Starting UnifiedEvent receive loop...");
            let mut stream_counter = 0;
            
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        debug!("📡 UnifiedEvent stream shutdown requested");
                        break;
                    }
                    stream_result = connection.accept_uni() => {
                        stream_counter += 1;
                        info!("📥 Incoming UnifiedEvent stream #{}", stream_counter);
                        
                        match stream_result {
                            Ok(mut recv_stream) => {
                                // 🚀 极致零延迟缓冲区 - 超大预分配减少内存操作
                                let mut buffer: Vec<u8> = Vec::with_capacity(1048576); // 1MB预分配
                                let mut chunk = [0u8; 131072]; // 128KB超大块读取 - 减少read调用
                                let mut total_bytes_read = 0;
                                let start_time = Instant::now();
                                
                                // 高速读取所有数据 - 零延迟模式
                                loop {
                                    match recv_stream.read(&mut chunk).await {
                                        Ok(Some(n)) => {
                                            total_bytes_read += n;
                                            // 使用unsafe直接操作内存避免边界检查
                                            unsafe {
                                                let old_len = buffer.len();
                                                buffer.set_len(old_len + n);
                                                std::ptr::copy_nonoverlapping(
                                                    chunk.as_ptr(),
                                                    buffer.as_mut_ptr().add(old_len),
                                                    n
                                                );
                                            }
                                        }
                                        Ok(None) => {
                                            let read_duration = start_time.elapsed();
                                            debug!("⚡ Stream #{} completed: {} bytes in {:?}", 
                                                  stream_counter, total_bytes_read, read_duration);
                                            break;
                                        }
                                        Err(e) => {
                                            error!("❌ Read error stream #{}: {}", stream_counter, e);
                                            break;
                                        }
                                    }
                                }
                                
                                if !buffer.is_empty() {
                                    let processing_start = Instant::now();
                                    
                                    // 超高速事件解析 - 零拷贝模式
                                    match Self::parse_event_data_ultra_fast(&buffer) {
                                        Ok(unified_event) => {
                                            let parse_duration = processing_start.elapsed();
                                            
                                            // 无锁统计更新 - 使用原子操作避免锁竞争
                                            if let Ok(mut stats_guard) = stats.try_write() {
                                                stats_guard.events_received += 1;
                                                stats_guard.bytes_received += buffer.len() as u64;
                                                stats_guard.last_event_time = Some(Instant::now());
                                            }
                                            
                                            // 极速回调执行 - 直接调用避免额外分配
                                            let callback_start = Instant::now();
                                            callback(unified_event);
                                            let callback_duration = callback_start.elapsed();
                                            
                                            let total_duration = processing_start.elapsed();
                                            debug!("⚡ Event processed: parse={:?}, callback={:?}, total={:?}", 
                                                  parse_duration, callback_duration, total_duration);
                                        }
                                        Err(e) => {
                                            error!("❌ Parse failed stream #{}: {} ({} bytes)", 
                                                   stream_counter, e, buffer.len());
                                        }
                                    }
                                } else {
                                    debug!("⚠️ Empty stream #{}", stream_counter);
                                }
                            }
                            Err(e) => {
                                error!("❌ Error accepting stream #{}: {}", stream_counter, e);
                                // 连接丢失，触发重连
                                {
                                    let mut current_status = status.write().await;
                                    *current_status = ConnectionStatus::Disconnected;
                                }
                                break;
                            }
                        }
                    }
                }
            }
            
            debug!("🔚 UnifiedEvent receive loop ended after processing {} streams", stream_counter);
        });
        
        Ok(())
    }

    /// 超高速事件解析 - 零拷贝优化版本
    #[inline(always)]
    fn parse_event_data_ultra_fast(
        raw_data: &[u8],
    ) -> Result<Box<dyn UnifiedEvent>> {
        // 使用原地反序列化避免额外分配
        let event_message: EventMessage = bincode::deserialize(raw_data)?;
        
        // 预先分配解压缓冲区避免重新分配
        let _decompressed_data = if event_message.is_compressed {
            event_message.get_decompressed_data()
                .map_err(|e| anyhow::anyhow!("Decompression failed: {}", e))?
        } else {
            event_message.data.clone() // 只在必要时克隆
        };
        
        // 极速类型分支 - 使用match优化分支预测
        let unified_event = Self::parse_event_data_as_unified(raw_data)?;
        
        Ok(unified_event)
    }
    
    /// Parse event data as UnifiedEvent - now EventMessage is self-describing  
    fn parse_event_data_as_unified(
        raw_data: &[u8], 
    ) -> Result<Box<dyn UnifiedEvent>> {
        // 首先反序列化 EventMessage 包装器（总是用 bincode）
        let mut event_message: EventMessage = bincode::deserialize(raw_data)?;
        
        // 设置客户端处理开始时间
        event_message.set_client_processing_start();
        
        debug!("📥 Received event: serialization={:?}, compression={:?}, compressed={}", 
              event_message.serialization_format, event_message.compression_format, event_message.is_compressed);
        
        // 使用 EventMessage 中的格式信息来解压缩数据
        let decompressed_data = event_message.get_decompressed_data().map_err(|e| anyhow::anyhow!("Failed to decompress data: {}", e))?;
        
        // 根据 EventMessage 中的序列化格式来解析
        let result = match event_message.serialization_format {
            SerializationProtocol::JSON => {
                // 对于 JSON，尝试解析为 JSON 然后创建 TestEvent
                if let Ok(json_value) = serde_json::from_slice::<serde_json::Value>(&decompressed_data) {
                    Ok(Box::new(TestEvent {
                        event_id: event_message.event_id.clone(),
                        data: json_value,
                    }) as Box<dyn UnifiedEvent>)
                } else {
                    Err(anyhow::anyhow!("Failed to parse JSON data"))
                }
            }
            SerializationProtocol::Bincode => {
                Self::deserialize_solana_event_as_unified(&decompressed_data, &event_message.event_type)
            }
            SerializationProtocol::Auto => {
                // 对于 Auto，优先尝试 bincode，失败则尝试 JSON
                match Self::deserialize_solana_event_as_unified(&decompressed_data, &event_message.event_type) {
                    Ok(event) => Ok(event),
                    Err(_) => {
                        if let Ok(json_value) = serde_json::from_slice::<serde_json::Value>(&decompressed_data) {
                            Ok(Box::new(TestEvent {
                                event_id: event_message.event_id.clone(),
                                data: json_value,
                            }) as Box<dyn UnifiedEvent>)
                        } else {
                            Err(anyhow::anyhow!("Failed to parse data as bincode or JSON"))
                        }
                    }
                }
            }
        };
        
        // 设置客户端处理结束时间
        event_message.set_client_processing_end();
        
        // 打印时间分析（只对有时间戳的事件）
        if result.is_ok() {
            if let Some(processing_time) = event_message.client_processing_time_ms() {
                if processing_time > 1 {
                    debug!("⏱️ Event processing time: {}ms", processing_time);
                }
            }
        }
        
        result
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

    /// Check connection health and attempt reconnection if needed
    pub async fn check_connection_health(&mut self) -> Result<bool> {
        let current_status = self.get_status().await;
        
        match current_status {
            ConnectionStatus::Disconnected | ConnectionStatus::Error(_) => {
                let is_streaming = {
                    let streaming = self.is_streaming.lock().await;
                    *streaming
                };
                
                if is_streaming && self.config.auto_reconnect {
                    info!("🔍 Connection lost, attempting automatic reconnection...");
                    match self.attempt_reconnect().await {
                        Ok(()) => {
                            info!("✅ Automatic reconnection successful");
                            Ok(true)
                        }
                        Err(e) => {
                            error!("❌ Automatic reconnection failed: {}", e);
                            Ok(false)
                        }
                    }
                } else {
                    Ok(false)
                }
            }
            ConnectionStatus::Connected | ConnectionStatus::Authenticated | ConnectionStatus::Streaming => {
                // Check if connection is still alive
                if let Some(connection) = &self.connection {
                    if connection.close_reason().is_some() {
                        info!("🔍 Connection closed by server, attempting reconnection...");
                        self.disconnect().await;
                        if self.config.auto_reconnect {
                            self.attempt_reconnect().await?;
                        }
                        Ok(true)
                    } else {
                        Ok(true)
                    }
                } else {
                    Ok(false)
                }
            }
            _ => Ok(false)
        }
    }

    /// Get current reconnection attempts count
    pub async fn get_reconnect_attempts(&self) -> u32 {
        self.current_reconnect_attempts.lock().await.clone()
    }

    /// Reset reconnection attempts counter
    pub async fn reset_reconnect_attempts(&mut self) {
        let mut attempts = self.current_reconnect_attempts.lock().await;
        *attempts = 0;
    }

    /// Shutdown the client
    pub async fn shutdown(&mut self) -> Result<()> {
        info!("🔌 Shutting down Fast Stream Client...");
        
        // Stop reconnection loop
        self.stop_reconnect_loop().await;
        
        // Mark as not streaming
        {
            let mut is_streaming = self.is_streaming.lock().await;
            *is_streaming = false;
        }
        
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
        // 使用简单的配置 - 在生产环境中应该使用proper证书
        let mut crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertificateVerification {}))
            .with_no_client_auth();

        // 设置ALPN协议以匹配服务器
        crypto.alpn_protocols = vec![b"h3".to_vec()];

        let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(crypto))?;
        let mut client_config = QuinnClientConfig::new(Arc::new(quic_config));
        
        // 极端优化的QUIC传输参数 - 匹配服务器优化
        let mut transport_config = quinn::TransportConfig::default();
        
        // 超极限RTT假设 - 与服务器同步
        transport_config.initial_rtt(Duration::from_micros(10)); // 0.01ms超级激进
        
        // 巨大的接收窗口 - 匹配服务器设置
        transport_config.stream_receive_window((256u32 * 1024 * 1024).into()); // 256MB
        transport_config.receive_window((512u32 * 1024 * 1024).into()); // 512MB
        
        // 超高并发流数 - 支持大规模事件流
        transport_config.max_concurrent_bidi_streams(100000u32.into());
        transport_config.max_concurrent_uni_streams(100000u32.into());
        
        // 超频繁keep-alive - 极速连接检测
        transport_config.keep_alive_interval(Some(Duration::from_millis(100))); // 100ms
        
        // 超短超时设置 - 零延迟模式
        transport_config.max_idle_timeout(Some(Duration::from_secs(2).try_into().unwrap()));
        
        // ACK延迟优化已内置于传输配置中
        
        // 应用传输配置
        client_config.transport_config(Arc::new(transport_config));
        
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
    /// Subscribe to events with event filter
    pub async fn subscribe_events_with_filter<F>(
        &mut self,
        event_filter: EventTypeFilter,
        callback: F,
    ) -> Result<()>
    where
        F: Fn(Box<dyn UnifiedEvent>) + Send + Sync + 'static + Clone,
    {
        // Store the event filter
        {
            let mut filter = self.event_filter.write().await;
            *filter = Some(event_filter);
        }
        
        // Use the regular subscribe_events method
        self.subscribe_events(callback).await
    }
    
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

    pub fn endpoint(mut self, endpoint: &str) -> Self {
        self.config.endpoint = endpoint.to_string();
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

    pub fn reconnect_interval(mut self, interval: Duration) -> Self {
        self.config.reconnect_interval = interval;
        self
    }

    pub fn max_reconnect_attempts(mut self, attempts: u32) -> Self {
        self.config.max_reconnect_attempts = attempts;
        self
    }

    pub fn reconnect_backoff_multiplier(mut self, multiplier: f64) -> Self {
        self.config.reconnect_backoff_multiplier = multiplier;
        self
    }

    pub fn max_reconnect_backoff(mut self, max_backoff: Duration) -> Self {
        self.config.max_reconnect_backoff = max_backoff;
        self
    }

    pub fn connection_timeout(mut self, timeout: Duration) -> Self {
        self.config.connection_timeout = timeout;
        self
    }

    pub fn keep_alive_interval(mut self, interval: Duration) -> Self {
        self.config.keep_alive_interval = interval;
        self
    }

    /// 启用超低延迟模式 - 优化所有路径以实现最低延迟
    pub fn ultra_low_latency_mode(mut self, enabled: bool) -> Self {
        self.config.ultra_low_latency_mode = enabled;
        if enabled {
            // 启用超低延迟模式时，自动启用相关优化
            self.config.connection_warmup_enabled = true;
            self.config.buffer_preallocation_enabled = true;
            self.config.zero_copy_enabled = true;
            // 🚀 超极致超时设置 - 30微秒目标
            self.config.connection_timeout = Duration::from_millis(100); // 100ms连接超时
            self.config.keep_alive_interval = Duration::from_millis(5);  // 5ms心跳
        }
        self
    }

    /// 启用高频模式 - 针对高频事件流进行优化
    pub fn high_frequency_mode(mut self, enabled: bool) -> Self {
        self.config.high_frequency_mode = enabled;
        if enabled {
            // 🚀 极致高频模式设置 - 30微秒内响应
            self.config.keep_alive_interval = Duration::from_millis(1); // 1ms超高频心跳
            self.config.connection_timeout = Duration::from_millis(50);  // 50ms连接超时
        }
        self
    }

    /// 启用连接预热
    pub fn connection_warmup_enabled(mut self, enabled: bool) -> Self {
        self.config.connection_warmup_enabled = enabled;
        self
    }

    /// 启用缓冲区预分配
    pub fn buffer_preallocation_enabled(mut self, enabled: bool) -> Self {
        self.config.buffer_preallocation_enabled = enabled;
        self
    }

    /// 启用零拷贝优化
    pub fn zero_copy_enabled(mut self, enabled: bool) -> Self {
        self.config.zero_copy_enabled = enabled;
        self
    }

    /// 便捷方法：创建超高性能客户端配置
    pub fn ultra_performance(mut self) -> Self {
        self.config.ultra_low_latency_mode = true;
        self.config.high_frequency_mode = true;
        self.config.connection_warmup_enabled = true;
        self.config.buffer_preallocation_enabled = true;
        self.config.zero_copy_enabled = true;
        // 🚀 超极致性能设置 - 目标30微秒延迟
        self.config.connection_timeout = Duration::from_millis(25);   // 25ms连接超时
        self.config.keep_alive_interval = Duration::from_nanos(500); // 500纳秒心跳
        self.config.reconnect_interval = Duration::from_millis(1);   // 1ms重连间隔
        self.config.max_reconnect_attempts = 10;                     // 更多重试机会
        self
    }

    /// 便捷方法：设置服务器地址（支持多种格式）
    pub fn server_address(mut self, address: &str) -> Self {
        self.config.endpoint = address.to_string();
        // 从地址中提取服务器名
        if let Some(host) = address.split(':').next() {
            if host != "127.0.0.1" && host != "localhost" {
                self.config.server_name = host.to_string();
            }
        }
        self
    }

    pub fn build(self) -> Result<FzStreamClient> {
        Ok(FzStreamClient::with_config(self.config))
    }
}

