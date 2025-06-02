use quinn::{Connection, TransportConfig};

pub const ALPN_TPU_FORWARDPROXY_PROTOCOL_ID: &[u8] = b"solana-tpu-forward-proxy";


//  stable_id 140266619216912, rtt=2.156683ms,
// stats FrameStats { ACK: 3, CONNECTION_CLOSE: 0, CRYPTO: 3,
// DATA_BLOCKED: 0, DATAGRAM: 0, HANDSHAKE_DONE: 1, MAX_DATA: 0,
// MAX_STREAM_DATA: 1, MAX_STREAMS_BIDI: 0, MAX_STREAMS_UNI: 0, NEW_CONNECTION_ID: 4,
// NEW_TOKEN: 0, PATH_CHALLENGE: 0, PATH_RESPONSE: 0, PING: 0, RESET_STREAM: 0,
// RETIRE_CONNECTION_ID: 1, STREAM_DATA_BLOCKED: 0, STREAMS_BLOCKED_BIDI: 0,
// STREAMS_BLOCKED_UNI: 0, STOP_SENDING: 0, STREAM: 0 }
pub fn connection_stats(connection: &Connection) -> String {
    format!(
        "stable_id {} stats {:?}, rtt={:?}",
        connection.stable_id(),
        connection.stats().frame_rx,
        connection.stats().path.rtt
    )
}
/// env flag to optionally disable GSO (generic segmentation offload) on environments where Quinn cannot detect it properly
/// see https://github.com/quinn-rs/quinn/pull/1671
pub fn apply_gso_workaround(tc: &mut TransportConfig) {
    if disable_gso() {
        tc.enable_segmentation_offload(false);
    }
}


/// note: true means that quinn's heuristic for GSO detection is used to decide if GSO is used
fn disable_gso() -> bool {
    std::env::var("DISABLE_GSO")
        .unwrap_or("false".to_string())
        .parse::<bool>()
        .expect("flag must be true or false")
}
