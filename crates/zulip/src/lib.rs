#![allow(deprecated)]
use std::time::Duration;

use base64::encode;
use octabot_rust_sdk::{wit::export, Action, Error, Metadata, Plugin, PluginError};
use serde::{Deserialize, Serialize};
use url::Url;
use waki::{Client, Method, RequestBuilder};

#[derive(Serialize, Deserialize)]
struct Message {
  channel: String,
  topic: String,
  message: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct PostMessageResponse {
  id: i32,
  msg: String,
  result: String,
}

#[derive(Serialize, Deserialize)]
struct ZulipPlugin {
  pub endpoint: String,
  pub timeout: Option<u64>,
  pub login: String,
  pub password: String,
}

impl ZulipPlugin {
  fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, PluginError> {
    let url = Url::parse(&self.endpoint)
      .map_err(|e| PluginError::Other(e.to_string()))?
      .join(path)
      .map_err(|e| PluginError::Other(e.to_string()))?;

    let credentials = format!("{}:{}", self.login, self.password);
    let authorization = encode(credentials);

    let client = Client::new()
      .request(method, url.as_str())
      .connect_timeout(Duration::from_secs(self.timeout.unwrap_or(60)))
      .headers([
        ("Content-Type", "application/json"),
        ("Authorization", &format!("Basic {}", authorization.as_str())),
      ]);

    Ok(client)
  }

  fn parse_configuration(config: &str) -> Result<Self, PluginError> {
    serde_json::from_str::<ZulipPlugin>(config)
      .map_err(|err| PluginError::Other(format!("unable to parse configuration: {}", err)))
  }
}

impl Plugin for ZulipPlugin {
  fn process(config: String, payload: String) -> Result<Vec<Action>, Error> {
    let zulip = ZulipPlugin::parse_configuration(&config)?;

    let message = serde_json::from_str::<Message>(&payload)
      .map_err(|err| PluginError::Other(format!("unable to parse message: {}", err)))?;

    let query = [
      ("type", "stream"),
      ("to", &message.channel),
      ("topic", &message.topic),
      ("content", &message.message),
    ];

    let client = zulip.request(Method::Post, "api/v1/messages")?.query(&query);

    let resp = match client.send() {
      Ok(resp) => match resp.status_code() {
        200 => match String::from_utf8(resp.body().unwrap()) {
          Ok(resp) => {
            serde_json::from_str::<PostMessageResponse>(&resp).map_err(|e| PluginError::Other(e.to_string()))?
          },
          Err(e) => return Err(PluginError::Other(e.to_string()).into()),
        },
        code => return Err(PluginError::Other(format!("HTTP/{}", code)).into()),
      },
      Err(e) => return Err(PluginError::Other(e.to_string()).into()),
    };

    if resp.result != "success" {
      return Err(PluginError::Other(resp.msg).into());
    }

    Ok(vec![])
  }

  fn init() -> Metadata {
    Metadata {
      name: "Zulip".to_string(),
      version: "0.1.0".to_string(),
      author: "OctaHive".to_string(),
      description: "Notification connector to zulip messenger".to_string(),
    }
  }
}

export!(ZulipPlugin with_types_in octabot_rust_sdk::wit);
