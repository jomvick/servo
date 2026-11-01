//! La face réseau locale : ce que l'agent écrit, et ce qu'on lui rend.
//!
//! Le pont lui-même (socket, token, `EngineHandle`) vient avec `webview-engine`, parce qu'il
//! n'a rien à dispatcher avant. Ce fichier-ci est le contrat, et il est vérifiable sans
//! moteur : l'enveloppe JSON-RPC 2.0, les noms `ego.*` de la v1, et la forme sous laquelle
//! une `EgoError` devient une réponse d'erreur.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use webview_core::{EgoError, SnapshotOptions, SpaceId, TabId};

pub const JSON_RPC: &str = "2.0";

/// Les codes de la spécification, plus la plage serveur sous laquelle l'API `ego.*` rend ses
/// propres erreurs.
pub mod rpc {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// Une erreur du modèle ou du moteur. JSON-RPC exige un nombre ; la chaîne `EGO_*`, elle,
    /// est dans `message` (les clients ego matchent dessus) et dans `data.code`.
    pub const EGO_ERROR: i64 = -32000;
}

/// L'identifiant d'une requête, tel que JSON-RPC l'autorise.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Id {
    Number(u64),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ErrorObject {
    pub fn rpc(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::rpc(rpc::INVALID_PARAMS, message)
    }

    pub fn ego(error: &EgoError) -> Self {
        Self {
            code: rpc::EGO_ERROR,
            message: error.code.as_str().to_owned(),
            data: Some(json!({ "code": error.code, "message": error.message })),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Reply {
    Success {
        jsonrpc: &'static str,
        id: Id,
        result: Value,
    },
    Failure {
        jsonrpc: &'static str,
        id: Option<Id>,
        error: ErrorObject,
    },
}

impl Reply {
    pub fn ok(id: Id, result: Value) -> Self {
        Self::Success {
            jsonrpc: JSON_RPC,
            id,
            result,
        }
    }

    /// `id` vaut `None` quand la requête était malformed à un point tel qu'aucun identifiant
    /// n'a pu en être tiré — la spécification rend alors `id: null`.
    pub fn error(id: Option<Id>, error: ErrorObject) -> Self {
        Self::Failure {
            jsonrpc: JSON_RPC,
            id,
            error,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a Reply always serializes")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// `None` = notification : elle s'exécute, et on ne répond pas.
    pub id: Option<Id>,
    pub method: String,
    pub params: Value,
}

impl Request {
    pub fn parse(raw: &str) -> Result<Self, Reply> {
        let value = match serde_json::from_str::<Value>(raw) {
            Ok(value) => value,
            Err(error) => {
                return Err(Reply::error(
                    None,
                    ErrorObject::rpc(rpc::PARSE_ERROR, error.to_string()),
                ));
            },
        };
        let (id, invalid_id) = match value.get("id") {
            None => (None, false),
            Some(Value::String(id)) => (Some(Id::Text(id.clone())), false),
            Some(Value::Number(id)) => match id.as_u64() {
                Some(id) => (Some(Id::Number(id)), false),
                None => (None, true),
            },
            Some(_) => (None, true),
        };
        if invalid_id {
            return Err(Reply::error(
                None,
                ErrorObject::rpc(
                    rpc::INVALID_REQUEST,
                    "`id` must be a non-negative integer or string",
                ),
            ));
        }
        if value.get("jsonrpc").and_then(Value::as_str) != Some(JSON_RPC) {
            return Err(Reply::error(
                id,
                ErrorObject::rpc(rpc::INVALID_REQUEST, "`jsonrpc` must be \"2.0\""),
            ));
        }
        let Some(method) = value.get("method").and_then(Value::as_str) else {
            return Err(Reply::error(
                id,
                ErrorObject::rpc(rpc::INVALID_REQUEST, "`method` is required"),
            ));
        };
        Ok(Self {
            id,
            method: method.to_owned(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        })
    }

    /// Le nom public est celui d'ego, avec son préfixe : `ego.snapshot`, `ego.createTab`.
    pub fn call(&self) -> Result<Call, ErrorObject> {
        let Some(name) = self.method.strip_prefix("ego.") else {
            return Err(ErrorObject::rpc(
                rpc::METHOD_NOT_FOUND,
                format!(
                    "unknown method `{}`; the bridge only speaks `ego.*`",
                    self.method
                ),
            ));
        };
        let defaults = SnapshotOptions::default();
        let params = &self.params;
        let call = match name {
            "getBrowserVersion" => Call::GetBrowserVersion,
            "listProfiles" => Call::ListProfiles,
            "listTaskSpaces" => Call::ListTaskSpaces,
            "createTaskSpace" => Call::CreateTaskSpace {
                name: string(params, "name")?,
            },
            "claimTaskSpace" => Call::ClaimTaskSpace {
                space: space(params)?,
            },
            "handOffTaskSpace" => Call::HandOffTaskSpace {
                space: space(params)?,
            },
            "takeOverTaskSpace" => Call::TakeOverTaskSpace {
                space: space(params)?,
            },
            "completeTaskSpace" => Call::CompleteTaskSpace {
                space: space(params)?,
            },
            "closeTaskSpace" => Call::CloseTaskSpace {
                space: space(params)?,
            },
            "listTabs" => Call::ListTabs {
                space: space(params)?,
            },
            "createTab" => Call::CreateTab {
                space: space(params)?,
                url: optional_string(params, "url")?,
            },
            "closeTab" => Call::CloseTab { tab: tab(params)? },
            "navigate" => Call::Navigate {
                tab: tab(params)?,
                url: string(params, "url")?,
            },
            "snapshot" => Call::Snapshot {
                tab: tab(params)?,
                options: SnapshotOptions {
                    max_chars: usize_or(params, "maxChars", defaults.max_chars)?,
                    interactive_only: bool_or(
                        params,
                        "interactiveOnly",
                        defaults.interactive_only,
                    )?,
                },
            },
            "evaluate" => Call::Evaluate {
                tab: tab(params)?,
                expression: string(params, "expression")?,
            },
            "click" => Call::Click {
                tab: tab(params)?,
                reference: reference(params)?,
            },
            "fill" => Call::Fill {
                tab: tab(params)?,
                reference: reference(params)?,
                value: string(params, "value")?,
            },
            "type" => Call::TypeText {
                tab: tab(params)?,
                text: string(params, "text")?,
            },
            "scroll" => Call::Scroll {
                tab: tab(params)?,
                delta_x: f64_or(params, "deltaX", 0.0)?,
                delta_y: f64_or(params, "deltaY", 0.0)?,
            },
            "capture" => Call::Capture { tab: tab(params)? },
            "sendCDPMessage" => Call::SendCdpMessage {
                tab: tab(params)?,
                message: required(params, "message")?.clone(),
            },
            _ => {
                return Err(ErrorObject::rpc(
                    rpc::METHOD_NOT_FOUND,
                    format!("unknown method `ego.{name}`"),
                ));
            },
        };
        Ok(call)
    }
}

/// Parse and dispatch one JSON-RPC message. The callback must run on the engine thread and
/// return a concrete result or model error for every `Call`; no method is silently accepted.
/// Notifications are dispatched but never produce a response.
pub fn handle_json_rpc<F>(raw: &str, mut dispatch: F) -> Option<String>
where
    F: FnMut(Call) -> Result<Value, EgoError>,
{
    let request = match Request::parse(raw) {
        Ok(request) => request,
        Err(reply) => return Some(reply.to_json()),
    };
    let id = request.id.clone();
    let call = match request.call() {
        Ok(call) => call,
        Err(error) => return id.map(|id| Reply::error(Some(id), error).to_json()),
    };
    let result = dispatch(call).map_err(|error| ErrorObject::ego(&error));
    id.map(|id| match result {
        Ok(result) => Reply::ok(id, result),
        Err(error) => Reply::error(Some(id), error),
    }
    .to_json())
}

/// Les appels de la v1. `useTaskSpace` (ego) n'a pas d'équivalent ici : chez ego il choisit le
/// space courant d'un agent, ce qui correspond à `claimTaskSpace` dans le modèle de D6.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    GetBrowserVersion,
    ListProfiles,
    ListTaskSpaces,
    CreateTaskSpace {
        name: String,
    },
    ClaimTaskSpace {
        space: SpaceId,
    },
    HandOffTaskSpace {
        space: SpaceId,
    },
    TakeOverTaskSpace {
        space: SpaceId,
    },
    CompleteTaskSpace {
        space: SpaceId,
    },
    CloseTaskSpace {
        space: SpaceId,
    },
    ListTabs {
        space: SpaceId,
    },
    CreateTab {
        space: SpaceId,
        url: Option<String>,
    },
    CloseTab {
        tab: TabId,
    },
    Navigate {
        tab: TabId,
        url: String,
    },
    Snapshot {
        tab: TabId,
        options: SnapshotOptions,
    },
    Evaluate {
        tab: TabId,
        expression: String,
    },
    Click {
        tab: TabId,
        reference: String,
    },
    Fill {
        tab: TabId,
        reference: String,
        value: String,
    },
    TypeText {
        tab: TabId,
        text: String,
    },
    Scroll {
        tab: TabId,
        delta_x: f64,
        delta_y: f64,
    },
    Capture {
        tab: TabId,
    },
    SendCdpMessage {
        tab: TabId,
        message: Value,
    },
}

fn usize_or(params: &Value, key: &str, default: usize) -> Result<usize, ErrorObject> {
    Ok(match as_u64(params, key)? {
        Some(value) => usize::try_from(value).unwrap_or(usize::MAX),
        None => default,
    })
}

fn f64_or(params: &Value, key: &str, default: f64) -> Result<f64, ErrorObject> {
    Ok(as_f64(params, key)?.unwrap_or(default))
}

fn required<'a>(params: &'a Value, key: &str) -> Result<&'a Value, ErrorObject> {
    params
        .get(key)
        .filter(|value| !value.is_null())
        .ok_or_else(|| ErrorObject::invalid_params(format!("`{key}` is required")))
}

fn string(params: &Value, key: &str) -> Result<String, ErrorObject> {
    text(params, key)?.ok_or_else(|| ErrorObject::invalid_params(format!("`{key}` is required")))
}

fn optional_string(params: &Value, key: &str) -> Result<Option<String>, ErrorObject> {
    if params.get(key).is_none_or(Value::is_null) {
        return Ok(None);
    }
    text(params, key)
}

fn text(params: &Value, key: &str) -> Result<Option<String>, ErrorObject> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ErrorObject::invalid_params(format!(
            "`{key}` must be a string"
        ))),
    }
}

fn as_u64(params: &Value, key: &str) -> Result<Option<u64>, ErrorObject> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .map(Some)
            .ok_or_else(|| ErrorObject::invalid_params(format!("`{key}` must be a whole number"))),
        Some(_) => Err(ErrorObject::invalid_params(format!(
            "`{key}` must be a number"
        ))),
    }
}

fn as_f64(params: &Value, key: &str) -> Result<Option<f64>, ErrorObject> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_f64()
            .map(Some)
            .ok_or_else(|| ErrorObject::invalid_params(format!("`{key}` must be a number"))),
        Some(_) => Err(ErrorObject::invalid_params(format!(
            "`{key}` must be a number"
        ))),
    }
}

fn bool_or(params: &Value, key: &str, default: bool) -> Result<bool, ErrorObject> {
    Ok(match params.get(key) {
        None | Some(Value::Null) => default,
        Some(Value::Bool(value)) => *value,
        Some(_) => {
            return Err(ErrorObject::invalid_params(format!(
                "`{key}` must be a boolean"
            )));
        },
    })
}

fn space(params: &Value) -> Result<SpaceId, ErrorObject> {
    as_u64(params, "space")?
        .map(SpaceId)
        .ok_or_else(|| ErrorObject::invalid_params("`space` is required"))
}

fn tab(params: &Value) -> Result<TabId, ErrorObject> {
    as_u64(params, "tab")?
        .map(TabId)
        .ok_or_else(|| ErrorObject::invalid_params("`tab` is required"))
}

/// ego écrit `ref` ; le modèle l'appelle `reference`. Les deux se lisent, `reference` gagne.
fn reference(params: &Value) -> Result<String, ErrorObject> {
    text(params, "reference")?
        .or(text(params, "ref")?)
        .ok_or_else(|| ErrorObject::invalid_params("`ref` is required"))
}
