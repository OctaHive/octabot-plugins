use std::time::Duration;
use std::{collections::HashMap, sync::Mutex};

use base64::prelude::{Engine, BASE64_STANDARD};
use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, TimeZone, Timelike};
use chrono_tz::Tz;
use octabot_rust_sdk::TaskData;
use octabot_rust_sdk::{wit::export, Error, Metadata, Plugin, PluginError, PluginResult};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use waki::{Client, Method};

static CONFIG: Lazy<Mutex<Option<Config>>> = Lazy::new(|| Mutex::new(None));

pub const EVENT_SHEBANG: &str = "[PLATFORM_BOT]";
pub const EVENT_SHEBANG_REGEXP: &str = r"(?:\[PLATFORM_BOT\])([\s\S]*)(?:\[PLATFORM_BOT\])";

struct ExchangePlugin;

#[derive(Serialize, Deserialize, Clone)]
struct Config {
  pub endpoint: String,
  pub timeout: Option<u64>,
  pub login: String,
  pub password: String,
  pub timezone: String,
}

#[derive(Serialize, Deserialize)]
struct QueryOptions {}

#[derive(Serialize, Deserialize)]
struct Params {
  task_id: String,
  options: QueryOptions,
}

#[derive(Deserialize, Serialize, Debug)]
struct EventLocation {
  #[serde(alias = "DisplayName")]
  display_name: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct EventBody {
  #[serde(alias = "Content")]
  content: String,
  #[serde(alias = "ContentType")]
  content_type: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct EventDate {
  #[serde(alias = "DateTime")]
  date_time: String,
  #[serde(alias = "TimeZone")]
  timezone: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct ExchangeEvent {
  #[serde(alias = "Id")]
  id: String,
  #[serde(alias = "Subject")]
  subject: String,
  #[serde(alias = "Location")]
  location: EventLocation,
  #[serde(alias = "Start")]
  start: EventDate,
  #[serde(alias = "LastModifiedDateTime")]
  last_modified: String,
  #[serde(alias = "Body")]
  body: EventBody,
}

#[derive(Deserialize, Serialize, Debug)]
struct ExchangeResponse {
  value: Vec<ExchangeEvent>,
}

impl ExchangePlugin {
  fn request(url: &str, login: &str, password: &str) -> Result<Vec<PluginResult>, PluginError> {
    let mut result = vec![];
    let nego_flags = ntlmclient::Flags::NEGOTIATE_UNICODE
      | ntlmclient::Flags::REQUEST_TARGET
      | ntlmclient::Flags::NEGOTIATE_NTLM
      | ntlmclient::Flags::NEGOTIATE_WORKSTATION_SUPPLIED;

    let nego_msg = ntlmclient::Message::Negotiate(ntlmclient::NegotiateMessage {
      flags: nego_flags,
      supplied_domain: String::new(),
      supplied_workstation: "octabot".to_owned(),
      os_version: Default::default(),
    });

    let nego_msg_bytes = nego_msg
      .to_bytes()
      .map_err(|e| PluginError::Other(format!("failed to encode NTLM negotiation message: {}", e)))?;
    let nego_b64 = BASE64_STANDARD.encode(&nego_msg_bytes);

    let client = Client::new()
      .request(Method::Get, url)
      .connect_timeout(Duration::from_secs(60))
      .header("Authorization", format!("NTLM {}", nego_b64));

    match client.send() {
      Ok(resp) => {
        let challenge_header = resp
          .headers()
          .get("www-authenticate")
          .expect("response missing challenge header");
        let challenge_b64 = challenge_header
          .to_str()
          .map_err(|_| PluginError::Other("challenge header not a string".to_owned()))?
          .split(" ")
          .nth(1)
          .expect("second chunk of challenge header missing");
        let challenge_bytes = BASE64_STANDARD
          .decode(challenge_b64)
          .map_err(|e| PluginError::Other(format!("base64 decoding challenge message failed: {}", e)))?;
        let challenge = ntlmclient::Message::try_from(challenge_bytes.as_slice())
          .map_err(|e| PluginError::Other(format!("decoding challenge message failed: {}", e)))?;

        let challenge_content = match challenge {
          ntlmclient::Message::Challenge(c) => c,
          other => return Err(PluginError::Other(format!("wrong challenge message: {:?}", other))),
        };

        let target_info_bytes: Vec<u8> = challenge_content
          .target_information
          .iter()
          .flat_map(|ie| ie.to_bytes())
          .collect();

        let (login, domain) = match login.split_once('@') {
          Some((login, domain)) => (login, domain),
          None => return Err(PluginError::Other(format!("invalid login format: {}", login))),
        };

        let creds = ntlmclient::Credentials {
          username: login.to_owned(),
          password: password.to_owned(),
          domain: domain.to_owned(),
        };

        let challenge_response = ntlmclient::respond_challenge_ntlm_v2(
          challenge_content.challenge,
          &target_info_bytes,
          ntlmclient::get_ntlm_time(),
          &creds,
        );

        let auth_flags = ntlmclient::Flags::NEGOTIATE_UNICODE | ntlmclient::Flags::NEGOTIATE_NTLM;
        let auth_msg = challenge_response.to_message(&creds, "octabot", auth_flags);
        let auth_msg_bytes = auth_msg
          .to_bytes()
          .map_err(|e| PluginError::Other(format!("failed to encode NTLM authentication message: {}", e)))?;
        let auth_b64 = BASE64_STANDARD.encode(&auth_msg_bytes);

        let client = Client::new()
          .request(Method::Get, url)
          .connect_timeout(Duration::from_secs(60))
          .headers([
            ("Authorization", format!("NTLM {}", auth_b64)),
            ("Prefer", "outlook.body-content-type=\"text\"".to_owned()),
            ("Prefer", "outlook.timezone=\"Europe/Moscow\"".to_owned()),
          ]);

        match client.send() {
          Ok(resp) => match resp.status_code() {
            200 => match String::from_utf8(resp.body().unwrap()) {
              Ok(content) => {
                let events: ExchangeResponse = serde_json::from_str(&content)
                  .map_err(|e| PluginError::Other(format!("Failed to parse exchange response: {}", e)))?;

                for event in events.value {
                  if !event.body.content.contains(EVENT_SHEBANG) {
                    // TODO: change to logging
                    println!("Found exchange event without shebang. Event subject: {}", event.subject);
                    continue;
                  }

                  match ExchangePlugin::process_single_event(&event) {
                    Ok(task_data) => result.push(PluginResult::Task(task_data)),
                    Err(e) => {
                      return Err(PluginError::Other(format!(
                        "Failed to process event {}: {}",
                        event.subject, e
                      )))
                    },
                  }
                }
              },
              Err(e) => return Err(PluginError::ParseResponse(e.to_string())),
            },
            code => return Err(PluginError::Other(format!("HTTP/{}", code))),
          },
          Err(e) => return Err(PluginError::Other(e.to_string())),
        }
      },
      Err(e) => return Err(PluginError::Other(e.to_string())),
    }

    Ok(result)
  }

  fn process_single_event(event: &ExchangeEvent) -> Result<TaskData, PluginError> {
    let options = ExchangePlugin::parse_event_options(event)?;
    let start_time = ExchangePlugin::parse_event_time(event)?;
    let modified_at: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(&event.last_modified)
      .map_err(|e| PluginError::Other(format!("Invalid datetime format: {}", e)))?;

    let project_code = options
      .get("project")
      .ok_or_else(|| PluginError::Other("No project code in options".to_owned()))?;

    let task = TaskData {
      name: event.subject.clone(),
      kind: String::from("notify"),
      //   schedule: None,
      project_code: project_code.clone(),
      external_id: event.id.clone(),
      external_modified_at: modified_at.to_utc().timestamp() as u32,
      start_at: start_time.timestamp() as u32,
      options: serde_json::to_string(&options)
        .map_err(|e| PluginError::Other(format!("Failed to parse task options: {}", e)))?,
    };

    Ok(task)
  }

  fn parse_event_options(event: &ExchangeEvent) -> Result<HashMap<String, String>, PluginError> {
    let html_re = Regex::new(r"<[^>]*>").unwrap();
    let no_html = html_re.replace_all(&event.body.content, "");
    let no_line_breaks = no_html.replace(['\n', '\r'], " ");

    let regex = Regex::new(EVENT_SHEBANG_REGEXP)
      .map_err(|e| PluginError::Other(format!("Failed to parse bot options regexp: {}", e)))?;

    let captures = regex
      .captures(&no_line_breaks)
      .ok_or_else(|| PluginError::Other("No bot options found in event".to_owned()))?;

    ExchangePlugin::parse_options(captures[1].trim())
  }

  fn parse_options(raw_options: &str) -> Result<HashMap<String, String>, PluginError> {
    let mut options = HashMap::new();

    for line in raw_options.lines().filter(|l| !l.is_empty()) {
      let parts: Vec<&str> = line.split(": ").collect();
      if parts.len() == 2 {
        options.insert(parts[0].trim().to_lowercase(), parts[1].trim().to_string());
      } else {
        return Err(PluginError::Other(format!("Invalid option format: '{}'", line)));
      }
    }

    Ok(options)
  }

  fn parse_event_time(event: &ExchangeEvent) -> Result<DateTime<Tz>, PluginError> {
    let tz: Tz = event
      .start
      .timezone
      .parse()
      .map_err(|e| PluginError::Other(format!("Invalid timezone: {}", e)))?;

    let naive = NaiveDateTime::parse_from_str(&event.start.date_time, "%Y-%m-%dT%H:%M:%S%.f")
      .map_err(|e| PluginError::Other(format!("Invalid datetime format: {}", e)))?;

    tz.from_local_datetime(&naive)
      .single()
      .ok_or_else(|| PluginError::Other("Invalid timestamp".to_owned()))
  }
}

impl Plugin for ExchangePlugin {
  fn process(payload: String) -> Result<Vec<PluginResult>, Error> {
    let config = CONFIG
      .lock()
      .map_err(|e| PluginError::ConfigLock(e.to_string()))?
      .clone()
      .ok_or_else(|| PluginError::Other("Config not initialized".to_string()))?;

    let _params = serde_json::from_str::<Params>(&payload)
      .map_err(|err| PluginError::ParseActionPaylod(format!("unable to parse zulip message: {}", err)))?;

    let tz: Tz = config
      .timezone
      .parse()
      .map_err(|e| PluginError::Other(format!("Invalid timezone: {}", e)))?;

    let start = Local::now()
      .with_timezone(&tz)
      .with_hour(0)
      .unwrap()
      .with_minute(0)
      .unwrap()
      .with_second(0)
      .unwrap();

    let end = Local::now()
      .with_timezone(&tz)
      .with_hour(23)
      .unwrap()
      .with_minute(59)
      .unwrap()
      .with_second(59)
      .unwrap();

    let url = format!("{}/api/v2.0/me/calendarview?startDateTime={}&endDateTime={}&$select=Subject,Start,Location,Body,LastModifiedDateTime",
      config.endpoint,
      start.format("%Y-%m-%dT%H:%M:%S"),
      end.format("%Y-%m-%dT%H:%M:%S"));

    let result = ExchangePlugin::request(&url, &config.login, &config.password)?;

    Ok(result)
  }

  fn init(config: String) -> Result<(), Error> {
    let config = serde_json::from_str::<Config>(&config).map_err(|err| PluginError::ParseBotConfig(err.to_string()))?;

    let mut global_config = CONFIG.lock().map_err(|e| PluginError::ConfigLock(e.to_string()))?;
    *global_config = Some(config.clone());

    Ok(())
  }

  fn load() -> Metadata {
    Metadata {
      name: "Exchange".to_string(),
      version: "0.1.0".to_string(),
      author: "OctaHive".to_string(),
      description: "Exchange integration connector".to_string(),
    }
  }
}

export!(ExchangePlugin with_types_in octabot_rust_sdk::wit);
