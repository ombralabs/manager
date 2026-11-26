use super::http::{self, MAX_HEAD, join_host_port, parse_status};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use std::fmt;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_socks::tcp::Socks5Stream;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamKind { Http, Https, Socks5, Direct }

impl From<crate::store::proxies::ProxyKind> for UpstreamKind {
    fn from(kind: crate::store::proxies::ProxyKind) -> Self {
        match kind { crate::store::proxies::ProxyKind::Http => Self::Http, crate::store::proxies::ProxyKind::Https => Self::Https, crate::store::proxies::ProxyKind::Socks5 => Self::Socks5 }
    }
}

#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials").field("username", &"***").field("password", &"***").finish()
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamConfig {
    /// Номер записи прокси — только он попадает в журнал.
    pub proxy_id: i64,
    pub kind: UpstreamKind,
    pub host: String,
    pub port: u16,
    pub credentials: Option<Credentials>,
    /// Корни доверия для HTTPS-прокси; `None` — корни Mozilla (webpki-roots).
    pub tls_roots: Option<Arc<rustls::RootCertStore>>,
}

impl UpstreamConfig {
    pub fn direct() -> Self { Self { proxy_id: 0, kind: UpstreamKind::Direct, host: String::new(), port: 0, credentials: None, tls_roots: None } }
    pub fn basic_auth(&self) -> Option<String> {
        self.credentials.as_ref().map(|c| format!("Basic {}", B64.encode(format!("{}:{}", c.username, c.password))))
    }
}

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
pub type BoxedIo = Box<dyn Io>;

#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    #[error("соединение с адресом: {0}")]
    Target(String),
    #[error("Время подключения к прокси истекло")]
    Timeout,
    #[error("прокси: {0}")]
    Io(#[from] io::Error),
    #[error("прокси ответил {0}")]
    Status(u16),
    #[error("прокси: неверный ответ")]
    Protocol,
    #[error("TLS до прокси: {0}")]
    Tls(String),
    #[error("SOCKS5: {0}")]
    Socks(String),
    #[error("SOCKS5: цель недоступна: {0}")]
    SocksTarget(String),
}

impl UpstreamError {
    /// Сломан сам прокси (а не сайт за ним): это считается для «шлюз недоступен» и класса ошибки геоданных.
    /// 403 на CONNECT — частый ответ прокси с неверной авторизацией вместо 407; SOCKS «general failure» — отказ самого прокси.
    pub fn is_proxy_failure(&self) -> bool {
        match self {
            UpstreamError::Status(code) => matches!(*code, 403 | 407),
            UpstreamError::SocksTarget(_) | UpstreamError::Target(_) => false,
            _ => true,
        }
    }
}

fn tls_config(cfg: &UpstreamConfig) -> Arc<rustls::ClientConfig> {
    client_tls(cfg.tls_roots.as_deref())
}

/// Настройки клиента TLS: корни `roots`, а без них — корни Mozilla (webpki-roots).
pub fn client_tls(roots: Option<&rustls::RootCertStore>) -> Arc<rustls::ClientConfig> {
    let roots = match roots {
        Some(roots) => roots.clone(),
        None => {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            roots
        }
    };
    // Провайдер задан явно: если другая зависимость включит aws-lc-rs, `ClientConfig::builder()` начнёт паниковать.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring поддерживает TLS 1.2 и 1.3")
        .with_root_certificates(roots)
        .with_no_client_auth();
    Arc::new(config)
}

/// Соединение с самим прокси: TCP, а для HTTPS — ещё и TLS.
pub async fn open_proxy_stream(cfg: &UpstreamConfig) -> Result<BoxedIo, UpstreamError> {
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((cfg.host.as_str(), cfg.port)))
        .await
        .map_err(|_| UpstreamError::Timeout)??;
    match cfg.kind {
        UpstreamKind::Http => Ok(Box::new(tcp)),
        UpstreamKind::Https => {
            let name = rustls::pki_types::ServerName::try_from(cfg.host.clone())
                .map_err(|e| UpstreamError::Tls(e.to_string()))?;
            let connector = tokio_rustls::TlsConnector::from(tls_config(cfg));
            let tls = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(name, tcp))
                .await
                .map_err(|_| UpstreamError::Timeout)?
                .map_err(|e| UpstreamError::Tls(e.to_string()))?;
            Ok(Box::new(tls))
        }
        UpstreamKind::Socks5 | UpstreamKind::Direct => Err(UpstreamError::Protocol),
    }
}

/// Туннель до `host:port` через прокси. Имя цели резолвит прокси.
pub async fn connect(cfg: &UpstreamConfig, host: &str, port: u16) -> Result<BoxedIo, UpstreamError> {
    match cfg.kind {
        UpstreamKind::Direct => {
            let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port))).await
                .map_err(|_| UpstreamError::Target("timeout".into()))?.map_err(|error| UpstreamError::Target(error.to_string()))?;
            Ok(Box::new(stream))
        }
        UpstreamKind::Http | UpstreamKind::Https => {
            let mut stream = open_proxy_stream(cfg).await?;
            let authority = join_host_port(host, port);
            let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
            if let Some(auth) = cfg.basic_auth() {
                request.push_str(&format!("Proxy-Authorization: {auth}\r\n"));
            }
            request.push_str("\r\n");
            stream.write_all(request.as_bytes()).await?;
            let head = tokio::time::timeout(CONNECT_TIMEOUT, read_head(&mut stream))
                .await
                .map_err(|_| UpstreamError::Timeout)??;
            let (code, len) = parse_status(&head).ok_or(UpstreamError::Protocol)?;
            if code != 200 {
                return Err(UpstreamError::Status(code));
            }
            if head.len() != len {
                return Err(UpstreamError::Protocol);
            }
            Ok(stream)
        }
        UpstreamKind::Socks5 => {
            let proxy = (cfg.host.as_str(), cfg.port);
            let attempt = async {
                match &cfg.credentials {
                    Some(c) => Socks5Stream::connect_with_password(proxy, (host, port), &c.username, &c.password).await,
                    None => Socks5Stream::connect(proxy, (host, port)).await,
                }
            };
            let stream = tokio::time::timeout(CONNECT_TIMEOUT, attempt)
                .await
                .map_err(|_| UpstreamError::Timeout)?
                .map_err(|e| match e {
                    tokio_socks::Error::HostUnreachable
                    | tokio_socks::Error::NetworkUnreachable
                    | tokio_socks::Error::ConnectionRefused
                    | tokio_socks::Error::TtlExpired => UpstreamError::SocksTarget(e.to_string()),
                    other => UpstreamError::Socks(other.to_string()),
                })?;
            Ok(Box::new(stream))
        }
    }
}

async fn read_head(stream: &mut BoxedIo) -> Result<Vec<u8>, UpstreamError> {
    match http::read_head(stream, MAX_HEAD).await? {
        http::Head::Done(buf) => Ok(buf),
        _ => Err(UpstreamError::Protocol),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_failures_are_told_from_target_failures() {
        assert!(UpstreamError::Status(407).is_proxy_failure());
        assert!(UpstreamError::Status(403).is_proxy_failure(), "403 вместо 407 — тоже отказ прокси");
        assert!(!UpstreamError::Status(502).is_proxy_failure(), "502 — сайт за прокси недоступен");
        assert!(UpstreamError::Socks(tokio_socks::Error::GeneralSocksServerFailure.to_string()).is_proxy_failure());
        assert!(!UpstreamError::SocksTarget("host unreachable".into()).is_proxy_failure());
        assert!(UpstreamError::Timeout.is_proxy_failure());
    }
}
