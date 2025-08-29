//! Client-side performance optimization module
//!
//! This module provides client-side performance optimizations including:
//! - Connection pooling
//! - Batch processing
//! - Event buffering
//! - Performance monitoring

use std::time::{Duration, Instant};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use anyhow::Result;
use serde::{Serialize, Deserialize};

/// Client performance configuration
#[derive(Debug, Clone)]
pub struct ClientPerformanceConfig {
    pub connection_timeout_ms: u64,
    pub reconnect_interval_ms: u64,
    pub max_reconnect_attempts: u32,
    pub enable_event_buffering: bool,
    pub buffer_size: usize,
    pub batch_size: usize,
    pub enable_compression: bool,
    pub heartbeat_interval_ms: u64,
}

impl Default for ClientPerformanceConfig {
    fn default() -> Self {
        Self {
            connection_timeout_ms: 5000, // 5 seconds
            reconnect_interval_ms: 1000, // 1 second
            max_reconnect_attempts: 10,
            enable_event_buffering: true,
            buffer_size: 10_000,
            batch_size: 100,
            enable_compression: true,
            heartbeat_interval_ms: 30_000, // 30 seconds
        }
    }
}

/// Client performance statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientStats {
    pub events_received: u64,
    pub events_processed: u64,
    pub bytes_received: u64,
    pub bytes_sent: u64,
    pub connection_count: u32,
    pub reconnection_count: u32,
    pub avg_event_processing_time_micros: f64,
    pub last_event_timestamp: Option<u64>,
    pub uptime_seconds: f64,
    pub throughput_eps: f64,
}

impl Default for ClientStats {
    fn default() -> Self {
        Self {
            events_received: 0,
            events_processed: 0,
            bytes_received: 0,
            bytes_sent: 0,
            connection_count: 0,
            reconnection_count: 0,
            avg_event_processing_time_micros: 0.0,
            last_event_timestamp: None,
            uptime_seconds: 0.0,
            throughput_eps: 0.0,
        }
    }
}

/// High-performance event buffer
pub struct EventBuffer<T> {
    buffer: crossbeam_queue::ArrayQueue<T>,
    capacity: usize,
}

impl<T> EventBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: crossbeam_queue::ArrayQueue::new(capacity),
            capacity,
        }
    }
    
    pub fn push(&self, item: T) -> Result<(), T> {
        self.buffer.push(item)
    }
    
    pub fn pop(&self) -> Option<T> {
        self.buffer.pop()
    }
    
    pub fn len(&self) -> usize {
        self.buffer.len()
    }
    
    pub fn is_full(&self) -> bool {
        self.buffer.len() == self.capacity
    }
    
    pub fn utilization(&self) -> f64 {
        self.buffer.len() as f64 / self.capacity as f64
    }
}

/// Client performance monitor
pub struct ClientPerformanceMonitor {
    start_time: Instant,
    events_received: AtomicU64,
    events_processed: AtomicU64,
    bytes_received: AtomicU64,
    bytes_sent: AtomicU64,
    connection_count: AtomicU64,
    reconnection_count: AtomicU64,
    total_processing_time: AtomicU64,
    last_event_time: parking_lot::Mutex<Option<Instant>>,
}

impl ClientPerformanceMonitor {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            events_received: AtomicU64::new(0),
            events_processed: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            connection_count: AtomicU64::new(0),
            reconnection_count: AtomicU64::new(0),
            total_processing_time: AtomicU64::new(0),
            last_event_time: parking_lot::Mutex::new(None),
        }
    }
    
    pub fn record_event_received(&self, bytes: usize) {
        self.events_received.fetch_add(1, Ordering::Relaxed);
        self.bytes_received.fetch_add(bytes as u64, Ordering::Relaxed);
        *self.last_event_time.lock() = Some(Instant::now());
    }
    
    pub fn record_event_processed(&self, processing_time_micros: u64) {
        self.events_processed.fetch_add(1, Ordering::Relaxed);
        self.total_processing_time.fetch_add(processing_time_micros, Ordering::Relaxed);
    }
    
    pub fn record_bytes_sent(&self, bytes: usize) {
        self.bytes_sent.fetch_add(bytes as u64, Ordering::Relaxed);
    }
    
    pub fn record_connection(&self) {
        self.connection_count.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn record_reconnection(&self) {
        self.reconnection_count.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn get_stats(&self) -> ClientStats {
        let uptime = self.start_time.elapsed().as_secs_f64();
        let events_received = self.events_received.load(Ordering::Relaxed);
        let events_processed = self.events_processed.load(Ordering::Relaxed);
        let total_processing_time = self.total_processing_time.load(Ordering::Relaxed);
        
        let avg_processing_time = if events_processed > 0 {
            total_processing_time as f64 / events_processed as f64
        } else {
            0.0
        };
        
        let throughput = if uptime > 0.0 {
            events_received as f64 / uptime
        } else {
            0.0
        };
        
        let last_event_timestamp = self.last_event_time.lock()
            .map(|time| time.duration_since(self.start_time).as_secs());
        
        ClientStats {
            events_received,
            events_processed,
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            connection_count: self.connection_count.load(Ordering::Relaxed) as u32,
            reconnection_count: self.reconnection_count.load(Ordering::Relaxed) as u32,
            avg_event_processing_time_micros: avg_processing_time,
            last_event_timestamp,
            uptime_seconds: uptime,
            throughput_eps: throughput,
        }
    }
}

/// Batch processor for efficient event handling
pub struct BatchProcessor<T> {
    batch: Vec<T>,
    batch_size: usize,
    last_flush: Instant,
    flush_interval: Duration,
}

impl<T> BatchProcessor<T> {
    pub fn new(batch_size: usize, flush_interval: Duration) -> Self {
        Self {
            batch: Vec::with_capacity(batch_size),
            batch_size,
            last_flush: Instant::now(),
            flush_interval,
        }
    }
    
    pub fn push(&mut self, item: T) -> Option<Vec<T>> {
        self.batch.push(item);
        
        if self.batch.len() >= self.batch_size || 
           self.last_flush.elapsed() >= self.flush_interval {
            self.flush()
        } else {
            None
        }
    }
    
    pub fn flush(&mut self) -> Option<Vec<T>> {
        if self.batch.is_empty() {
            None
        } else {
            let batch = std::mem::replace(&mut self.batch, Vec::with_capacity(self.batch_size));
            self.last_flush = Instant::now();
            Some(batch)
        }
    }
    
    pub fn len(&self) -> usize {
        self.batch.len()
    }
}

/// Connection pool for managing multiple QUIC connections
pub struct ConnectionPool {
    connections: Vec<quinn::Connection>,
    current_index: AtomicU64,
    max_connections: usize,
}

impl ConnectionPool {
    pub fn new(max_connections: usize) -> Self {
        Self {
            connections: Vec::with_capacity(max_connections),
            current_index: AtomicU64::new(0),
            max_connections,
        }
    }
    
    pub fn add_connection(&mut self, connection: quinn::Connection) -> bool {
        if self.connections.len() < self.max_connections {
            self.connections.push(connection);
            true
        } else {
            false
        }
    }
    
    pub fn get_connection(&self) -> Option<&quinn::Connection> {
        if self.connections.is_empty() {
            return None;
        }
        
        let index = self.current_index.fetch_add(1, Ordering::Relaxed) as usize;
        Some(&self.connections[index % self.connections.len()])
    }
    
    pub fn remove_connection(&mut self, connection_id: usize) -> bool {
        let initial_len = self.connections.len();
        self.connections.retain(|conn| conn.stable_id() != connection_id);
        self.connections.len() < initial_len
    }
    
    pub fn len(&self) -> usize {
        self.connections.len()
    }
    
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }
}

/// Reconnection manager with exponential backoff
pub struct ReconnectionManager {
    config: ClientPerformanceConfig,
    attempt_count: u32,
    last_attempt: Option<Instant>,
}

impl ReconnectionManager {
    pub fn new(config: ClientPerformanceConfig) -> Self {
        Self {
            config,
            attempt_count: 0,
            last_attempt: None,
        }
    }
    
    pub fn should_reconnect(&self) -> bool {
        if self.attempt_count >= self.config.max_reconnect_attempts {
            return false;
        }
        
        if let Some(last) = self.last_attempt {
            let backoff_ms = self.calculate_backoff();
            last.elapsed().as_millis() >= backoff_ms as u128
        } else {
            true
        }
    }
    
    pub fn record_attempt(&mut self) {
        self.attempt_count += 1;
        self.last_attempt = Some(Instant::now());
    }
    
    pub fn reset(&mut self) {
        self.attempt_count = 0;
        self.last_attempt = None;
    }
    
    fn calculate_backoff(&self) -> u64 {
        let base_interval = self.config.reconnect_interval_ms;
        let exponential_factor = 2_u64.pow(self.attempt_count.min(10)); // Cap at 2^10
        base_interval * exponential_factor
    }
    
    pub fn attempts(&self) -> u32 {
        self.attempt_count
    }
}

/// 🚀 HYPER-ULTRA-LOW-LATENCY客户端QUIC配置优化 - 目标<30微秒！
pub fn configure_client_endpoint() -> Result<quinn::ClientConfig> {
    // Install crypto provider if not already installed
    static CRYPTO_INSTALLED: std::sync::Once = std::sync::Once::new();
    CRYPTO_INSTALLED.call_once(|| {
        let _ = rustls::crypto::CryptoProvider::install_default(
            rustls::crypto::ring::default_provider()
        );
    });
    
    // 🔥 30微秒目标极限优化传输配置！
    let mut transport = quinn::TransportConfig::default();
    
    // 1. 优化初始RTT - 与服务器匹配
    transport.initial_rtt(Duration::from_millis(1)); // 1ms - 与服务器同步
    
    // 2. 高频保活 - 保持连接稳定
    transport.keep_alive_interval(Some(Duration::from_millis(50))); // 50ms保活
    
    // 3. 合理超时 - 确保连接稳定性
    transport.max_idle_timeout(Some(Duration::from_secs(3).try_into().unwrap())); // 3秒超时
    
    // 4. 超大接收窗口 - 完全消除流控制瓶颈
    transport.receive_window(quinn::VarInt::from_u32(1024 * 1024 * 1024)); // 1GB接收窗口
    transport.stream_receive_window(quinn::VarInt::from_u32(512 * 1024 * 1024)); // 512MB每流
    
    // 5. 百万级并发流
    transport.max_concurrent_bidi_streams(quinn::VarInt::from_u32(2000000)); // 2M双向流
    transport.max_concurrent_uni_streams(quinn::VarInt::from_u32(2000000)); // 2M单向流
    
    // 6. 极大数据报缓冲
    transport.datagram_receive_buffer_size(Some(64 * 1024 * 1024)); // 64MB数据报缓冲
    
    // 7. 最大MTU和禁用发现
    transport.min_mtu(1500);
    transport.mtu_discovery_config(None); // 禁用MTU发现减少延迟
    
    // 8. 极致ACK优化 - Quinn内建优化（无需手动配置）
    // ACK延迟由Quinn内部优化管理，确保纳秒级响应
    
    // 9. BBR拥塞控制
    transport.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    
    // 10. 最大发送窗口
    transport.send_window(1024 * 1024 * 1024); // 1GB发送窗口
    
    // Create TLS configuration (skip certificate verification for performance)
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    
    // Convert to QUIC configuration
    let crypto_config = quinn::crypto::rustls::QuicClientConfig::try_from(tls_config)?;
    let mut client_config = quinn::ClientConfig::new(Arc::new(crypto_config));
    client_config.transport_config(Arc::new(transport));
    
    Ok(client_config)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_event_buffer() {
        let buffer = EventBuffer::new(5);
        
        assert_eq!(buffer.push(1), Ok(()));
        assert_eq!(buffer.push(2), Ok(()));
        assert_eq!(buffer.len(), 2);
        
        assert_eq!(buffer.pop(), Some(1));
        assert_eq!(buffer.pop(), Some(2));
        assert_eq!(buffer.pop(), None);
    }
    
    #[test]
    fn test_batch_processor() {
        let mut processor = BatchProcessor::new(3, Duration::from_millis(100));
        
        assert!(processor.push(1).is_none());
        assert!(processor.push(2).is_none());
        
        let batch = processor.push(3);
        assert!(batch.is_some());
        assert_eq!(batch.unwrap(), vec![1, 2, 3]);
    }
    
    #[test]
    fn test_reconnection_manager() {
        let config = ClientPerformanceConfig::default();
        let mut manager = ReconnectionManager::new(config);
        
        assert!(manager.should_reconnect());
        
        manager.record_attempt();
        assert_eq!(manager.attempts(), 1);
        
        manager.reset();
        assert_eq!(manager.attempts(), 0);
    }
    
    #[test]
    fn test_performance_monitor() {
        let monitor = ClientPerformanceMonitor::new();
        
        monitor.record_event_received(100);
        monitor.record_event_processed(50);
        
        let stats = monitor.get_stats();
        assert_eq!(stats.events_received, 1);
        assert_eq!(stats.events_processed, 1);
        assert_eq!(stats.bytes_received, 100);
    }
}