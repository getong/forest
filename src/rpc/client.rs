use std::fmt::Display;
use std::time::Duration;

use http0::{header, HeaderMap, HeaderValue};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::core::params::{ArrayParams, ObjectParams};
use jsonrpsee::core::ClientError;
use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use serde::de::DeserializeOwned;
use tracing::debug;
use url::Url;

pub struct Client {
    inner: ClientInner,
}

impl Client {
    pub async fn from_multiaddr_with_path(
        multiaddr: &Multiaddr,
        path: impl Display,
        token: impl Into<Option<String>>,
        timeout: Duration,
    ) -> Result<Self, ClientError> {
        let Some(mut it) = multiaddr2url(&multiaddr) else {
            return Err(ClientError::Custom(String::from(
                "Couldn't convert multiaddr to URL",
            )));
        };
        it.set_path(&path.to_string());
        Self::from_url(it, token, timeout).await
    }
    pub async fn from_url(
        url: Url,
        token: impl Into<Option<String>>,
        timeout: Duration,
    ) -> Result<Self, ClientError> {
        let headers = match token.into() {
            Some(it) => HeaderMap::from_iter([(
                header::AUTHORIZATION,
                match HeaderValue::try_from(it) {
                    Ok(it) => it,
                    Err(e) => {
                        return Err(ClientError::Custom(format!(
                            "Invalid authorization token: {e}"
                        )))
                    }
                },
            )]),
            None => Default::default(),
        };
        let inner = match url.scheme() {
            "ws" | "wss" => ClientInner::Ws(
                jsonrpsee::ws_client::WsClientBuilder::new()
                    .set_headers(headers)
                    .request_timeout(timeout)
                    .build(&url)
                    .await?,
            ),
            "http" | "https" => ClientInner::Https(
                jsonrpsee::http_client::HttpClientBuilder::new()
                    .set_headers(headers)
                    .request_timeout(timeout)
                    .build(&url)?,
            ),
            it => return Err(ClientError::Custom(format!("Unsupported URL scheme: {it}"))),
        };
        Ok(Self { inner })
    }
    pub async fn call<T: crate::lotus_json::HasLotusJson + std::fmt::Debug>(
        &self,
        req: crate::rpc_client::RpcRequest<T>,
    ) -> Result<T, ClientError> {
        let crate::rpc_client::RpcRequest {
            method_name,
            params,
            result_type,
            rpc_endpoint,
            timeout,
        } = req;
        let result_or_timeout = tokio::time::timeout(
            timeout,
            match params {
                serde_json::Value::Null => {
                    self.request::<T::LotusJson, _>(method_name, ArrayParams::new())
                }
                serde_json::Value::Array(it) => {
                    let mut params = ArrayParams::new();
                    for param in it {
                        params.insert(param)?
                    }
                    self.request(method_name, params)
                }
                serde_json::Value::Object(it) => {
                    let mut params = ObjectParams::new();
                    for (name, param) in it {
                        params.insert(&name, param)?
                    }
                    self.request(method_name, params)
                }
                prim @ (serde_json::Value::Bool(_)
                | serde_json::Value::Number(_)
                | serde_json::Value::String(_)) => {
                    return Err(ClientError::Custom(format!(
                        "invalid parameter type: {}",
                        prim
                    )))
                }
            },
        )
        .await;
        let result = match result_or_timeout {
            Ok(Ok(it)) => Ok(T::from_lotus_json(it)),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(ClientError::RequestTimeout),
        };
        debug!(?result);
        result
    }
}

enum ClientInner {
    Ws(jsonrpsee::ws_client::WsClient),
    Https(jsonrpsee::http_client::HttpClient),
}

#[async_trait::async_trait]
impl jsonrpsee::core::client::ClientT for Client {
    async fn notification<P: jsonrpsee::core::traits::ToRpcParams + Send>(
        &self,
        method: &str,
        params: P,
    ) -> Result<(), jsonrpsee::core::ClientError> {
        match &self.inner {
            ClientInner::Ws(it) => it.notification(method, params).await,
            ClientInner::Https(it) => it.notification(method, params).await,
        }
    }
    async fn request<R: DeserializeOwned, P: jsonrpsee::core::traits::ToRpcParams + Send>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R, jsonrpsee::core::ClientError> {
        match &self.inner {
            ClientInner::Ws(it) => it.request(method, params).await,
            ClientInner::Https(it) => it.request(method, params).await,
        }
    }
    async fn batch_request<'a, R: DeserializeOwned + 'a + std::fmt::Debug>(
        &self,
        batch: jsonrpsee::core::params::BatchRequestBuilder<'a>,
    ) -> Result<jsonrpsee::core::client::BatchResponse<'a, R>, jsonrpsee::core::ClientError> {
        match &self.inner {
            ClientInner::Ws(it) => it.batch_request(batch).await,
            ClientInner::Https(it) => it.batch_request(batch).await,
        }
    }
}

fn multiaddr2url(m: &Multiaddr) -> Option<Url> {
    let mut components = m.iter().peekable();
    let host = match components.next()? {
        Protocol::Dns4(it) | Protocol::Dns6(it) | Protocol::Dnsaddr(it) => it.to_string(),
        Protocol::Ip4(it) => it.to_string(),
        Protocol::Ip6(it) => it.to_string(),
        _ => return None,
    };
    let port = components
        .next_if(|it| matches!(it, Protocol::Tcp(_)))
        .map(|it| match it {
            Protocol::Tcp(port) => port,
            _ => unreachable!(),
        });
    // ENHANCEMENT: could recognise `Tcp/443/Tls` as `https`
    let scheme = match components.next()? {
        Protocol::Http => "http",
        Protocol::Https => "https",
        Protocol::Ws(it) if it == "/" => "ws",
        Protocol::Wss(it) if it == "/" => "wss",
        _ => return None,
    };
    let None = components.next() else { return None };
    let parse_me = match port {
        Some(port) => format!("{}://{}:{}", scheme, host, port),
        None => format!("{}://{}", scheme, host),
    };
    parse_me.parse().ok()
}
