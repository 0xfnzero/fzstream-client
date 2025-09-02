use std::time::Duration;
use tokio::time::sleep;
use fzstream_common::{EventMessage, EventType, SerializationProtocol, CompressionLevel};
use fzstream_client::{FzStreamClient, StreamClientConfig};
use fzstream_server::{create_server, ServerConfig};
use solana_streamer_sdk::streaming::event_parser::core::UnifiedEvent;
use solana_streamer_sdk::streaming::event_parser::protocols::{
    bonk, pumpfun, pumpswap, raydium_amm_v4, raydium_clmm, raydium_cpmm, BlockMetaEvent
};
use serde::{Serialize, Deserialize};
use solana_streamer_sdk::match_event;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 设置日志
    env_logger::init();
    
    println!("🚀 开始基本 QUIC 连接测试...");
    
    // 启动 QUIC 服务器
    let server_addr = "127.0.0.1:2222";
    // 创建并启动客户端
    println!("🔌 创建 QUIC 客户端...");
    // let mut config = StreamClientConfig::default();
    // config.auth_token = Some("demo_token_12345".to_string()); // 添加测试认证令牌
    // config.endpoint = server_addr.to_string();

    let mut client = FzStreamClient::builder()
    .server_address(&server_addr)
    .auth_token("demo_token_12345")
    .ultra_low_latency_mode(true)
    .build()
    .expect("Failed to create client");
    
    // let mut client = FzStreamClient::with_config(config);
    
    // 连接到服务器
    println!("🔗 连接到服务器...");
    client.connect().await?;
    println!("✅ 客户端已连接");
    
    // 使用 subscribe_events 方法接收事件
    println!("📡 开始订阅事件...");
    let event_received = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let event_received_clone = event_received.clone();
    
    let client_handle = tokio::spawn(async move {
        if let Err(e) = client.subscribe_events(create_event_callback(event_received_clone)).await {
            eprintln!("❌ 客户端订阅事件失败: {}", e);
        }
        
        // 保持连接活跃
        loop {
            sleep(Duration::from_millis(100)).await;
        }
    });
    
    // // 等待事件被接收
    // println!("⏳ 等待事件接收...");
    // let mut attempts = 0;
    // while !event_received.load(std::sync::atomic::Ordering::Relaxed) && attempts < 30 {
    //     sleep(Duration::from_millis(1000)).await;
    //     attempts += 1;
    //     println!("等待中... ({}/30)", attempts);
    // }
    
    // if event_received.load(std::sync::atomic::Ordering::Relaxed) {
    //     println!("🎉 测试成功! 客户端成功接收到事件数据!");
    // } else {
    //     println!("❌ 测试失败! 客户端未能接收到事件数据");
    //     println!("💡 这可能是因为服务器和客户端之间的 QUIC 连接还没有完全建立");
    //     println!("💡 或者事件广播机制还没有实现");
    // }
    
    // // 清理
    // println!("🛑 清理资源...");
    // client_handle.abort();
    // // server_handle.abort();
    
    println!("✅ 测试完成!");
    Ok(())
}

fn create_event_callback(event_received: std::sync::Arc<std::sync::atomic::AtomicBool>) -> impl Fn(Box<dyn UnifiedEvent>) + Clone {
    let event_received = event_received.clone();
    move |event: Box<dyn UnifiedEvent>| {
        println!("🎉 Event received! Type: {:?}, Signature: {}", event.event_type(), event.signature());
        event_received.store(true, std::sync::atomic::Ordering::Relaxed);
        
        match_event!(event, {
            // -------------------------- block meta -----------------------
            BlockMetaEvent => |e: BlockMetaEvent| {
                println!("BlockMetaEvent: {e:?}");
            },
            // -------------------------- bonk -----------------------
            BonkPoolCreateEvent => |e: bonk::BonkPoolCreateEvent| {
                // When using grpc, you can get block_time from each event
                println!("block_time: {:?}, block_time_ms: {:?}", e.metadata.block_time, e.metadata.block_time_ms);
                println!("BonkPoolCreateEvent: {:?}", e.base_mint_param.symbol);
            },
            BonkTradeEvent => |e: bonk::BonkTradeEvent| {
                println!("BonkTradeEvent: {e:?}");
            },
            BonkMigrateToAmmEvent => |e: bonk::BonkMigrateToAmmEvent| {
                println!("BonkMigrateToAmmEvent: {e:?}");
            },
            BonkMigrateToCpswapEvent => |e: bonk::BonkMigrateToCpswapEvent| {
                println!("BonkMigrateToCpswapEvent: {e:?}");
            },
            // -------------------------- pumpfun -----------------------
            PumpFunTradeEvent => |e: pumpfun::PumpFunTradeEvent| {
                println!("PumpFunTradeEvent: {e:?}");
            },
            PumpFunMigrateEvent => |e: pumpfun::PumpFunMigrateEvent| {
                println!("PumpFunMigrateEvent: {e:?}");
            },
            PumpFunCreateTokenEvent => |e: pumpfun::PumpFunCreateTokenEvent| {
                println!("PumpFunCreateTokenEvent: {e:?}");
            },
            // -------------------------- pumpswap -----------------------
            PumpSwapBuyEvent => |e: pumpswap::PumpSwapBuyEvent| {
                println!("Buy event: {e:?}");
            },
            PumpSwapSellEvent => |e: pumpswap::PumpSwapSellEvent| {
                println!("Sell event: {e:?}");
            },
            PumpSwapCreatePoolEvent => |e: pumpswap::PumpSwapCreatePoolEvent| {
                println!("CreatePool event: {e:?}");
            },
            PumpSwapDepositEvent => |e: pumpswap::PumpSwapDepositEvent| {
                println!("Deposit event: {e:?}");
            },
            PumpSwapWithdrawEvent => |e: pumpswap::PumpSwapWithdrawEvent| {
                println!("Withdraw event: {e:?}");
            },
            // -------------------------- raydium_cpmm -----------------------
            RaydiumCpmmSwapEvent => |e: raydium_cpmm::RaydiumCpmmSwapEvent| {
                println!("RaydiumCpmmSwapEvent: {e:?}");
            },
            RaydiumCpmmDepositEvent => |e: raydium_cpmm::RaydiumCpmmDepositEvent| {
                println!("RaydiumCpmmDepositEvent: {e:?}");
            },
            RaydiumCpmmInitializeEvent => |e: raydium_cpmm::RaydiumCpmmInitializeEvent| {
                println!("RaydiumCpmmInitializeEvent: {e:?}");
            },
            RaydiumCpmmWithdrawEvent => |e: raydium_cpmm::RaydiumCpmmWithdrawEvent| {
                println!("RaydiumCpmmWithdrawEvent: {e:?}");
            },
            // -------------------------- raydium_clmm -----------------------
            RaydiumClmmSwapEvent => |e: raydium_clmm::RaydiumClmmSwapEvent| {
                println!("RaydiumClmmSwapEvent: {e:?}");
            },
            RaydiumClmmSwapV2Event => |e: raydium_clmm::RaydiumClmmSwapV2Event| {
                println!("RaydiumClmmSwapV2Event: {e:?}");
            },
            RaydiumClmmClosePositionEvent => |e: raydium_clmm::RaydiumClmmClosePositionEvent| {
                println!("RaydiumClmmClosePositionEvent: {e:?}");
            },
            RaydiumClmmDecreaseLiquidityV2Event => |e: raydium_clmm::RaydiumClmmDecreaseLiquidityV2Event| {
                println!("RaydiumClmmDecreaseLiquidityV2Event: {e:?}");
            },
            RaydiumClmmCreatePoolEvent => |e: raydium_clmm::RaydiumClmmCreatePoolEvent| {
                println!("RaydiumClmmCreatePoolEvent: {e:?}");
            },
            RaydiumClmmIncreaseLiquidityV2Event => |e: raydium_clmm::RaydiumClmmIncreaseLiquidityV2Event| {
                println!("RaydiumClmmIncreaseLiquidityV2Event: {e:?}");
            },
            RaydiumClmmOpenPositionWithToken22NftEvent => |e: raydium_clmm::RaydiumClmmOpenPositionWithToken22NftEvent| {
                println!("RaydiumClmmOpenPositionWithToken22NftEvent: {e:?}");
            },
            RaydiumClmmOpenPositionV2Event => |e: raydium_clmm::RaydiumClmmOpenPositionV2Event| {
                println!("RaydiumClmmOpenPositionV2Event: {e:?}");
            },
            // -------------------------- raydium_amm_v4 -----------------------
            RaydiumAmmV4SwapEvent => |e: raydium_amm_v4::RaydiumAmmV4SwapEvent| {
                println!("RaydiumAmmV4SwapEvent: {e:?}");
            },
            RaydiumAmmV4DepositEvent => |e: raydium_amm_v4::RaydiumAmmV4DepositEvent| {
                println!("RaydiumAmmV4DepositEvent: {e:?}");
            },
            RaydiumAmmV4Initialize2Event => |e: raydium_amm_v4::RaydiumAmmV4Initialize2Event| {
                println!("RaydiumAmmV4Initialize2Event: {e:?}");
            },
            RaydiumAmmV4WithdrawEvent => |e: raydium_amm_v4::RaydiumAmmV4WithdrawEvent| {
                println!("RaydiumAmmV4WithdrawEvent: {e:?}");
            },
            RaydiumAmmV4WithdrawPnlEvent => |e: raydium_amm_v4::RaydiumAmmV4WithdrawPnlEvent| {
                println!("RaydiumAmmV4WithdrawPnlEvent: {e:?}");
            },
            // -------------------------- account -----------------------
            BonkPoolStateAccountEvent => |e: bonk::BonkPoolStateAccountEvent| {
                println!("BonkPoolStateAccountEvent: {e:?}");
            },
            BonkGlobalConfigAccountEvent => |e: bonk::BonkGlobalConfigAccountEvent| {
                println!("BonkGlobalConfigAccountEvent: {e:?}");
            },
            BonkPlatformConfigAccountEvent => |e: bonk::BonkPlatformConfigAccountEvent| {
                println!("BonkPlatformConfigAccountEvent: {e:?}");
            },
            PumpSwapGlobalConfigAccountEvent => |e: pumpswap::PumpSwapGlobalConfigAccountEvent| {
                println!("PumpSwapGlobalConfigAccountEvent: {e:?}");
            },
            PumpSwapPoolAccountEvent => |e: pumpswap::PumpSwapPoolAccountEvent| {
                println!("PumpSwapPoolAccountEvent: {e:?}");
            },
            PumpFunBondingCurveAccountEvent => |e: pumpfun::PumpFunBondingCurveAccountEvent| {
                println!("PumpFunBondingCurveAccountEvent: {e:?}");
            },
            PumpFunGlobalAccountEvent => |e: pumpfun::PumpFunGlobalAccountEvent| {
                println!("PumpFunGlobalAccountEvent: {e:?}");
            },
            RaydiumAmmV4AmmInfoAccountEvent => |e: raydium_amm_v4::RaydiumAmmV4AmmInfoAccountEvent| {
                println!("RaydiumAmmV4AmmInfoAccountEvent: {e:?}");
            },
            RaydiumClmmAmmConfigAccountEvent => |e: raydium_clmm::RaydiumClmmAmmConfigAccountEvent| {
                println!("RaydiumClmmAmmConfigAccountEvent: {e:?}");
            },
            RaydiumClmmPoolStateAccountEvent => |e: raydium_clmm::RaydiumClmmPoolStateAccountEvent| {
                println!("RaydiumClmmPoolStateAccountEvent: {e:?}");
            },
            RaydiumClmmTickArrayStateAccountEvent => |e: raydium_clmm::RaydiumClmmTickArrayStateAccountEvent| {
                println!("RaydiumClmmTickArrayStateAccountEvent: {e:?}");
            },
            RaydiumCpmmAmmConfigAccountEvent => |e: raydium_cpmm::RaydiumCpmmAmmConfigAccountEvent| {
                println!("RaydiumCpmmAmmConfigAccountEvent: {e:?}");
            },
            RaydiumCpmmPoolStateAccountEvent => |e: raydium_cpmm::RaydiumCpmmPoolStateAccountEvent| {
                println!("RaydiumCpmmPoolStateAccountEvent: {e:?}");
            },
        });
    }
}
