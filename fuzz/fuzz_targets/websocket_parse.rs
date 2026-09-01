#![no_main]

use libfuzzer_sys::fuzz_target;
use media_websocket::{WebSocketConfig, WebSocketFrame, WebSocketRole};

fuzz_target!(|data: &[u8]| {
    let _ = WebSocketFrame::decode(data, WebSocketConfig::default());
    let _ = WebSocketFrame::decode(
        data,
        WebSocketConfig {
            role: WebSocketRole::Client,
            ..WebSocketConfig::default()
        },
    );
});
