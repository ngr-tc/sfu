use retty::bootstrap::bootstrap_udp_server::BootstrapUdpServer;
use retty::channel::handler::{
    Handler, InboundHandler, InboundHandlerContext, InboundHandlerGeneric, OutboundHandler,
    OutboundHandlerGeneric,
};
use retty::channel::pipeline::Pipeline;
use retty::codec::byte_to_message_decoder::line_based_frame_decoder::{
    LineBasedFrameDecoder, TerminatorType,
};
use retty::codec::byte_to_message_decoder::tagged::TaggedByteToMessageCodec;
use retty::codec::string_codec::tagged::{TaggedString, TaggedStringCodec};
use retty::runtime::default_runtime;
use retty::transport::async_transport_udp::AsyncTransportUdp;
use retty::transport::{AsyncTransportWrite, TransportContext};

use async_trait::async_trait;
use log::{error, info};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex};

pub mod data;
pub mod media;
pub mod session;

struct TaggedEchoDecoder;
struct TaggedEchoEncoder;
struct TaggedEchoHandler {
    decoder: TaggedEchoDecoder,
    encoder: TaggedEchoEncoder,
}

impl TaggedEchoHandler {
    fn new() -> Self {
        TaggedEchoHandler {
            decoder: TaggedEchoDecoder,
            encoder: TaggedEchoEncoder,
        }
    }
}

#[async_trait]
impl InboundHandlerGeneric<TaggedString> for TaggedEchoDecoder {
    async fn read_generic(&mut self, ctx: &mut InboundHandlerContext, msg: &mut TaggedString) {
        println!(
            "handling {} from {:?}",
            msg.message, msg.transport.peer_addr
        );
        if msg.message == "bye" {
            ctx.fire_close().await;
        } else {
            ctx.fire_write(&mut TaggedString {
                transport: msg.transport,
                message: format!("{}\r\n", msg.message),
            })
            .await;
        }
    }
    async fn read_eof_generic(&mut self, ctx: &mut InboundHandlerContext) {
        ctx.fire_close().await;
    }
}

impl OutboundHandlerGeneric<TaggedString> for TaggedEchoEncoder {}

impl Handler for TaggedEchoHandler {
    fn id(&self) -> String {
        "TaggedEcho Handler".to_string()
    }

    fn split(
        self,
    ) -> (
        Arc<Mutex<dyn InboundHandler>>,
        Arc<Mutex<dyn OutboundHandler>>,
    ) {
        let decoder: Box<dyn InboundHandlerGeneric<TaggedString>> = Box::new(self.decoder);
        let encoder: Box<dyn OutboundHandlerGeneric<TaggedString>> = Box::new(self.encoder);
        (Arc::new(Mutex::new(decoder)), Arc::new(Mutex::new(encoder)))
    }
}

/// udp_rtc_server starts a RTC Server on top of UDP
pub async fn udp_rtc_server(
    host: String,
    port: u16,
    _sdp_rx: mpsc::Receiver<String>,
    mut cancel_rx: broadcast::Receiver<()>,
) -> broadcast::Receiver<()> {
    let (done_tx, done_rx) = broadcast::channel(1);
    tokio::spawn(async move {
        let mut bootstrap = BootstrapUdpServer::new(default_runtime().unwrap());
        let err_done_tx = done_tx.clone();
        if let Err(err) = bootstrap
            .pipeline(Box::new(
                move |sock: Box<dyn AsyncTransportWrite + Send + Sync>| {
                    let mut pipeline = Pipeline::new(TransportContext {
                        local_addr: sock.local_addr().unwrap(),
                        peer_addr: sock.peer_addr().ok(),
                    });

                    let async_transport_handler = AsyncTransportUdp::new(sock);
                    let line_based_frame_decoder_handler = TaggedByteToMessageCodec::new(Box::new(
                        LineBasedFrameDecoder::new(8192, true, TerminatorType::BOTH),
                    ));
                    let string_codec_handler = TaggedStringCodec::new();
                    let echo_handler = TaggedEchoHandler::new();

                    pipeline.add_back(async_transport_handler);
                    pipeline.add_back(line_based_frame_decoder_handler);
                    pipeline.add_back(string_codec_handler);
                    pipeline.add_back(echo_handler);

                    Box::pin(async move { pipeline.finalize().await })
                },
            ))
            .bind(format!("{}:{}", host, port))
            .await
        {
            error!("udp_rtc_server error: {}", err);
            let _ = err_done_tx.send(());
        }

        tokio::select! {
            _ = cancel_rx.recv() => {
                bootstrap.stop().await;
                info!("udp_rtc_server receives cancel signal");
                let _ = done_tx.send(());
            }
        };
    });

    done_rx
}