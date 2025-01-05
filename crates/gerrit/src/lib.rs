#![allow(deprecated)]
use std::{collections::HashMap, sync::Mutex, time::Duration};

use base64::encode;
use octabot_rust_sdk::{wit::export, ActionData, Error, Metadata, Plugin, PluginError, PluginResult};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use strfmt::strfmt;
use url::Url;
use waki::{Client, Method, RequestBuilder};

const GERRIT_RESPONSE_PREFIX: &str = ")]}'";

static CONFIG: Lazy<Mutex<Option<Config>>> = Lazy::new(|| Mutex::new(None));

#[derive(Serialize, Deserialize, Clone)]
struct Config {
  pub endpoint: String,
  pub timeout: Option<u64>,
  pub login: String,
  pub password: String,
}

#[derive(Serialize, Deserialize)]
struct QueryOptions {
  query: String,
  channel: String,
  topic: String,
  project: String,
  template: String,
  review_template: String,
}

#[derive(Serialize, Deserialize)]
struct Params {
  task_id: String,
  options: QueryOptions,
}

#[derive(Deserialize, Serialize, Debug)]
struct User {
  name: String,
  email: String,
  username: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct Label {
  rejected: Option<User>,
  approved: Option<User>,
  disliked: Option<User>,
  recommended: Option<User>,
}

#[derive(Deserialize, Serialize, Debug)]
struct Labels {
  #[serde(alias = "Verified")]
  verified: Label,
  #[serde(alias = "Code-Review")]
  code_review: Label,
}

#[derive(Deserialize, Serialize, Debug)]
struct Review {
  id: String,
  project: String,
  branch: String,
  change_id: String,
  subject: String,
  status: String,
  created: String,
  updated: String,
  submit_type: String,
  insertions: i32,
  deletions: i32,
  unresolved_comment_count: i32,
  owner: User,
  labels: Labels,
  _number: i32,
}

struct GerritPlugin;

impl GerritPlugin {
  fn request(path: &str) -> Result<RequestBuilder, PluginError> {
    let config = CONFIG
      .lock()
      .map_err(|e| PluginError::ConfigLock(e.to_string()))?
      .clone()
      .ok_or_else(|| PluginError::Other("Config not initialized".to_string()))?;

    let url = Url::parse(&format!("{}/{}", config.endpoint, path)).map_err(|e| PluginError::Other(e.to_string()))?;

    let credentials = format!("{}:{}", config.login, config.password);
    let authorization = encode(credentials);

    let client = Client::new()
      .request(Method::Get, url.as_str())
      .connect_timeout(Duration::from_secs(config.timeout.unwrap_or(60)))
      .headers([
        ("Content-Type", "application/json"),
        ("Authorization", &format!("Basic {}", authorization.as_str())),
      ]);

    Ok(client)
  }

  fn format_review_message(
    review: &Review,
    template: &str,
    config: &Config,
    params: &QueryOptions,
  ) -> Result<String, PluginError> {
    let vars = HashMap::from([
      ("subject".to_string(), review.subject.clone()),
      ("insertions".to_string(), review.insertions.to_string()),
      ("deletions".to_string(), review.deletions.to_string()),
      ("url".to_string(), config.endpoint.clone()),
      ("number".to_string(), review._number.to_string()),
      ("project".to_string(), params.project.to_string()),
    ]);

    strfmt(template, &vars).map_err(|e| PluginError::Other(format!("Failed to format review message template: {}", e)))
  }
}

impl Plugin for GerritPlugin {
  fn process(payload: String) -> Result<Vec<PluginResult>, Error> {
    let mut actions = vec![];
    let params = serde_json::from_str::<Params>(&payload)
      .map_err(|err| PluginError::ParseActionPaylod(format!("unable to parse gerrit query params: {}", err)))?;

    let query = format!("{} project:{}", params.options.query, params.options.project);
    let query = [
      ("q", &query),
      ("o", &"DETAILED_ACCOUNTS".to_string()),
      ("o", &"LABELS".to_string()),
    ];

    let client = GerritPlugin::request("a/changes")?.query(&query);

    let resp = match client.send() {
      Ok(resp) => match resp.status_code() {
        200 => match String::from_utf8(resp.body().unwrap()) {
          Ok(resp) => resp,
          Err(e) => return Err(PluginError::ParseResponse(e.to_string()).into()),
        },
        code => return Err(PluginError::SendHttpRequest(format!("HTTP/{}", code)).into()),
      },
      Err(e) => return Err(PluginError::SendHttpRequest(e.to_string()).into()),
    };

    if resp == "Unauthorized" {
      return Err(PluginError::Other("Authorization error".into()))?;
    }

    let data = resp
      .strip_prefix(GERRIT_RESPONSE_PREFIX)
      .ok_or_else(|| PluginError::ParseResponse("Missing gerrit prefix in response".into()))?;

    let reviews: Vec<Review> = serde_json::from_str(data)
      .map_err(|e| PluginError::ParseResponse(format!("Failed to parse reviews from response: {}", e)))?;

    if !reviews.is_empty() {
      let vars = HashMap::from([("project".to_string(), params.options.project.to_string())]);
      let mut message = strfmt(&params.options.template, &vars)
        .map_err(|e| PluginError::Other(format!("Failed to format message template: {}", e)))?;

      let config = CONFIG
        .lock()
        .map_err(|e| PluginError::ConfigLock(e.to_string()))?
        .clone()
        .ok_or_else(|| PluginError::Other("Config not initialized".to_string()))?;

      for review in reviews {
        let review_message =
          GerritPlugin::format_review_message(&review, &params.options.review_template, &config, &params.options)?;
        message.push_str(&review_message);
      }

      let action = ActionData {
        name: "zulip".to_string(), // Name of notificationm zulip action
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
      name: "Gerrit".to_string(),
      version: "0.1.0".to_string(),
      author: "OctaHive".to_string(),
      description: "Gerrit integration connector".to_string(),
    }
  }
}

export!(GerritPlugin with_types_in octabot_rust_sdk::wit);
