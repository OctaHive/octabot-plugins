#![allow(deprecated)]
use std::time::Duration;
use std::{num::NonZeroU32, sync::Mutex};

use base64::encode;
use governor::{clock::DefaultClock, state::keyed::DefaultKeyedStateStore, Quota, RateLimiter};
use octabot_rust_sdk::{wit::export, Error, Metadata, Plugin, PluginError, PluginResult};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use url::Url;
use waki::{Client, Method, RequestBuilder};

type Limiter = RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>;

static RATE_LIMITER: Lazy<Mutex<Option<Limiter>>> = Lazy::new(|| Mutex::new(None));

static CONFIG: Lazy<Mutex<Option<Config>>> = Lazy::new(|| Mutex::new(None));

#[derive(Serialize, Deserialize, Clone)]
struct Config {
  pub endpoint: String,
  pub timeout: Option<u64>,
  pub login: String,
  pub password: String,
  pub max_request_in_minute: u32,
}

#[derive(Serialize, Deserialize)]
struct Message {
  channel: String,
  topic: String,
  message: String,
}

#[derive(Serialize, Deserialize)]
struct Params {
  task_id: String,
  options: Message,
}

#[derive(Deserialize, Serialize, Debug)]
struct PostMessageResponse {
  id: i32,
  msg: String,
  result: String,
}

struct ZulipPlugin;

impl ZulipPlugin {
  fn request(key: String, path: &str) -> Result<RequestBuilder, PluginError> {
    let limiter = RATE_LIMITER
      .lock()
      .map_err(|e| PluginError::Other(format!("Can't lock rate limiter: {}", e)))?;
    let config = CONFIG
      .lock()
      .map_err(|e| PluginError::ConfigLock(e.to_string()))?
      .clone()
      .ok_or_else(|| PluginError::Other("Config not initialized".to_string()))?;

    if let Some(limiter) = limiter.as_ref() {
      // Check rate limit
      if let Err(not_until) = limiter.check_key(&key) {
        return Err(PluginError::Other(format!(
          "Rate limit exceeded. Try again in {} seconds",
          not_until
            .wait_time_from(governor::clock::Clock::now(limiter.clock()))
            .as_secs()
        )));
      }

      let url = Url::parse(&format!("{}/{}", &config.endpoint, path)).map_err(|e| PluginError::Other(e.to_string()))?;

      let credentials = format!("{}:{}", config.login, config.password);
      let authorization = encode(credentials);

      let client = Client::new()
        .request(Method::Post, url.as_str())
        .connect_timeout(Duration::from_secs(config.timeout.unwrap_or(60)))
        .headers([
          ("Content-Type", "application/json"),
          ("Authorization", &format!("Basic {}", authorization.as_str())),
        ]);

      Ok(client)
    } else {
      Err(PluginError::Other("Rate limiter not initialized".to_string()))
    }
  }
}

impl Plugin for ZulipPlugin {
  fn process(payload: String) -> Result<Vec<PluginResult>, Error> {
    let params = serde_json::from_str::<Params>(&payload)
      .map_err(|err| PluginError::ParseActionPaylod(format!("unable to parse zulip params: {}", err)))?;

    let query = [
      ("type", "stream"),
      ("to", &params.options.channel),
      ("topic", &params.options.topic),
      ("content", &params.options.message),
    ];

    let client = ZulipPlugin::request(params.task_id, "api/v1/messages")?.query(&query);

    let resp = match client.send() {
      Ok(resp) => match resp.status_code() {
        200 => match String::from_utf8(resp.body().unwrap()) {
          Ok(resp) => {
            serde_json::from_str::<PostMessageResponse>(&resp).map_err(|e| PluginError::ParseResponse(e.to_string()))?
          },
          Err(e) => return Err(PluginError::ParseResponse(e.to_string()).into()),
        },
        code => return Err(PluginError::SendHttpRequest(format!("HTTP/{}", code)).into()),
      },
      Err(e) => return Err(PluginError::SendHttpRequest(e.to_string()).into()),
    };

    if resp.result != "success" {
      return Err(PluginError::Other(resp.msg).into());
    }

    Ok(vec![])
  }

  fn init(config: String) -> Result<(), Error> {
    let config = serde_json::from_str::<Config>(&config).map_err(|err| PluginError::ParseBotConfig(err.to_string()))?;

    let mut global_config = CONFIG.lock().map_err(|e| PluginError::ConfigLock(e.to_string()))?;
    *global_config = Some(config.clone());

    let mut limiter = RATE_LIMITER.lock().map_err(|e| PluginError::Other(e.to_string()))?;
    if limiter.is_none() {
      let rate = NonZeroU32::new(config.max_request_in_minute.max(1))
        .ok_or_else(|| PluginError::Other("Invalid rate limit value".to_string()))?;

      *limiter = Some(RateLimiter::keyed(Quota::per_minute(rate)));
      // TODO: change to logging
      println!(
        "Rate limiter initialized with {} requests per minute.",
        config.max_request_in_minute
      );
    }

    Ok(())
  }

  fn load() -> Metadata {
    Metadata {
      name: "Zulip".to_string(),
      version: "0.1.0".to_string(),
      author: "OctaHive".to_string(),
      description: "Notification connector to zulip messenger".to_string(),
    }
  }
}

export!(ZulipPlugin with_types_in octabot_rust_sdk::wit);
