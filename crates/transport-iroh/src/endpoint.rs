//! `transport.IrohConfig`, `BindIroh` and the dial phases (`transport/iroh.go:22-406`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::slog::Logger;
use dstore_transport::addr::{GoRelayUrl, GoTransportAddr};
use dstore_transport::{Conn, Endpoint, NodeId, TransportError};
use tokio_util::sync::CancellationToken;

use crate::mdns::MdnsResolver;

/// The relay configuration of an enabled relay mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayChoice {
    Default,
    Custom(GoRelayUrl),
}

/// cmd/dstore `relayModeOf`: no_relay → None; url → Custom (a parse error is returned verbatim); else
/// Default.
pub fn relay_mode_of(url: &str, no_relay: bool) -> Result<Option<RelayChoice>, String> {
    todo!()
}

/// `transport.IrohConfig`.
pub struct IrohConfig {
    pub secret_key: iroh::SecretKey,
    /// Client: empty.
    pub alpns: Vec<String>,
    /// None = relays disabled.
    pub relay: Option<RelayChoice>,
    /// Node-side.
    pub advertise: Option<Vec<std::net::SocketAddr>>,
    /// Tests / node.
    pub bind_addr: Option<std::net::SocketAddr>,
    pub loopback: bool,
    /// None → `DIRECT_TIMEOUT`.
    pub direct_timeout: Option<Duration>,
    pub discover: bool,
    /// Must be false in v1.
    pub announce: bool,
    /// None → `Logger::default_logger()`.
    pub logger: Option<Logger>,
}

/// `transport.IrohEndpoint`.
pub struct IrohEndpoint {
    ep: iroh::Endpoint,
    id: NodeId,
    relay: Option<RelayChoice>,
    loopback: bool,
    direct_timeout: Duration,
    discover: bool,
    logger: Logger,
    addrs: Mutex<Vec<String>>,
    mdns: Option<Arc<MdnsResolver>>,
    bg: CancellationToken,
}

/// `transport.BindIroh`.
pub async fn bind_iroh(ctx: &Ctx, cfg: IrohConfig) -> Result<Arc<IrohEndpoint>, TransportError> {
    todo!()
}

/// dial = direct phase, relay+direct phase, discover_dial (transport §4.5).
#[async_trait::async_trait]
impl Endpoint for IrohEndpoint {
    fn id(&self) -> NodeId {
        todo!()
    }

    async fn dial(
        &self,
        ctx: &Ctx,
        id: NodeId,
        addrs: Vec<String>,
        alpn: &str,
    ) -> Result<Arc<dyn Conn>, TransportError> {
        todo!()
    }

    async fn accept(&self, ctx: &Ctx) -> Result<Arc<dyn Conn>, TransportError> {
        todo!()
    }

    fn addrs(&self) -> Vec<String> {
        todo!()
    }

    async fn close(&self) {
        todo!()
    }
}

impl IrohEndpoint {
    /// Go `IrohEndpoint.Raw`.
    pub fn raw(&self) -> &iroh::Endpoint {
        todo!()
    }

    /// `Endpoint::close()` bounded to `d`.
    pub async fn close_bounded(&self, d: Duration) {
        todo!()
    }
}

/// A fresh ephemeral identity.
pub fn generate_secret_key() -> iroh::SecretKey {
    todo!()
}

/// GoTransportAddr → iroh candidate; None when it cannot be converted (unparseable relay URL, unknown
/// zone name).
pub fn to_iroh_addr(a: &GoTransportAddr) -> Option<iroh::TransportAddr> {
    todo!()
}

/// The Go form of a relay URL as nodes publish it ("relay:" prefix added by the caller).
pub fn go_relay_string(u: &iroh::RelayUrl) -> String {
    todo!()
}
