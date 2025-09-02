use std::time::Duration;
use std::sync::Arc;
use tokio::time::sleep;
use anyhow::Result;
use fzstream_client::FzStreamClient;
use fzstream_common::EventTypeFilter;
use solana_streamer_sdk::streaming::event_parser::UnifiedEvent;
use solana_streamer_sdk::streaming::event_parser::common::EventType;
use solana_streamer_sdk::streaming::event_parser::protocols::{
    bonk::*, pumpfun::*, pumpswap::*, raydium_amm_v4::*, raydium_clmm::*, raydium_cpmm::*, BlockMetaEvent
};
use solana_streamer_sdk::match_event;

#[tokio::main]
async fn main() -> Result<()> {
    // 设置日志
    env_logger::init();
            
    // 连接到QUIC服务器的参数（不需要http://前缀）
    let server_addr = "127.0.0.1:2222"; 
    let auth_token = "demo_token_12345";   

    let client = Arc::new(tokio::sync::Mutex::new(
        FzStreamClient::builder()
            .server_address(&server_addr)
            .auth_token(auth_token)
            .connection_timeout(Duration::from_secs(5))  // 设置合理的连接超时
            .build()
            .expect("Failed to create client")
    ));
    
    println!("🔗 连接到服务器...");
    {
        let mut client_guard = client.lock().await;
        client_guard.connect().await?;
    }
    println!("✅ 客户端已连接");
    
    // 设置事件过滤器
    let event_filter = EventTypeFilter::allow_only(vec![
        EventType::BlockMeta,
    ]);
    
    println!("📡 开始订阅事件...");
    println!("🎯 设置事件过滤器为: {}", event_filter.get_summary());
    
    // 使用 subscribe_events_with_filter 方法接收事件
    let client_clone = Arc::clone(&client);
    let _client_handle = tokio::spawn(async move {          
        let mut client_guard = client_clone.lock().await;
        println!("🔄 开始订阅事件流...");
        match client_guard.subscribe_events_with_filter(event_filter, create_event_callback()).await {
            Ok(_) => {
                println!("✅ 订阅成功完成");
            }
            Err(e) => {
                eprintln!("❌ 客户端订阅事件失败: {}", e);
                eprintln!("   错误详情: {:?}", e);
            }
        }
        drop(client_guard);  // 释放锁
        
        println!("⚠️ subscribe_events_with_filter 已返回，保持任务运行...");
        
        // 保持连接活跃
        loop {
            sleep(Duration::from_millis(100)).await;
        }
    });
    
    // 给外部服务器一些时间来处理连接
    println!("⏳ 等待与外部服务器建立稳定连接...");
    sleep(Duration::from_millis(2000)).await;
    
    // 设置信号处理器来优雅地处理 Ctrl+C
    let shutdown = tokio::signal::ctrl_c();
    println!("📡 开始接收事件流... (按 Ctrl+C 停止)");
    println!("─────────────────────────────────────────────");
    
    // 等待 Ctrl+C 信号或客户端错误
    tokio::select! {
        _ = shutdown => {
            println!("\n─────────────────────────────────────────────");
            println!("⚠️  接收到 Ctrl+C，正在停止...");
        }
        _ = _client_handle => {
            println!("\n❌ 客户端连接意外中断");
        }
    }
    
    // 正确关闭连接
    println!("🔌 正在关闭连接...");
    {
        let mut client_guard = client.lock().await;
        client_guard.disconnect().await;
    }
    
    println!("✅ 程序已正常退出");
    Ok(())
}

fn create_event_callback() -> impl Fn(Box<dyn UnifiedEvent>) + Clone {
    move |event: Box<dyn UnifiedEvent>| {
        println!("🎉 Event received! Type: {:?}, Signature: {}", event.event_type(), event.signature());
        
        match_event!(event, {
            // -------------------------- block meta -----------------------
            BlockMetaEvent => |e: BlockMetaEvent| {
                println!("BlockMetaEvent: {e:?}");
            },
            // -------------------------- bonk -----------------------
            BonkPoolCreateEvent => |e: BonkPoolCreateEvent| {
                // When using grpc, you can get block_time from each event
                println!("block_time: {:?}, block_time_ms: {:?}", e.metadata.block_time, e.metadata.block_time_ms);
                println!("BonkPoolCreateEvent: {:?}", e.base_mint_param.symbol);
            },
            BonkTradeEvent => |e: BonkTradeEvent| {
                println!("BonkTradeEvent: {e:?}");
            },
            BonkMigrateToAmmEvent => |e: BonkMigrateToAmmEvent| {
                println!("BonkMigrateToAmmEvent: {e:?}");
            },
            BonkMigrateToCpswapEvent => |e: BonkMigrateToCpswapEvent| {
                println!("BonkMigrateToCpswapEvent: {e:?}");
            },
            // -------------------------- pumpfun -----------------------
            PumpFunTradeEvent => |e: PumpFunTradeEvent| {
                println!("PumpFunTradeEvent: {e:?}");
            },
            PumpFunMigrateEvent => |e: PumpFunMigrateEvent| {
                println!("PumpFunMigrateEvent: {e:?}");
            },
            PumpFunCreateTokenEvent => |e: PumpFunCreateTokenEvent| {
                println!("PumpFunCreateTokenEvent: {e:?}");
            },
            // -------------------------- pumpswap -----------------------
            PumpSwapBuyEvent => |e: PumpSwapBuyEvent| {
                println!("Buy event: {e:?}");
            },
            PumpSwapSellEvent => |e: PumpSwapSellEvent| {
                println!("Sell event: {e:?}");
            },
            PumpSwapCreatePoolEvent => |e: PumpSwapCreatePoolEvent| {
                println!("CreatePool event: {e:?}");
            },
            PumpSwapDepositEvent => |e: PumpSwapDepositEvent| {
                println!("Deposit event: {e:?}");
            },
            PumpSwapWithdrawEvent => |e: PumpSwapWithdrawEvent| {
                println!("Withdraw event: {e:?}");
            },
            // -------------------------- raydium_cpmm -----------------------
            RaydiumCpmmSwapEvent => |e: RaydiumCpmmSwapEvent| {
                println!("RaydiumCpmmSwapEvent: {e:?}");
            },
            RaydiumCpmmDepositEvent => |e: RaydiumCpmmDepositEvent| {
                println!("RaydiumCpmmDepositEvent: {e:?}");
            },
            RaydiumCpmmInitializeEvent => |e: RaydiumCpmmInitializeEvent| {
                println!("RaydiumCpmmInitializeEvent: {e:?}");
            },
            RaydiumCpmmWithdrawEvent => |e: RaydiumCpmmWithdrawEvent| {
                println!("RaydiumCpmmWithdrawEvent: {e:?}");
            },
            // -------------------------- raydium_clmm -----------------------
            RaydiumClmmSwapEvent => |e: RaydiumClmmSwapEvent| {
                println!("RaydiumClmmSwapEvent: {e:?}");
            },
            RaydiumClmmSwapV2Event => |e: RaydiumClmmSwapV2Event| {
                println!("RaydiumClmmSwapV2Event: {e:?}");
            },
            RaydiumClmmClosePositionEvent => |e: RaydiumClmmClosePositionEvent| {
                println!("RaydiumClmmClosePositionEvent: {e:?}");
            },
            RaydiumClmmDecreaseLiquidityV2Event => |e: RaydiumClmmDecreaseLiquidityV2Event| {
                println!("RaydiumClmmDecreaseLiquidityV2Event: {e:?}");
            },
            RaydiumClmmCreatePoolEvent => |e: RaydiumClmmCreatePoolEvent| {
                println!("RaydiumClmmCreatePoolEvent: {e:?}");
            },
            RaydiumClmmIncreaseLiquidityV2Event => |e: RaydiumClmmIncreaseLiquidityV2Event| {
                println!("RaydiumClmmIncreaseLiquidityV2Event: {e:?}");
            },
            RaydiumClmmOpenPositionWithToken22NftEvent => |e: RaydiumClmmOpenPositionWithToken22NftEvent| {
                println!("RaydiumClmmOpenPositionWithToken22NftEvent: {e:?}");
            },
            RaydiumClmmOpenPositionV2Event => |e: RaydiumClmmOpenPositionV2Event| {
                println!("RaydiumClmmOpenPositionV2Event: {e:?}");
            },
            // -------------------------- raydium_amm_v4 -----------------------
            RaydiumAmmV4SwapEvent => |e: RaydiumAmmV4SwapEvent| {
                println!("RaydiumAmmV4SwapEvent: {e:?}");
            },
            RaydiumAmmV4DepositEvent => |e: RaydiumAmmV4DepositEvent| {
                println!("RaydiumAmmV4DepositEvent: {e:?}");
            },
            RaydiumAmmV4Initialize2Event => |e: RaydiumAmmV4Initialize2Event| {
                println!("RaydiumAmmV4Initialize2Event: {e:?}");
            },
            RaydiumAmmV4WithdrawEvent => |e: RaydiumAmmV4WithdrawEvent| {
                println!("RaydiumAmmV4WithdrawEvent: {e:?}");
            },
            RaydiumAmmV4WithdrawPnlEvent => |e: RaydiumAmmV4WithdrawPnlEvent| {
                println!("RaydiumAmmV4WithdrawPnlEvent: {e:?}");
            },
            // -------------------------- account -----------------------
            BonkPoolStateAccountEvent => |e: BonkPoolStateAccountEvent| {
                println!("BonkPoolStateAccountEvent: {e:?}");
            },
            BonkGlobalConfigAccountEvent => |e: BonkGlobalConfigAccountEvent| {
                println!("BonkGlobalConfigAccountEvent: {e:?}");
            },
            BonkPlatformConfigAccountEvent => |e: BonkPlatformConfigAccountEvent| {
                println!("BonkPlatformConfigAccountEvent: {e:?}");
            },
            PumpSwapGlobalConfigAccountEvent => |e: PumpSwapGlobalConfigAccountEvent| {
                println!("PumpSwapGlobalConfigAccountEvent: {e:?}");
            },
            PumpSwapPoolAccountEvent => |e: PumpSwapPoolAccountEvent| {
                println!("PumpSwapPoolAccountEvent: {e:?}");
            },
            PumpFunBondingCurveAccountEvent => |e: PumpFunBondingCurveAccountEvent| {
                println!("PumpFunBondingCurveAccountEvent: {e:?}");
            },
            PumpFunGlobalAccountEvent => |e: PumpFunGlobalAccountEvent| {
                println!("PumpFunGlobalAccountEvent: {e:?}");
            },
            RaydiumAmmV4AmmInfoAccountEvent => |e: RaydiumAmmV4AmmInfoAccountEvent| {
                println!("RaydiumAmmV4AmmInfoAccountEvent: {e:?}");
            },
            RaydiumClmmAmmConfigAccountEvent => |e: RaydiumClmmAmmConfigAccountEvent| {
                println!("RaydiumClmmAmmConfigAccountEvent: {e:?}");
            },
            RaydiumClmmPoolStateAccountEvent => |e: RaydiumClmmPoolStateAccountEvent| {
                println!("RaydiumClmmPoolStateAccountEvent: {e:?}");
            },
            RaydiumClmmTickArrayStateAccountEvent => |e: RaydiumClmmTickArrayStateAccountEvent| {
                println!("RaydiumClmmTickArrayStateAccountEvent: {e:?}");
            },
            RaydiumCpmmAmmConfigAccountEvent => |e: RaydiumCpmmAmmConfigAccountEvent| {
                println!("RaydiumCpmmAmmConfigAccountEvent: {e:?}");
            },
            RaydiumCpmmPoolStateAccountEvent => |e: RaydiumCpmmPoolStateAccountEvent| {
                println!("RaydiumCpmmPoolStateAccountEvent: {e:?}");
            },
        });
    }
}