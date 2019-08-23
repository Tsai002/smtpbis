#![warn(rust_2018_idioms)]

use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::BytesMut;
use tokio::net::TcpListener;
use tokio::prelude::*;

use tokio_rustls::rustls::{
    internal::pemfile::{certs, pkcs8_private_keys},
    NoClientAuth, ServerConfig, ServerSession, Session,
};

use rustyknife::rfc5321::{ForwardPath, Param, Path, ReversePath};
use rustyknife::types::{DomainPart, Mailbox};
use smtpbis::{smtp_server, Handler, HandlerResult, LineError, Reply, ServerError};

const CERT: &[u8] = include_bytes!("ssl-cert-snakeoil.pem");
const KEY: &[u8] = include_bytes!("ssl-cert-snakeoil.key");

struct DummyHandler {
    tls_config: Arc<ServerConfig>,
    addr: SocketAddr,
    mail: Option<ReversePath>,
    rcpt: Option<ForwardPath>,
    body: Vec<u8>,
}

#[async_trait]
impl Handler for DummyHandler {
    async fn tls_request(&mut self) -> Option<Arc<ServerConfig>> {
        Some(self.tls_config.clone())
    }

    async fn tls_started(&mut self, session: &ServerSession) {
        println!(
            "TLS started: {:?}/{:?}",
            session.get_protocol_version(),
            session.get_negotiated_ciphersuite()
        );
    }

    async fn mail(&mut self, path: ReversePath, _params: Vec<Param>) -> HandlerResult {
        println!("Handler MAIL: {:?}", path);
        if let ReversePath::Null = &path {
            return Err(None);
        }

        self.mail = Some(path);
        Ok(None)
    }

    async fn rcpt(&mut self, path: ForwardPath, _params: Vec<Param>) -> HandlerResult {
        println!("Handler RCPT: {:?}", path);
        if let ForwardPath::Path(Path(Mailbox(_, DomainPart::Domain(domain)), _)) = &path {
            if domain.starts_with('z') {
                return Err(None);
            }
        };
        self.rcpt = Some(path);
        Ok(None)
    }

    async fn data_start(&mut self) -> HandlerResult {
        println!("Handler DATA start");
        Ok(None)
    }

    async fn data<S>(&mut self, stream: &mut S) -> Result<Option<Reply>, ServerError>
    where
        S: Stream<Item = Result<BytesMut, LineError>> + Unpin + Send,
    {
        let mut nb_lines: usize = 0;
        self.body.clear();

        while let Some(line) = stream.next().await {
            let line = line?;

            self.body.extend(line);
            nb_lines += 1;
            if self.body.len() > 1000 {
                return Ok(Some(Reply::new(521, None, "too large")));
            }
        }

        println!("got {} body lines", nb_lines);

        Ok(Some(Reply::new(
            250,
            None,
            format!("Received {} bytes in {} lines.", self.body.len(), nb_lines),
        )))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:8080".parse().unwrap();
    let mut listener = TcpListener::bind(&addr).unwrap();

    let mut tls_config = ServerConfig::new(NoClientAuth::new());
    let certs = certs(&mut Cursor::new(CERT)).unwrap();
    let key = pkcs8_private_keys(&mut Cursor::new(KEY)).unwrap().remove(0);
    tls_config.set_single_cert(certs, key).unwrap();
    let tls_config = Arc::new(tls_config);

    loop {
        let (socket, addr) = listener.accept().await?;
        let handler = DummyHandler {
            addr,
            tls_config: tls_config.clone(),
            mail: None,
            rcpt: None,
            body: Vec::new(),
        };

        tokio::spawn(async move {
            if let Err(e) = smtp_server(socket, handler).await {
                println!("Top level error: {:?}", e);
            }
        });
    }
}
