// Configuration system for the Fast Stream Client SDK
// use serde::{Serialize, Deserialize}; // Unused for now
use std::collections::HashMap;
use fzstream_common::{
    SerializationProtocol, 
    CompressionLevel, 
    PerformanceProfile, 
    CustomSettings, 
    AutoOptimizationConfig,
    ClientConfig as CommonClientConfig
};

// 使用公共库中的类型定义，但创建一个包装类型来添加方法
#[derive(Debug, Clone)]
pub struct ClientConfig {
    #[allow(dead_code)]
    inner: CommonClientConfig,
}

impl Default for ClientConfig {
    fn default() -> Self {
        let mut available_profiles = HashMap::new();
        
        // Predefined performance profiles
        available_profiles.insert("ultra_low_latency".to_string(), PerformanceProfile {
            name: "Ultra Low Latency".to_string(),
            description: "For latency-sensitive real-time trading scenarios, prioritize fastest serialization".to_string(),
            serialization: SerializationProtocol::JSON,
            compression: CompressionLevel::None,
            priority: 0,
            use_cases: vec![
                "Real-time trading signals".to_string(),
                "High-frequency order book updates".to_string(),
                "Price alerts".to_string(),
            ],
        });
        
        available_profiles.insert("bandwidth_optimized".to_string(), PerformanceProfile {
            name: "Bandwidth Optimized".to_string(),
            description: "For bandwidth-constrained environments, maximize data compression efficiency".to_string(),
            serialization: SerializationProtocol::Bincode,
            compression: CompressionLevel::ZstdHigh,
            priority: 1,
            use_cases: vec![
                "Mobile network environments".to_string(),
                "Large historical data transfer".to_string(),
                "Batch event synchronization".to_string(),
            ],
        });
        
        available_profiles.insert("balanced".to_string(), PerformanceProfile {
            name: "Balanced Mode".to_string(),
            description: "Balance between latency and bandwidth".to_string(),
            serialization: SerializationProtocol::Auto,
            compression: CompressionLevel::LZ4Fast,
            priority: 2,
            use_cases: vec![
                "General web applications".to_string(),
                "Medium frequency data updates".to_string(),
                "Development and testing environments".to_string(),
            ],
        });
        
        available_profiles.insert("high_throughput".to_string(), PerformanceProfile {
            name: "High Throughput".to_string(),
            description: "For large data transfer scenarios, optimize overall throughput".to_string(),
            serialization: SerializationProtocol::Bincode,
            compression: CompressionLevel::LZ4Fast,
            priority: 3,
            use_cases: vec![
                "Large batch event processing".to_string(),
                "Data backup and synchronization".to_string(),
                "Log aggregation".to_string(),
            ],
        });

        Self {
            inner: CommonClientConfig {
                client_id: "default_client".to_string(),
                current_profile: "balanced".to_string(),
                custom_settings: CustomSettings {
                    serialization_protocol: SerializationProtocol::Auto,
                    compression_level: CompressionLevel::LZ4Fast,
                    enable_metrics: true,
                    adaptive_protocol: true,
                    latency_target_us: Some(1000), // 1ms target latency
                    bandwidth_limit_kbps: None,
                    message_size_threshold: 256, // 256 byte threshold
                },
                available_profiles,
                auto_optimization: AutoOptimizationConfig {
                    enabled: true,
                    switch_threshold_pct: 15.0, // Switch when 15% performance difference
                    min_samples: 10,
                    evaluation_window_secs: 60,
                },
            },
        }
    }
}

#[allow(dead_code)]
impl ClientConfig {
    pub fn new(client_id: &str) -> Self {
        let mut config = Self::default();
        config.inner.client_id = client_id.to_string();
        config
    }
    
    pub fn should_use_bincode_for_message(&self, message_size: usize, is_complex: bool) -> bool {
        match self.inner.custom_settings.serialization_protocol {
            SerializationProtocol::JSON => false,
            SerializationProtocol::Bincode => true,
            SerializationProtocol::Auto => {
                // Auto selection logic
                if is_complex || message_size > self.inner.custom_settings.message_size_threshold {
                    true // Use Bincode for complex or large messages
                } else {
                    false // Use JSON for simple small messages
                }
            }
        }
    }
    
    pub fn get_compression_algorithm(&self) -> &str {
        match self.inner.custom_settings.compression_level {
            CompressionLevel::None => "none",
            CompressionLevel::LZ4Fast | CompressionLevel::LZ4High => "lz4",
            CompressionLevel::ZstdFast | CompressionLevel::ZstdMedium 
            | CompressionLevel::ZstdHigh | CompressionLevel::ZstdMax => "zstd",
        }
    }
    
    pub fn get_compression_level(&self) -> i32 {
        match self.inner.custom_settings.compression_level {
            CompressionLevel::None => 0,
            CompressionLevel::LZ4Fast => 1,
            CompressionLevel::LZ4High => 4,
            CompressionLevel::ZstdFast => 1,
            CompressionLevel::ZstdMedium => 5,
            CompressionLevel::ZstdHigh => 8,
            CompressionLevel::ZstdMax => 15,
        }
    }
    
    // Accessor methods for internal fields
    pub fn client_id(&self) -> &str {
        &self.inner.client_id
    }
    
    pub fn custom_settings(&self) -> &CustomSettings {
        &self.inner.custom_settings
    }
    
    pub fn auto_optimization(&self) -> &AutoOptimizationConfig {
        &self.inner.auto_optimization
    }
    
    pub fn inner(&self) -> &CommonClientConfig {
        &self.inner
    }
}

// Configuration builder for convenient custom configuration creation
#[allow(dead_code)]
pub struct ConfigBuilder {
    config: ClientConfig,
}

#[allow(dead_code)]
impl ConfigBuilder {
    pub fn new(client_id: &str) -> Self {
        Self {
            config: ClientConfig::new(client_id),
        }
    }
    
    pub fn protocol(mut self, protocol: SerializationProtocol) -> Self {
        self.config.inner.custom_settings.serialization_protocol = protocol;
        self
    }
    
    pub fn compression(mut self, compression: CompressionLevel) -> Self {
        self.config.inner.custom_settings.compression_level = compression;
        self
    }
    
    pub fn latency_target(mut self, target_us: u64) -> Self {
        self.config.inner.custom_settings.latency_target_us = Some(target_us);
        self
    }
    
    pub fn bandwidth_limit(mut self, limit_kbps: u64) -> Self {
        self.config.inner.custom_settings.bandwidth_limit_kbps = Some(limit_kbps);
        self
    }
    
    pub fn adaptive(mut self, enabled: bool) -> Self {
        self.config.inner.custom_settings.adaptive_protocol = enabled;
        self
    }
    
    pub fn auto_optimization(mut self, enabled: bool) -> Self {
        self.config.inner.auto_optimization.enabled = enabled;
        self
    }
    
    pub fn build(self) -> ClientConfig {
        self.config
    }
}