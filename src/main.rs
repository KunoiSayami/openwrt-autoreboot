/*
 ** Copyright (C) 2021 KunoiSayami
 **
 ** This file is part of openwrt-autoreboot and is released under
 ** the AGPL v3 License: https://www.gnu.org/licenses/agpl-3.0.txt
 **
 ** This program is free software: you can redistribute it and/or modify
 ** it under the terms of the GNU Affero General Public License as published by
 ** the Free Software Foundation, either version 3 of the License, or
 ** any later version.
 **
 ** This program is distributed in the hope that it will be useful,
 ** but WITHOUT ANY WARRANTY; without even the implied warranty of
 ** MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 ** GNU Affero General Public License for more details.
 **
 ** You should have received a copy of the GNU Affero General Public License
 ** along with this program. If not, see <https://www.gnu.org/licenses/>.
 */

use clap::{App, Arg, ArgMatches};
use log::{info, warn};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Map;

pub fn get_current_timestamp() -> u64 {
    let start = std::time::SystemTime::now();
    let since_the_epoch = start
        .duration_since(std::time::UNIX_EPOCH)
        .expect("Time went backwards");
    since_the_epoch.as_secs()
}

#[derive(Deserialize, Serialize)]
struct TokenField {
    token: String,
}

impl TokenField {
    fn new(token: String) -> Self {
        Self { token }
    }
}

#[derive(Deserialize, Serialize)]
struct LuciLoginField {
    luci_username: String,
    luci_password: String,
}

impl From<&Server> for LuciLoginField {
    fn from(server: &Server) -> Self {
        Self {
            luci_password: server.password.clone(),
            luci_username: server.user.clone(),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Server {
    host: String,
    user: String,
    password: String,
}

impl Server {
    fn try_from_matches(matches: &ArgMatches) -> Option<Self> {
        matches.value_of("password")?;
        Some(Self {
            host: matches.value_of("host").unwrap().to_string(),
            user: matches.value_of("user").unwrap().to_string(),
            password: matches.value_of("password").unwrap().to_string(),
        })
    }

    fn get_host(&self) -> &String {
        &self.host
    }
}

#[derive(Deserialize, Serialize)]
struct Config {
    server: Server,
}

impl Config {
    pub async fn load() -> anyhow::Result<Self> {
        let context = tokio::fs::read_to_string("config.toml").await?;
        Ok(toml::from_str(context.as_str())?)
    }
}

async fn async_main(matches: &ArgMatches) -> anyhow::Result<()> {
    let server = if let Some(server) = Server::try_from_matches(matches) {
        server
    } else {
        let config = Config::load().await?;
        config.server
    };
    let token_exp = Regex::new(r"token: '(?P<token>[\da-f]{32})'")?;
    let client = reqwest::ClientBuilder::new().cookie_store(true).build()?;
    client
        .post(format!("{}/cgi-bin/luci", server.get_host()))
        .form(&LuciLoginField::from(&server))
        .send()
        .await?;
    let response = client
        .get(format!(
            "{}/cgi-bin/luci/?status=1&_={}",
            server.get_host(),
            get_current_timestamp()
        ))
        .send()
        .await?;
    let response: Map<String, serde_json::Value> = response.json().await?;
    if let Some(serde_json::Value::String(cpu)) = response.get("cpuusage") {
        let (usage, _) = cpu.split_once("\n").unwrap();
        let cpu_usage = usage.parse::<i32>().unwrap();
        if cpu_usage > 20 {
            info!(
                "Current cpu usage is {}, checking is always in this value",
                cpu_usage
            );
            if let Some(serde_json::Value::Array(load_avg)) = response.get("loadavg") {
                if load_avg
                    .iter()
                    .map(|x| {
                        if let serde_json::Value::Number(n) = x {
                            let value = n.as_i64().unwrap();
                            if value > 65000 {
                                info!("Current load average value is {}", value);
                            }
                            value > 65000
                        } else {
                            false
                        }
                    })
                    .all(|x| x)
                {
                    warn!("Should call reboot now, performance OpenWRT reboot");
                    let response = client
                        .get(format!(
                            "{}/cgi-bin/luci/admin/system/reboot",
                            server.get_host()
                        ))
                        .send()
                        .await?;
                    let response = response.text().await?;
                    let matches = token_exp.captures(response.as_str()).unwrap();
                    let token = &matches["token"];
                    client
                        .post(format!(
                            "{}/cgi-bin/luci/admin/system/reboot/call",
                            server.get_host()
                        ))
                        .form(&TokenField::new(token.to_string()))
                        .send()
                        .await?;
                }
            }
        } else {
            info!(
                "Current cpu usage is {}, there is nothing to do.",
                cpu_usage
            )
        }
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env().init();
    let matches = App::new("Auto reboot openwrt service")
        .version(env!("CARGO_PKG_VERSION"))
        .arg(Arg::new("host").about("Specify remote host"))
        .arg(Arg::new("user").about("Specify host username"))
        .arg(Arg::new("password").about("Specify host password"))
        .get_matches();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async_main(&matches))
}
