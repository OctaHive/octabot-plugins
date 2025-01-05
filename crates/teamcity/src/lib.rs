#![allow(deprecated)]
use std::{collections::HashMap, sync::Mutex, time::Duration};

use octabot_rust_sdk::{wit::export, ActionData, Error, KeyValue, Metadata, Plugin, PluginError, PluginResult};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use strfmt::strfmt;
use url::Url;
use waki::{Client, Method, RequestBuilder};

static CONFIG: Lazy<Mutex<Option<Config>>> = Lazy::new(|| Mutex::new(None));

#[derive(Serialize, Deserialize, Clone)]
struct Config {
  pub endpoint: String,
  pub timeout: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct QueryOptions {
  build_name: String,
  channel: String,
  topic: String,
  template: String,
}

#[derive(Serialize, Deserialize)]
struct Params {
  task_id: String,
  options: QueryOptions,
}

#[derive(Deserialize, Serialize, Debug)]
struct TestOccurrences {
  passed: Option<i32>,
  failed: Option<i32>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct BuildType {
  name: String,
  project_name: String,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct BuildStatusResponse {
  id: u64,
  state: String,
  status: String,
  build_type: BuildType,
  status_text: String,
  web_url: String,
  finish_date: String,
  test_occurrences: Option<TestOccurrences>,
}

struct TeamcityPlugin;

impl TeamcityPlugin {
  fn request(path: &str) -> Result<RequestBuilder, PluginError> {
    let config = CONFIG
      .lock()
      .map_err(|e| PluginError::ConfigLock(e.to_string()))?
      .clone()
      .ok_or_else(|| PluginError::Other("Config not initialized".to_string()))?;

    let url = Url::parse(&format!("{}/{}", config.endpoint, path)).map_err(|e| PluginError::Other(e.to_string()))?;

    let client = Client::new()
      .request(Method::Get, url.as_str())
      .connect_timeout(Duration::from_secs(config.timeout.unwrap_or(60)))
      .headers([("Accept", "application/json")]);

    Ok(client)
  }

  fn should_notify(keyvalue: &KeyValue, build: &BuildStatusResponse) -> bool {
    if let Ok(result) = keyvalue.get("key") {
      if result.is_none() && build.state == "finished" && build.status == "FAILURE" {
        return true;
      }
    }

    return false;
  }
}

impl Plugin for TeamcityPlugin {
  fn process(payload: String) -> Result<Vec<PluginResult>, Error> {
    let mut actions = vec![];
    let params = serde_json::from_str::<Params>(&payload)
      .map_err(|err| PluginError::ParseActionPaylod(format!("unable to parse teamcity params: {}", err)))?;

    let path = format!("builds/buildType:{}", params.options.build_name);

    let client = TeamcityPlugin::request(&path)?;

    let resp: BuildStatusResponse = match client.send() {
      Ok(resp) => match resp.status_code() {
        200 => match String::from_utf8(resp.body().unwrap()) {
          Ok(resp) => serde_json::from_str(&resp)
            .map_err(|e| PluginError::Other(format!("Failed to parse teamcity response: {}", e)))?,
          Err(e) => return Err(PluginError::ParseResponse(e.to_string()).into()),
        },
        code => return Err(PluginError::SendHttpRequest(format!("HTTP/{}", code)).into()),
      },
      Err(e) => return Err(PluginError::SendHttpRequest(e.to_string()).into()),
    };

    let keyvalue = octabot_rust_sdk::KeyValue::open()?;
    if TeamcityPlugin::should_notify(&keyvalue, &resp) {
      keyvalue.set(&format!("{}", resp.id), b"")?;

      let vars = HashMap::from([
        ("name".to_string(), resp.build_type.name.clone()),
        ("project_name".to_string(), resp.build_type.project_name.clone()),
        ("status".to_string(), resp.status_text.clone()),
        ("web_url".to_string(), resp.web_url.clone()),
      ]);

      let message = strfmt(&params.options.template, &vars)
        .map_err(|err| PluginError::Other(format!("unable to format template: {}", err)))?;

      let action = ActionData {
        name: "zulip".to_string(),
        payload: json!({
          "task_id": params.task_id,
          "options": {
            "channel": params.options.channel,
            "topic": params.options.topic,
            "message": message
          }
        })
        .to_string(),
      };

      actions.push(PluginResult::Action(action));
    }

    Ok(actions)
  }

  fn init(config: String) -> Result<(), Error> {
    let config = serde_json::from_str::<Config>(&config).map_err(|err| PluginError::ParseBotConfig(err.to_string()))?;
    let mut global_config = CONFIG.lock().map_err(|e| PluginError::ConfigLock(e.to_string()))?;
    *global_config = Some(config.clone());

    Ok(())
  }

  fn load() -> Metadata {
    Metadata {
      name: "Teamcity".to_string(),
      version: "0.1.0".to_string(),
      author: "OctaHive".to_string(),
      description: "Teamcity integration connector".to_string(),
    }
  }
}

export!(TeamcityPlugin with_types_in octabot_rust_sdk::wit);
