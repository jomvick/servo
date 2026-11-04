use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

/// Les codes d'erreur publiés par l'API `ego.*` d'ego-lite. Le nom Rust reste camelCase, la
/// forme sérialisée est le code `EGO_*` que l'agent attend côté fil.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
pub enum ErrorCode {
    #[serde(rename = "EGO_TASK_SPACE_NOT_FOUND")]
    TaskSpaceNotFound,
    #[serde(rename = "EGO_TASK_SPACE_USER_IN_CONTROL")]
    TaskSpaceUserInControl,
    #[serde(rename = "EGO_TASK_SPACE_INACTIVE")]
    TaskSpaceInactive,
    #[serde(rename = "EGO_WEB_CONTENTS_UNAVAILABLE")]
    WebContentsUnavailable,
    #[serde(rename = "EGO_SNAPSHOT_FAILED")]
    SnapshotFailed,
    #[serde(rename = "EGO_INVALID_ARGUMENT")]
    InvalidArgument,
    #[serde(rename = "EGO_TASK_HOST_DISCONNECTED")]
    TaskHostDisconnected,
    #[serde(rename = "EGO_OPERATION_FAILED")]
    OperationFailed,
    #[serde(rename = "EGO_CDP_CHANNEL_UNAVAILABLE")]
    CdpChannelUnavailable,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskSpaceNotFound => "EGO_TASK_SPACE_NOT_FOUND",
            Self::TaskSpaceUserInControl => "EGO_TASK_SPACE_USER_IN_CONTROL",
            Self::TaskSpaceInactive => "EGO_TASK_SPACE_INACTIVE",
            Self::WebContentsUnavailable => "EGO_WEB_CONTENTS_UNAVAILABLE",
            Self::SnapshotFailed => "EGO_SNAPSHOT_FAILED",
            Self::InvalidArgument => "EGO_INVALID_ARGUMENT",
            Self::TaskHostDisconnected => "EGO_TASK_HOST_DISCONNECTED",
            Self::OperationFailed => "EGO_OPERATION_FAILED",
            Self::CdpChannelUnavailable => "EGO_CDP_CHANNEL_UNAVAILABLE",
        }
    }
}

impl Display for ErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct EgoError {
    pub code: ErrorCode,
    pub message: String,
}

impl EgoError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl Display for EgoError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl Error for EgoError {}
