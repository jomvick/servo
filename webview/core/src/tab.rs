use serde::{Deserialize, Serialize};

use crate::ids::{SpaceId, TabId};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Tab {
    pub id: TabId,
    pub space: SpaceId,
    pub url: Option<String>,
    pub title: Option<String>,
    pub loading: bool,
}
