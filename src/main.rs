mod config;

use anyhow::Result;
use http_body::Body;
use http_body_util::combinators::BoxBody;
use http_body_util::BodyExt;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{body, Request, Response, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

// use tokio::time::timeout;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Instant;
use tracing::{debug, error, info};

/// A server to which the load balancer will route requests
#[derive(Clone, Debug)]
struct Server<B> {
    /// Server port
    pub port: u16,
    /// Server address
    pub addr: String,
    phantom: PhantomData<B>,
}

impl<B> From<config::Backend> for Server<B> {
    fn from(value: config::Backend) -> Self {
        Self {
            addr: value.address,
            port: value.port,            phantom: PhantomData,
        }
    }
}

impl<B> Server<B>
where
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    pub fn new(addr: &str, port: u16) -> Self {
        Self {
            addr: addr.into(),
            port,
            phantom: PhantomData,
        }
    }

    /// Forward an incoming request from the load balancer
    pub async fn forward(
        &self,
        req: Request<body::Incoming>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>> {
        let uri = req.uri().to_string();
        debug!("Uri received from request, {}", uri);
        let uri = format!("{}:{}{}", self.addr, self.port, uri);
        info!("Forwarding request to {}", uri);
        let mut request = req;
        *request.uri_mut() = Uri::from_str(&uri)?;
        let now = Instant::now();
        let response = Client::builder(TokioExecutor::new())
            .build_http()
            .request(request)
            .await;
        debug!("Received response, {:?}", response);
        info!(
            "Received response in {}",
            humantime::format_duration(now.elapsed())
        );
        Ok(Response::new(
            response
                .map_err(|e| {
                    error!("{}", e);
                    e
                })?
                .boxed(),
        ))
    }
}

#[derive(Debug)]
struct Balancer {
    pub servers: Vec<Server<Response<BoxBody<Bytes, hyper::Error>>>>,
    next_request_index: Arc<Mutex<AtomicU64>>,
}

impl Balancer {
    pub fn new() -> Self {
        Self {
            servers: Vec::new(),
            next_request_index: Arc::new(Mutex::new(AtomicU64::new(0))),
        }
    }

    /// Obtains the servers' index the request will be routed to
    /// We will determine which server to send the request to based on the simple round robin algorithm
    fn next_id(&self) -> usize {
        let lock = self.next_request_index.lock().unwrap();
        let val = lock.load(Ordering::Acquire);
        // To ensure the index never goes out of bounds
        lock.store((val + 1) % self.servers.len() as u64, Ordering::Release);
        val as usize
    }

    pub fn with_server(mut self, addr: &SocketAddr) -> Self {
        let srv = Server::new(&addr.ip().to_string(), addr.port());
        self.servers.push(srv);
        self
    }

    /// distribute an incoming request
    pub async fn distribute(
        &self,
        req: Request<body::Incoming>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>> {
        // get the index of the server that will receive the request
        let index = self.next_id();
        info!("Forwarding request to server at index {}", index);
        self.servers[index].forward(req).await
    }
}

/// checks periodically which servers are healthy
/* async fn heartbeat(balancer: Arc<Balancer>) -> Result<()> {
    for srv in balancer.servers.iter() {
        let uri: Uri = format!("http://{}:{}/", srv.addr, srv.port).parse()?;
        let _ = tokio::spawn(async {
            info!("Checking for healthyness at {}", uri);
            let fut = Client::new().get(uri);
            match timeout(Duration::from_secs(10), fut).await {
                Ok(res) => {
                    match res {
                        Ok(r) => {
                            if r.status().is_success() {
                                info!("Server is healthy.");
                            }
                        },
                        Err(_) => {
                            error!("Server is not healthy.");
                        }
                    }
                },
                Err(_) => {
                    error!("Server not healthy.");
                }
            }
        });
    }
    Ok(())
}*/

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // set up logging
    tracing_subscriber::fmt::init();
    // configuration
    let config = config::Config::default();
    println!("{}", toml::to_string(&config).unwrap());
    info!("Loading config");
    let config = config::Config::from_file("config.toml").map_err(|e| {
        error!("Failed to load configuration");
        e
    })?;
    info!("configuration loaded successfully.");
    // Create the load balancer
    let mut balancer = Balancer::new();
    balancer.servers.extend(config.server.into_iter().map(Server::from));
    let state = Arc::new(balancer);
    let address = &config.load_balancer.address.unwrap_or("127.0.0.1".to_string());
    let addr: SocketAddr = format!("{}:{}", address, config.load_balancer.port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    info!("Listening on {}", addr);
    loop {
        let state_clone = state.clone();
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        info!("Incoming connection from {}", io.inner().peer_addr()?);
        tokio::task::spawn(async move {
            let state = state_clone.clone();
            if let Err(e) = http1::Builder::new()
                .serve_connection(
                    io,
                    service_fn(|req: Request<body::Incoming>| {
                        let state = state.clone();
                        async move { state.distribute(req).await }
                    }),
                )
                .await
            {
                error!("Error while serving connection, {:?}", e);
            }
        });
    }
}
