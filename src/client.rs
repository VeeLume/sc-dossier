//! gRPC channel construction and the bearer-JWT interceptor.

use std::time::Duration;

use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Status};

use crate::error::{Error, Result};

/// Open a TLS gRPC channel to `endpoint`.
///
/// `user_agent` is the caller-supplied gRPC user-agent; tonic appends
/// ` tonic/<version>` to it on the wire. Keepalive: 120s interval, 20s timeout,
/// permitted while idle.
pub async fn connect(endpoint: &str, user_agent: &str) -> Result<Channel> {
    let tls = ClientTlsConfig::new().with_webpki_roots();
    let channel = Endpoint::from_shared(endpoint.to_string())?
        .user_agent(user_agent)?
        .tls_config(tls)?
        .http2_keep_alive_interval(Duration::from_secs(120))
        .keep_alive_timeout(Duration::from_secs(20))
        .keep_alive_while_idle(true)
        .connect()
        .await?;
    Ok(channel)
}

/// Interceptor attaching `authorization: Bearer <jwt>` to every request.
#[derive(Clone)]
pub struct BearerAuth {
    header: MetadataValue<Ascii>,
}

impl BearerAuth {
    /// Construct from a raw JWT (no `Bearer ` prefix).
    pub fn new(jwt: &str) -> Result<Self> {
        let header = MetadataValue::try_from(format!("Bearer {jwt}"))
            .map_err(|_| Error::Invalid("JWT contains invalid header characters".into()))?;
        Ok(Self { header })
    }
}

impl Interceptor for BearerAuth {
    fn call(&mut self, mut request: Request<()>) -> std::result::Result<Request<()>, Status> {
        request.metadata_mut().insert("authorization", self.header.clone());
        Ok(request)
    }
}
