#![allow(deprecated)]
use std::time::Duration;

use base64::encode;
use octabot_rust_sdk::{wit::export, Action, Error, Metadata, Plugin, PluginError};
use serde::{Deserialize, Serialize};
use url::Url;
use waki::{Client, Method, RequestBuilder};

const GERRIT_RESPONSE_PREFIX: &str = ")]}'";

#[derive(Serialize, Deserialize)]
struct QueryOptions {
  query: String,
  project: String,
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

#[derive(Serialize, Deserialize)]
struct GerritPlugin {
  pub endpoint: String,
  pub timeout: Option<u64>,
  pub login: String,
  pub password: String,
}

impl GerritPlugin {
  fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, PluginError> {
    let url = Url::parse(&format!("https://{}/{}", self.endpoint, path))
      .map_err(|e| e.to_string())
      .unwrap();

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
    serde_json::from_str::<GerritPlugin>(config)
      .map_err(|err| PluginError::Other(format!("unable to parse configuration: {}", err)))
  }
}

impl Plugin for GerritPlugin {
  fn process(config: String, payload: String) -> Result<Vec<Action>, Error> {
    let query_options = serde_json::from_str::<QueryOptions>(&payload)
      .map_err(|err| PluginError::Other(format!("unable to parse configuration: {}", err)))?;

    let query = format!("{} project:{}", query_options.query, query_options.project);
    let query = [
      ("q", &query),
      ("o", &"DETAILED_ACCOUNTS".to_string()),
      ("o", &"LABELS".to_string()),
    ];

    let gerrit = GerritPlugin::parse_configuration(&config)?;
    let client = gerrit.request(Method::Get, "a/changes")?.query(&query);

    let resp = match client.send() {
      Ok(resp) => match resp.status_code() {
        200 => match String::from_utf8(resp.body().unwrap()) {
          Ok(resp) => Ok(Some(resp)),
          Err(e) => Err(e.to_string()),
        },
        code => Err(format!("HTTP/{}", code)),
      },
      Err(e) => Err(e.to_string()),
    };

    let resp = match resp {
      Ok(resp) => resp.unwrap_or("".to_owned()),
      Err(e) => e,
    };

    if resp == "Unauthorized" {
      return Err(PluginError::Other("Authorization error".into()))?;
    }

    let data = resp
      .strip_prefix(GERRIT_RESPONSE_PREFIX)
      .ok_or_else(|| PluginError::Other("Invalid response format".into()))?;

    let reviews: Vec<Review> =
      serde_json::from_str(data).map_err(|e| PluginError::Other(format!("Failed to parse response: {}", e)))?;

    if !reviews.is_empty() {
      println!("{:#?}", reviews);
    }

    Ok(vec![])
  }

  fn init() -> Metadata {
    Metadata {
      name: "Gerrit".to_string(),
      version: "0.1.0".to_string(),
      author: "OctaHive".to_string(),
      description: "Gerrit integration connector".to_string(),
    }
  }
}

export!(GerritPlugin with_types_in octabot_rust_sdk::wit);
