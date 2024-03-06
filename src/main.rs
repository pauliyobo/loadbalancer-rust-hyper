use anyhow::Result;
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Request, Response, Uri};
use tokio::time::timeout;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Instant, Duration};
use tracing::{info, error};

/// A server to which the load balancer will route requests
#[derive(Clone, Debug)]
struct Server {
    /// Server port
    pub port: u16,
    /// Server address
    pub addr: String,
}

impl Server {
    pub fn new(addr: &str, port: u16) -> Self {
        Self {
            addr: addr.into(),
            port,
        }
    }

    /// Forward an incoming request from the load balancer
    pub async fn forward(&self, req: Request<Body>) -> Result<Response<Body>> {
        let path = req.uri().path().to_owned();
        let uri = format!("http://{}:{}{}", self.addr, self.port, path);
        info!("Forwarding request to {}", uri);
        let mut request = req;
        *request.uri_mut() = Uri::from_str(&uri)?;
        let now = Instant::now();
        let response = Client::new().request(request).await?;
        info!("{:?}", response);
        info!("Received response in {}", humantime::format_duration(now.elapsed()));
        Ok(response)
    }
}

#[derive(Clone, Debug)]
struct Balancer {
    pub servers: Vec<Server>,
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
    pub async fn distribute(&self, req: Request<Body>) -> Result<Response<Body>> {
        // get the index of the server that will receive the request
        let index = self.next_id();
        info!("Forwarding request to server at index {}", index);
        self.servers[index].forward(req).await
    }
}

/// checks periodically which servers are healthy
async fn heartbeat(balancer: Arc<Balancer>) -> Result<()> {
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
}

async fn handler(balancer: Arc<Balancer>, req: Request<Body>) -> Result<Response<Body>> {
    let res = balancer.distribute(req);
    Ok(res.await?)
}

#[tokio::main]
async fn main() {
    // set up logging
    tracing_subscriber::fmt::init();
    // Create the load balancer
    let state = Arc::new(Balancer::new().with_server(&"127.0.0.1:8001".parse().unwrap()).with_server(&"127.0.0.1:8000".parse().unwrap()));
    let state_clone = state.clone();
    let addr: SocketAddr = "0.0.0.0:80".parse().unwrap();
    let sv = make_service_fn(move |con: &AddrStream| {
        info!("Incoming connection from {}", con.remote_addr());
        let state = state.clone();
        async move {
            anyhow::Ok(service_fn(move |req: Request<Body>| {
                handler(state.clone(), req)
            }))
        }
    });

    let server = hyper::Server::bind(&addr).serve(sv);
    let _ = tokio::spawn(async move {
        loop {
            let state = state_clone.clone();
            info!("Running health check.");
            heartbeat(state)        .await.unwrap();
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });

    if let Err(e) = server.await {
        error!("{}", e);
    }
}
