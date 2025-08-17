use serde::{Serialize, Deserialize};
use anyhow::Result;
use log::{debug, error, info};
use std::collections::HashMap;
use tokio::sync::broadcast;

use super::client::{TransactionEvent, EventMetadata};

/// 事件类型枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EventType {
    Transaction,
    Block,
    Account,
    Slot,
    Signature,
    Custom(String),
}

/// 解析后的事件数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedEvent {
    pub event_id: String,
    pub event_type: EventType,
    pub timestamp: u64,
    pub data: serde_json::Value,
    pub metadata: EventMetadata,
}

/// 事件处理器特征
pub trait EventHandler: Send + Sync {
    fn handle_event(&self, event: ParsedEvent) -> Result<()>;
}

/// 客户端事件解析器
pub struct ClientEventParser {
    handlers: HashMap<EventType, Vec<Box<dyn EventHandler>>>,
    event_sender: broadcast::Sender<ParsedEvent>,
}

impl ClientEventParser {
    /// 创建新的事件解析器
    pub fn new() -> Self {
        let (event_sender, _) = broadcast::channel(1000);
        
        Self {
            handlers: HashMap::new(),
            event_sender,
        }
    }

    /// 注册事件处理器
    pub fn register_handler(&mut self, event_type: EventType, handler: Box<dyn EventHandler>) {
        self.handlers.entry(event_type).or_insert_with(Vec::new).push(handler);
        info!("📝 注册客户端事件处理器: {:?}", event_type);
    }

    /// 获取事件接收器
    pub fn subscribe(&self) -> broadcast::Receiver<ParsedEvent> {
        self.event_sender.subscribe()
    }

    /// 解析TransactionEvent中的data字段
    pub fn parse_transaction_event(&self, event: TransactionEvent) -> Result<ParsedEvent> {
        debug!("🔍 解析客户端事件: event_type={}, event_id={}", event.event_type, event.event_id);
        
        // 尝试解析data字段
        let parsed_data = self.parse_data_field(&event.data)?;
        
        // 确定事件类型
        let event_type = self.determine_event_type(&event.event_type, &parsed_data)?;
        
        // 提取元数据
        let metadata = self.extract_metadata(&event, &parsed_data)?;
        
        let parsed_event = ParsedEvent {
            event_id: event.event_id,
            event_type,
            timestamp: event.timestamp,
            data: parsed_data,
            metadata,
        };
        
        // 发送到广播通道
        if let Err(e) = self.event_sender.send(parsed_event.clone()) {
            error!("❌ 发送解析事件到广播通道失败: {}", e);
        }
        
        // 调用注册的处理器
        self.call_handlers(&parsed_event)?;
        
        Ok(parsed_event)
    }

    /// 解析data字段
    fn parse_data_field(&self, data: &serde_json::Value) -> Result<serde_json::Value> {
        // 如果data已经是JSON对象，直接返回
        if data.is_object() || data.is_array() || data.is_string() || data.is_number() || data.is_boolean() {
            return Ok(data.clone());
        }
        
        // 如果data是字符串，尝试解析为JSON
        if let Some(data_str) = data.as_str() {
            // 尝试解析为JSON
            if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(data_str) {
                return Ok(json_value);
            }
            
            // 如果不是JSON，返回原始字符串
            return Ok(serde_json::Value::String(data_str.to_string()));
        }
        
        // 如果data是数字数组（可能是bincode），尝试base64解码
        if let Some(data_array) = data.as_array() {
            if data_array.iter().all(|v| v.is_u64()) {
                let bytes: Vec<u8> = data_array.iter()
                    .filter_map(|v| v.as_u64().map(|n| n as u8))
                    .collect();
                
                // 尝试bincode反序列化
                if let Ok(bincode_value) = bincode::deserialize::<serde_json::Value>(&bytes) {
                    return Ok(bincode_value);
                }
                
                // 尝试字符串解析
                if let Ok(string_data) = String::from_utf8(bytes.clone()) {
                    return Ok(serde_json::Value::String(string_data));
                }
                
                // 如果都失败，返回base64编码
                let base64_data = base64::encode(&bytes);
                return Ok(serde_json::json!({
                    "raw_data": base64_data,
                    "data_format": "binary",
                    "data_size": bytes.len()
                }));
            }
        }
        
        // 默认返回原始数据
        Ok(data.clone())
    }

    /// 确定事件类型
    fn determine_event_type(&self, event_type_str: &str, data: &serde_json::Value) -> Result<EventType> {
        match event_type_str.to_lowercase().as_str() {
            "transaction" | "tx" | "pumpfuntrade" | "bonktrade" | "pumpswapbuy" | "pumpswapsell" | "raydiumcpmmswap" | "raydiumclmmswap" => Ok(EventType::Transaction),
            "block" | "blockmeta" => Ok(EventType::Block),
            "account" | "poolstateaccount" | "globalconfigaccount" | "ammconfigaccount" => Ok(EventType::Account),
            "slot" => Ok(EventType::Slot),
            "signature" | "sig" => Ok(EventType::Signature),
            _ => {
                // 尝试从数据中推断事件类型
                if let Some(obj) = data.as_object() {
                    if obj.contains_key("signature") {
                        return Ok(EventType::Transaction);
                    }
                    if obj.contains_key("slot") {
                        return Ok(EventType::Slot);
                    }
                    if obj.contains_key("block_time") {
                        return Ok(EventType::Block);
                    }
                }
                Ok(EventType::Custom(event_type_str.to_string()))
            }
        }
    }

    /// 提取元数据
    fn extract_metadata(&self, event: &TransactionEvent, data: &serde_json::Value) -> Result<EventMetadata> {
        let mut metadata = event.metadata.clone().unwrap_or_else(|| EventMetadata {
            block_time: None,
            slot: None,
            signature: None,
            source: None,
            priority: None,
        });

        // 从数据中提取元数据
        if let Some(obj) = data.as_object() {
            if metadata.slot.is_none() {
                if let Some(slot) = obj.get("slot").and_then(|v| v.as_u64()) {
                    metadata.slot = Some(slot);
                }
            }
            if metadata.block_time.is_none() {
                if let Some(block_time) = obj.get("block_time").and_then(|v| v.as_u64()) {
                    metadata.block_time = Some(block_time);
                }
            }
            if metadata.signature.is_none() {
                if let Some(signature) = obj.get("signature").and_then(|v| v.as_str()) {
                    metadata.signature = Some(signature.to_string());
                }
            }
            if metadata.source.is_none() {
                if let Some(source) = obj.get("source").and_then(|v| v.as_str()) {
                    metadata.source = Some(source.to_string());
                }
            }
        }

        Ok(metadata)
    }

    /// 调用注册的处理器
    fn call_handlers(&self, event: &ParsedEvent) -> Result<()> {
        if let Some(handlers) = self.handlers.get(&event.event_type) {
            for handler in handlers {
                if let Err(e) = handler.handle_event(event.clone()) {
                    error!("❌ 客户端事件处理器执行失败: {}", e);
                }
            }
        }
        
        // 调用通用处理器（处理所有事件类型）
        if let Some(handlers) = self.handlers.get(&EventType::Custom("all".to_string())) {
            for handler in handlers {
                if let Err(e) = handler.handle_event(event.clone()) {
                    error!("❌ 通用事件处理器执行失败: {}", e);
                }
            }
        }
        
        Ok(())
    }
}

/// 默认客户端事件处理器实现
pub struct DefaultClientEventHandler {
    name: String,
}

impl DefaultClientEventHandler {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

impl EventHandler for DefaultClientEventHandler {
    fn handle_event(&self, event: ParsedEvent) -> Result<()> {
        info!("📊 [{}] 处理客户端事件: {:?} - {}", self.name, event.event_type, event.event_id);
        debug!("📄 事件数据: {:?}", event.data);
        Ok(())
    }
}

/// 日志事件处理器
pub struct LogClientEventHandler;

impl EventHandler for LogClientEventHandler {
    fn handle_event(&self, event: ParsedEvent) -> Result<()> {
        info!("📝 客户端事件日志: {:?} - {} - {}", event.event_type, event.event_id, event.timestamp);
        if let Some(signature) = &event.metadata.signature {
            info!("🔐 签名: {}", signature);
        }
        if let Some(slot) = event.metadata.slot {
            info!("🎯 槽位: {}", slot);
        }
        Ok(())
    }
}

/// 统计事件处理器
pub struct StatsClientEventHandler {
    stats: std::sync::Arc<std::sync::Mutex<ClientEventStats>>,
}

#[derive(Debug, Default)]
pub struct ClientEventStats {
    pub total_events: u64,
    pub events_by_type: HashMap<EventType, u64>,
    pub last_event_time: Option<u64>,
}

impl StatsClientEventHandler {
    pub fn new() -> Self {
        Self {
            stats: std::sync::Arc::new(std::sync::Mutex::new(ClientEventStats::default())),
        }
    }

    pub fn get_stats(&self) -> ClientEventStats {
        self.stats.lock().unwrap().clone()
    }
}

impl EventHandler for StatsClientEventHandler {
    fn handle_event(&self, event: ParsedEvent) -> Result<()> {
        let mut stats = self.stats.lock().unwrap();
        stats.total_events += 1;
        *stats.events_by_type.entry(event.event_type).or_insert(0) += 1;
        stats.last_event_time = Some(event.timestamp);
        Ok(())
    }
}

impl Clone for ClientEventStats {
    fn clone(&self) -> Self {
        Self {
            total_events: self.total_events,
            events_by_type: self.events_by_type.clone(),
            last_event_time: self.last_event_time,
        }
    }
}
