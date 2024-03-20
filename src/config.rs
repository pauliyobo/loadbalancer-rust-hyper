//! Configuration module
use serde::{Deserialize, Serialize};
use anyhow::Result;
use std::{fs::File, io::Read};

/// Top level configuration
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// Define on which address:port combination the loadbalancer will listen
    pub listen: String,
}

impl Config {
    pub fn from_file(path: &str) -> Result<Config> {
        let mut data = String::new();
        File::open(path)?.read_to_string(&mut data)?;
        let conf = toml::from_str(&data).expect("Failed to deserialize config");
        Ok(conf)
    }
}