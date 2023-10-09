use anyhow::{Ok, Result};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Request, Response, Uri};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

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
        let uri = format!("{}:{}{}", self.addr, self.port, path);
        let mut request = Request::from(req);
        *request.uri_mut() = Uri::from_str(&uri)?;
        Ok(Client::new().request(request).await?)
    }
}

#[derive(Clone, Debug)]
struct Balancer {
    servers: Vec<Server>,
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
        self.servers[index].forward(req).await
    }
}

async fn handler(balancer: Arc<Balancer>, req: Request<Body>) -> Result<Response<Body>> {
    let res = balancer.distribute(req);
    Ok(res.await?)
}

#[tokio::main]
async fn main() {
    // Create the load balancer
    let state = Arc::new(Balancer::new().with_server(&"127.0.0.1:8000".parse().unwrap()));
    let addr: SocketAddr = "0.0.0.0:80".parse().unwrap();
    let sv = make_service_fn(move |_| {
        let state = state.clone();
        async move {
            Ok(service_fn(move |req: Request<Body>| {
                handler(state.clone(), req)
            }))
        }
    });

    let server = hyper::Server::bind(&addr).serve(sv);

    if let Err(e) = server.await {
        println!("{}", e);
    }
}
