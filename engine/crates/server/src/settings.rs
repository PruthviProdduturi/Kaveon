//! Per-request session settings on `POST /v1/statement`.
//!
//! HTTP is stateless and the Engine keeps no server-side session: a setting
//! lives exactly as long as the statement it arrives with. A request may
//! lower a bound the server configuration sets; it can never raise one.
//! Settings arrive as the request's `settings` object, or as `SET SESSION
//! <key> = <value>;` statements that lead the statement text in the same
//! request. Both forms go through the same validation.

use crate::config::ServerConfig;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// What one statement set for itself, validated against the server. Only
/// the keys a request gave are present, so a record serialises nothing for
/// a statement that set nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuerySettings {
    /// This statement's query memory pool, at most the configured per-query
    /// limit. Workers admit each task of the statement with the same value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_memory_limit_bytes: Option<u64>,
    /// Aggregator threads per task, from one to the node's configured
    /// parallelism. Reaches every operator through the query memory pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_parallelism: Option<usize>,
    /// `false` bypasses the coordinator's result cache for this statement:
    /// no lookup and no insertion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_cache: Option<bool>,
    /// How long this statement waits for memory admission on the
    /// coordinator, at most the configured wait; `0` refuses at once when
    /// the budget does not fit on arrival.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_wait_seconds: Option<u64>,
    /// `true` lets the planner answer a plain `COUNT(DISTINCT col)` from a
    /// HyperLogLog sketch — computed, or stored in the table's statistics
    /// — as `APPROX_COUNT_DISTINCT` would, the estimate's error stated on
    /// the query record. Off by default; `APPROX_*` functions need no
    /// setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approximate: Option<bool>,
    /// `false` bypasses every answer from statistics: the `context` path
    /// for `COUNT(*)`/`MIN`/`MAX` and the statistics path for `APPROX_*`
    /// both stand aside and the rows are read (`APPROX_*` computes its
    /// sketch over them). File skipping by the statistics' bounds still
    /// applies — pruning, not answering. On by default; a benchmark of
    /// the read path sets it beside `result_cache = false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_statistics: Option<bool>,
}

impl QuerySettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The statement's query memory pool: its own limit when it set one,
    /// else the node's, and never more than the node's.
    pub fn query_memory_limit_bytes(&self, config: &ServerConfig) -> u64 {
        self.query_memory_limit_bytes
            .map_or(config.query_memory_limit_bytes, |limit| {
                limit.min(config.query_memory_limit_bytes)
            })
            .max(1)
    }

    /// Whether the result cache may serve or keep this statement.
    pub fn result_cache_enabled(&self) -> bool {
        self.result_cache.unwrap_or(true)
    }

    /// Whether exact distinct counts may be answered from sketches.
    pub fn approximate(&self) -> bool {
        self.approximate.unwrap_or(false)
    }

    /// Whether a statement may be answered from statistics at all.
    pub fn use_statistics(&self) -> bool {
        self.use_statistics.unwrap_or(true)
    }

    /// How long the statement waits for memory admission: its own bound
    /// when it set one, else the node's, and never more than the node's.
    /// Zero when the node has no queue.
    pub fn admission_wait(&self, config: &ServerConfig) -> std::time::Duration {
        let ceiling = if config.memory_admission_queue == 0 {
            0
        } else {
            config.memory_admission_wait_seconds
        };
        std::time::Duration::from_secs(
            self.admission_wait_seconds
                .map_or(ceiling, |seconds| seconds.min(ceiling)),
        )
    }

    /// Validates the keys a request gave. Unknown keys and out-of-range
    /// values are refused with the key named; `config` supplies the ceilings.
    pub fn from_request(
        settings: &Map<String, Value>,
        config: &ServerConfig,
    ) -> Result<Self, SettingsError> {
        let mut validated = Self::default();
        for (key, value) in settings {
            match key.as_str() {
                "query_memory_limit_bytes" => {
                    let limit = unsigned(key, value)?;
                    let ceiling = config.query_memory_limit_bytes;
                    if limit == 0 || limit > ceiling {
                        return Err(SettingsError(format!(
                            "setting '{key}' must be between 1 and {ceiling}, the configured per-query limit"
                        )));
                    }
                    validated.query_memory_limit_bytes = Some(limit);
                }
                "local_parallelism" => {
                    let threads = unsigned(key, value)?;
                    let ceiling = kaveon_exec::local_parallel::configured_parallelism()
                        .map_err(|error| SettingsError(error.to_string()))?;
                    if threads == 0 || threads > ceiling as u64 {
                        return Err(SettingsError(format!(
                            "setting '{key}' must be between 1 and {ceiling}, this node's configured parallelism"
                        )));
                    }
                    validated.local_parallelism = Some(threads as usize);
                }
                "result_cache" => {
                    validated.result_cache = Some(boolean(key, value)?);
                }
                "approximate" => {
                    validated.approximate = Some(boolean(key, value)?);
                }
                "use_statistics" => {
                    validated.use_statistics = Some(boolean(key, value)?);
                }
                "admission_wait_seconds" => {
                    let seconds = unsigned(key, value)?;
                    let ceiling = config.memory_admission_wait_seconds;
                    if seconds > ceiling {
                        return Err(SettingsError(format!(
                            "setting '{key}' must be between 0 and {ceiling}, the configured admission wait"
                        )));
                    }
                    validated.admission_wait_seconds = Some(seconds);
                }
                "time_zone" => {
                    return Err(SettingsError(
                        "setting 'time_zone' is the request's time_zone field; set it there or with SET SESSION time_zone"
                            .into(),
                    ));
                }
                _ => {
                    return Err(SettingsError(format!("unknown setting '{key}'")));
                }
            }
        }
        Ok(validated)
    }
}

/// A refused setting: the message names the key and the bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError(pub String);

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// An unsigned integer, as a JSON number or as the digits of a `SET SESSION`
/// literal.
fn unsigned(key: &str, value: &Value) -> Result<u64, SettingsError> {
    let parsed = match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    };
    parsed.ok_or_else(|| SettingsError(format!("setting '{key}' must be an unsigned integer")))
}

/// A boolean, as JSON `true`/`false` or the words of a `SET SESSION` literal.
fn boolean(key: &str, value: &Value) -> Result<bool, SettingsError> {
    let parsed = match value {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "on" => Some(true),
            "false" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    };
    parsed.ok_or_else(|| SettingsError(format!("setting '{key}' must be true or false")))
}

/// The `SET SESSION` statements that led a request's statement text, and the
/// statement that follows them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SessionPrefix {
    /// Keys and literal values in the order given, `time_zone` included.
    pub assignments: Vec<(String, Value)>,
    /// The statement after the prefix, trimmed.
    pub statement: String,
}

/// Splits leading `SET SESSION <key> = <value>;` statements from `sql`. The
/// keywords are case-insensitive; the key is an identifier; the value is a
/// single-quoted string (with `''` for a quote), a number, or a bare word.
/// A request that is only `SET SESSION` statements is refused: there is no
/// session for the setting to outlive the request in.
pub fn split_session_prefix(sql: &str) -> Result<SessionPrefix, SettingsError> {
    let mut rest = sql.trim();
    let mut assignments = Vec::new();
    while let Some(after_session) =
        strip_keyword(rest, "SET").and_then(|after_set| strip_keyword(after_set, "SESSION"))
    {
        let (key, after_key) = take_identifier(after_session)
            .ok_or_else(|| SettingsError("SET SESSION requires a setting name".into()))?;
        let after_equals = after_key
            .trim_start()
            .strip_prefix('=')
            .ok_or_else(|| SettingsError(format!("SET SESSION {key} requires '= <value>'")))?;
        let (value, after_value) = take_literal(after_equals.trim_start())
            .ok_or_else(|| SettingsError(format!("SET SESSION {key} requires a value")))?;
        let after_value = after_value.trim_start();
        assignments.push((key.to_ascii_lowercase(), value));
        match after_value.strip_prefix(';') {
            Some(next) => rest = next.trim_start(),
            None if after_value.is_empty() => {
                rest = "";
                break;
            }
            None => {
                return Err(SettingsError(format!(
                    "SET SESSION {key} must end with ';' before the statement it applies to"
                )));
            }
        }
    }
    if !assignments.is_empty() && rest.trim_end_matches(';').trim().is_empty() {
        return Err(SettingsError(
            "SET SESSION applies only to the statement submitted with it: HTTP requests are stateless and there is no server-side session"
                .into(),
        ));
    }
    Ok(SessionPrefix {
        assignments,
        statement: rest.to_owned(),
    })
}

fn strip_keyword<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    let text = text.trim_start();
    let head = text.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &text[keyword.len()..];
    // A keyword ends where the identifier characters do.
    if rest
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(rest)
}

fn take_identifier(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    let end = text
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '.'))
        .map_or(text.len(), |(index, _)| index);
    if end == 0 || text.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    Some((&text[..end], &text[end..]))
}

fn take_literal(text: &str) -> Option<(Value, &str)> {
    if let Some(quoted) = text.strip_prefix('\'') {
        let mut value = String::new();
        let mut chars = quoted.char_indices().peekable();
        while let Some((index, c)) = chars.next() {
            if c == '\'' {
                if chars.peek().is_some_and(|(_, next)| *next == '\'') {
                    chars.next();
                    value.push('\'');
                    continue;
                }
                return Some((Value::String(value), &quoted[index + 1..]));
            }
            value.push(c);
        }
        return None;
    }
    let end = text
        .char_indices()
        .find(|(_, c)| c.is_whitespace() || *c == ';')
        .map_or(text.len(), |(index, _)| index);
    if end == 0 {
        return None;
    }
    let word = &text[..end];
    let value = if let Ok(number) = word.parse::<u64>() {
        Value::Number(number.into())
    } else if word.eq_ignore_ascii_case("true") {
        Value::Bool(true)
    } else if word.eq_ignore_ascii_case("false") {
        Value::Bool(false)
    } else {
        Value::String(word.to_owned())
    };
    Some((value, &text[end..]))
}

/// Folds `SET SESSION` assignments into the request's settings object. A key
/// given in both places must agree; `time_zone` is returned separately for
/// the request's own field.
pub fn merge_session_prefix(
    settings: &mut Map<String, Value>,
    prefix: &SessionPrefix,
) -> Result<Option<String>, SettingsError> {
    let mut time_zone = None;
    for (key, value) in &prefix.assignments {
        if key == "time_zone" {
            let Value::String(zone) = value else {
                return Err(SettingsError(
                    "SET SESSION time_zone requires a quoted zone name".into(),
                ));
            };
            time_zone = Some(zone.clone());
            continue;
        }
        match settings.get(key) {
            Some(existing) if !same_setting(existing, value) => {
                return Err(SettingsError(format!(
                    "setting '{key}' is given twice with different values"
                )));
            }
            _ => {
                settings.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(time_zone)
}

/// Two spellings of one value: `256` and `"256"`, `true` and `"true"`.
fn same_setting(left: &Value, right: &Value) -> bool {
    fn canonical(value: &Value) -> String {
        match value {
            Value::String(text) => text.trim().to_ascii_lowercase(),
            other => other.to_string(),
        }
    }
    canonical(left) == canonical(right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> ServerConfig {
        ServerConfig {
            query_memory_limit_bytes: 1 << 30,
            ..ServerConfig::default()
        }
    }

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn a_request_lowers_bounds_and_never_raises_them() {
        let config = config();
        let settings = QuerySettings::from_request(
            &map(json!({"query_memory_limit_bytes": 1 << 20})),
            &config,
        )
        .unwrap();
        assert_eq!(settings.query_memory_limit_bytes(&config), 1 << 20);
        assert!(!settings.is_default());

        let above = QuerySettings::from_request(
            &map(json!({"query_memory_limit_bytes": (1u64 << 30) + 1})),
            &config,
        )
        .unwrap_err();
        assert!(above.0.contains("query_memory_limit_bytes"), "{above}");
        assert!(above.0.contains("1073741824"), "{above}");
        assert!(
            QuerySettings::from_request(&map(json!({"query_memory_limit_bytes": 0})), &config)
                .is_err()
        );
        assert!(
            QuerySettings::from_request(&map(json!({"query_memory_limit_bytes": -5})), &config)
                .is_err()
        );
        assert_eq!(
            QuerySettings::default().query_memory_limit_bytes(&config),
            1 << 30
        );
    }

    #[test]
    fn parallelism_is_bounded_by_the_node() {
        let config = config();
        let ceiling = kaveon_exec::local_parallel::configured_parallelism().unwrap();
        let settings =
            QuerySettings::from_request(&map(json!({"local_parallelism": 1})), &config).unwrap();
        assert_eq!(settings.local_parallelism, Some(1));
        let error =
            QuerySettings::from_request(&map(json!({"local_parallelism": ceiling + 1})), &config)
                .unwrap_err();
        assert!(error.0.contains("local_parallelism"), "{error}");
        assert!(error.0.contains(&ceiling.to_string()), "{error}");
        assert!(
            QuerySettings::from_request(&map(json!({"local_parallelism": 0})), &config).is_err()
        );
        assert!(
            QuerySettings::from_request(&map(json!({"local_parallelism": "two"})), &config)
                .is_err()
        );
    }

    #[test]
    fn the_admission_wait_is_bounded_by_the_node_and_zero_means_no_wait() {
        let config = ServerConfig {
            memory_admission_queue: 8,
            memory_admission_wait_seconds: 30,
            ..config()
        };
        assert_eq!(
            QuerySettings::default().admission_wait(&config),
            std::time::Duration::from_secs(30)
        );
        let settings =
            QuerySettings::from_request(&map(json!({"admission_wait_seconds": 5})), &config)
                .unwrap();
        assert_eq!(
            settings.admission_wait(&config),
            std::time::Duration::from_secs(5)
        );
        let none =
            QuerySettings::from_request(&map(json!({"admission_wait_seconds": "0"})), &config)
                .unwrap();
        assert_eq!(none.admission_wait(&config), std::time::Duration::ZERO);
        let error =
            QuerySettings::from_request(&map(json!({"admission_wait_seconds": 31})), &config)
                .unwrap_err();
        assert!(error.0.contains("admission_wait_seconds"), "{error}");
        assert!(error.0.contains("30"), "{error}");
        assert!(
            QuerySettings::from_request(&map(json!({"admission_wait_seconds": -1})), &config)
                .is_err()
        );
        // No queue on the node: nothing waits, whatever the request asked.
        let unqueued = ServerConfig {
            memory_admission_queue: 0,
            ..config
        };
        assert_eq!(
            settings.admission_wait(&unqueued),
            std::time::Duration::ZERO
        );
    }

    #[test]
    fn unknown_keys_are_refused_by_name_and_time_zone_is_redirected() {
        let config = config();
        let error =
            QuerySettings::from_request(&map(json!({"max_spill_bytes": 1})), &config).unwrap_err();
        assert_eq!(error.0, "unknown setting 'max_spill_bytes'");
        let error =
            QuerySettings::from_request(&map(json!({"time_zone": "UTC"})), &config).unwrap_err();
        assert!(error.0.contains("time_zone field"), "{error}");
        let error = QuerySettings::from_request(&map(json!({"result_cache": "maybe"})), &config)
            .unwrap_err();
        assert!(error.0.contains("result_cache"), "{error}");
        let settings =
            QuerySettings::from_request(&map(json!({"result_cache": false})), &config).unwrap();
        assert!(!settings.result_cache_enabled());
        assert!(QuerySettings::default().result_cache_enabled());
    }

    #[test]
    fn approximate_and_use_statistics_are_booleans_off_and_on_by_default() {
        let config = config();
        let defaults = QuerySettings::default();
        assert!(!defaults.approximate());
        assert!(defaults.use_statistics());
        let settings = QuerySettings::from_request(
            &map(json!({"approximate": true, "use_statistics": "off"})),
            &config,
        )
        .unwrap();
        assert!(settings.approximate());
        assert!(!settings.use_statistics());
        assert_eq!(
            serde_json::to_value(&settings).unwrap(),
            json!({"approximate": true, "use_statistics": false})
        );
        for key in ["approximate", "use_statistics"] {
            let error = QuerySettings::from_request(&map(json!({key: 1})), &config).unwrap_err();
            assert_eq!(error.0, format!("setting '{key}' must be true or false"));
        }
        let prefix = split_session_prefix(
            "SET SESSION use_statistics = false; SET SESSION approximate = true; SELECT 1",
        )
        .unwrap();
        let mut settings = map(json!({}));
        merge_session_prefix(&mut settings, &prefix).unwrap();
        let settings = QuerySettings::from_request(&settings, &config).unwrap();
        assert!(!settings.use_statistics());
        assert!(settings.approximate());
    }

    #[test]
    fn serialisation_carries_only_what_was_set() {
        let settings = QuerySettings {
            local_parallelism: Some(2),
            ..QuerySettings::default()
        };
        assert_eq!(
            serde_json::to_value(&settings).unwrap(),
            json!({"local_parallelism": 2})
        );
        let parsed: QuerySettings = serde_json::from_value(json!({})).unwrap();
        assert!(parsed.is_default());
    }

    #[test]
    fn set_session_prefix_is_split_and_merged() {
        let prefix = split_session_prefix(
            "set session result_cache = false; SET SESSION local_parallelism = 2 ; SET SESSION time_zone = 'Europe/Dublin';\n SELECT 'SET SESSION x = 1' AS s",
        )
        .unwrap();
        assert_eq!(prefix.statement, "SELECT 'SET SESSION x = 1' AS s");
        assert_eq!(
            prefix.assignments,
            vec![
                ("result_cache".to_owned(), json!(false)),
                ("local_parallelism".to_owned(), json!(2)),
                ("time_zone".to_owned(), json!("Europe/Dublin")),
            ]
        );
        let mut settings = map(json!({"local_parallelism": "2"}));
        let time_zone = merge_session_prefix(&mut settings, &prefix).unwrap();
        assert_eq!(time_zone.as_deref(), Some("Europe/Dublin"));
        assert_eq!(settings.get("result_cache"), Some(&json!(false)));
        assert_eq!(settings.get("local_parallelism"), Some(&json!(2)));
        assert!(!settings.contains_key("time_zone"));

        let mut conflicting = map(json!({"local_parallelism": 3}));
        let error = merge_session_prefix(&mut conflicting, &prefix).unwrap_err();
        assert!(error.0.contains("local_parallelism"), "{error}");
    }

    #[test]
    fn set_session_alone_is_refused_and_plain_statements_pass_through() {
        let error = split_session_prefix("SET SESSION result_cache = false").unwrap_err();
        assert!(error.0.contains("stateless"), "{error}");
        let error = split_session_prefix("SET SESSION result_cache = false;").unwrap_err();
        assert!(error.0.contains("stateless"), "{error}");
        let plain = split_session_prefix("  SELECT 1 ").unwrap();
        assert!(plain.assignments.is_empty());
        assert_eq!(plain.statement, "SELECT 1");
        // A statement that merely starts with the word SET is not a prefix.
        let settings_table = split_session_prefix("SELECT * FROM settings").unwrap();
        assert!(settings_table.assignments.is_empty());
        let set_table = split_session_prefix("SETTINGS").unwrap();
        assert!(set_table.assignments.is_empty());
        assert!(split_session_prefix("SET SESSION = 1; SELECT 1").is_err());
        assert!(split_session_prefix("SET SESSION x 1; SELECT 1").is_err());
        assert!(split_session_prefix("SET SESSION x = 1 SELECT 1").is_err());
        assert!(split_session_prefix("SET SESSION x = 'unterminated; SELECT 1").is_err());
        let quoted = split_session_prefix("SET SESSION time_zone = 'O''Clock'; SELECT 1").unwrap();
        assert_eq!(quoted.assignments[0].1, json!("O'Clock"));
    }
}
