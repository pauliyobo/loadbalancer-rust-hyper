//! Configuration module
use serde::{Deserialize, Serialize};
use anyhow::Result;
use std::{fs::File, io::Read};

/// Defines connection protocols supported by the load Balancer
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Http,
}

/// Top level configuration
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// Define on which address:port combination the loadbalancer will listen
    pub load_balancer: LoadBalancer,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoadBalancer {
    /// Address of the load balancer
    pub address: Option<String>,
    /// to which port should it be listening?
    pub port: u16,
    /// protocol for incoming connections
    pub protocol: Protocol,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            load_balancer: LoadBalancer {
                address: None,
                port: 80,
                protocol: Protocol::Http
            }
        }
    }
}

impl Config {
    pub fn from_file(path: &str) -> Result<Config> {
        let mut data = String::new();
        File::open(path)?.read_to_string(&mut data)?;
        let conf = toml::from_str(&data).expect("Failed to deserialize config");
        Ok(conf)
    }
}